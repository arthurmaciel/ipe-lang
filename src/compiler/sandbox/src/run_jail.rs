//! The runtime jail around the *emitted app* (as opposed to the build-time jail
//! around an untrusted crate compile in [`crate`]).
//!
//! A program's capability set is inferred from pure Ipê and declared for native
//! `Rust.` code, but nothing confines the emitted binary when it *runs*: a Tier
//! 2 wrapper is arbitrary native Rust that can make any syscall. This module is
//! the fail-closed jail that runs the binary confined to exactly its
//! declared-plus-inferred set — declared effects work, undeclared ones are
//! impossible.
//!
//! The pipeline is: a [`Capability`] set → a platform-independent
//! [`SandboxProfile`] ([`profile_from_capabilities`]) → a `bwrap` argv + a
//! seccomp program ([`run_jail_argv`] + [`crate::seccomp`]). The profile is the
//! serializable value the built artifact carries (`ipe.profile`); the argv is
//! what a launcher execs.
//!
//! ## Invariants
//!
//! - **Deny-by-default, structurally.** [`profile_from_capabilities`] is an
//!   exhaustive `match Capability` with no `_` catch-all, so a newly-added
//!   capability variant fails to *compile* until it is classified — an
//!   unclassified variant can never default to "allowed". [`SandboxProfile`] has
//!   no `Default` that yields an all-allowed value: the empty set lowers to the
//!   maximally-isolated profile.
//! - **Fail-closed on an unknown database driver.** `database` lowers to
//!   `network` or `filesystem` per the driver; an unknown/missing driver is an
//!   error, never a silently-dropped axis.
//! - **Reuse the mechanism, not the numbers.** The argv reuses the build jail's
//!   `bwrap`/`prlimit` flag vocabulary but defines its own [`RunResourceLimits`]
//!   (no wall-clock kill by default — a long-lived server is legitimate) and
//!   adds the baseline denials the build argv lacks (`--proc /proc`,
//!   `no_new_privs`, the seccomp filter).

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use ipe_diagnostics::{Code, Diagnostic as SharedDiag, IPE_F4413, SandboxError};
use ipe_kernels::Capability;

// The seccomp program is the Linux (x86_64 or aarch64) lowering of the
// subprocess axis; off those targets no argv/seccomp this crate builds would
// confine the app (the documented refuse-gap), so the import is Linux-only.
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
use crate::seccomp;

/// Stamp `$item` with the `#[cfg(...)]` for the targets that HAVE a real run
/// jail compiled into [`exec_in_run_jail`], and the negation on a matching `no:`
/// item — the ONE place the supported-target set is written as a predicate.
///
/// The supported set is Linux `x86_64`/`aarch64` (the `bwrap`+seccomp jail), macOS (the
/// `sandbox-exec` SBPL jail), and Windows (the Job Object + `AppContainer` +
/// launcher-scrub jail). The value [`platform_supports_jail`] returns
/// (`JAIL_COMPILED_IN`) is stamped through this macro, and the refuse-stub
/// `exec_in_run_jail` arm is gated on the NEGATION of this same predicate. So
/// `platform_supports_jail` is `true` exactly where a real jail arm compiles: the
/// admit verdict (`Holds` vs `RefuseGap`) and the jail actually compiled in cannot
/// drift, and a target with only the stub arm reports "refuse", fail-closed. The
/// per-OS real arms keep their own precise `#[cfg]` (their bodies differ), and the
/// `platform_supports_jail_matches_the_compiled_in_jail_arm` unit test asserts the
/// predicate spelled here equals the one those arms use.
///
/// Being a jailed target makes [`platform_supports_jail`] `true`; it does NOT
/// imply the target confines EVERY axis. Linux and macOS confine the full set,
/// but Windows is PARTIAL (see [`platform_confined_axes`]): its jail confines
/// subprocess + env, and filesystem + network + database only under
/// `AppContainer` on an ACL volume. [`CONFINED_AXES`] is therefore NOT stamped by
/// this macro — it is a separate per-OS `#[cfg]` so a jailed-but-partial target
/// can list fewer axes than it compiles an arm for.
macro_rules! on_jailed_target {
    (yes: { $($yes:item)* } no: { $($no:item)* }) => {
        $(
            #[cfg(any(
                all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64")),
                target_os = "macos",
                target_os = "windows"
            ))]
            $yes
        )*
        $(
            #[cfg(not(any(
                all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64")),
                target_os = "macos",
                target_os = "windows"
            )))]
            $no
        )*
    };
}

// ── the database axis (a run-jail input, resolved from ipe.toml) ─────────────

/// How `Capability::Database` lowers for this project.
///
/// The driver decides whether a database effect is really a network effect (a
/// TCP driver) or a filesystem effect (an embedded/`SQLite` file). Resolved by
/// the CLI from the `ipe.toml` driver selection before the profile is built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseAxis {
    /// A TCP-connected database (`Postgres`, `MySQL`, …) → the `network` control.
    Network,
    /// A file-backed database (`SQLite`, an embedded store) → the `filesystem`
    /// control.
    Filesystem,
    /// No `database` capability is present, so the driver is irrelevant. Kept
    /// distinct from a *missing* driver: the caller supplies this only when the
    /// set does not include `database`, so an unknown driver where one is needed
    /// is still an error.
    NotApplicable,
}

/// Why a capability set could not be lowered to a profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileError {
    /// The set includes `database` but the `ipe.toml` driver could not be
    /// resolved to a concrete axis. Fail-closed: dropping the axis would
    /// *tighten* the jail past what the program needs (a false-deny), and
    /// guessing an axis could under-isolate — so this refuses.
    UnknownDatabaseDriver,
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownDatabaseDriver => write!(
                f,
                "the program uses `database`, but the ipe.toml database driver could not be \
                 resolved to a concrete axis (network or filesystem); refusing to build a jail \
                 that might over- or under-isolate it"
            ),
        }
    }
}

impl std::error::Error for ProfileError {}

// ── the platform-independent profile ────────────────────────────────────────

/// The filesystem scope a program runs under.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FilesystemScope {
    /// `filesystem` absent: `/` read-only, home/tmp masked, exactly one scoped
    /// writable tempdir. The maximally-isolated filesystem view.
    Isolated,
    /// `filesystem` granted: the working tree is bound read-write (still coarse
    /// — any path under it — per the first-cut map).
    WorkingTreeReadWrite,
}

/// The resource caps for a run-jailed app.
///
/// Distinct from the build jail's `ResourceLimits`, whose values (10 GiB AS,
/// 900 s wall/CPU) are tuned to kill a giant one-shot rustdoc and would wrongly
/// kill a legitimate long-lived server (a false-deny).
///
/// The mechanism (`prlimit` + optional `timeout`) is reused; the numbers are
/// not. `wall_secs = None` means no wall-clock kill (a server runs
/// indefinitely); a runaway is still bounded by the CPU/memory/proc caps, which
/// stay mandatory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RunResourceLimits {
    /// Address-space cap in bytes.
    pub as_bytes: u64,
    /// CPU-seconds cap (bounds a busy-loop even without a wall clock).
    pub cpu_secs: u64,
    /// Wall-clock cap in seconds, or `None` for no wall-clock kill.
    pub wall_secs: Option<u64>,
    /// Open-file-descriptor cap.
    pub fd_cap: u64,
    /// Process-count cap.
    pub proc_cap: u64,
}

impl Default for RunResourceLimits {
    fn default() -> Self {
        // A long-lived app, not a one-shot build: no wall clock, a generous CPU
        // ceiling (a runaway loop is still bounded, a slow server is not), a
        // large-but-finite address space, and sane FD/process caps.
        Self {
            as_bytes: 4 * 1024 * 1024 * 1024,
            cpu_secs: 86_400,
            wall_secs: None,
            fd_cap: 4096,
            proc_cap: 512,
        }
    }
}

/// The platform-independent description of a run jail.
///
/// What the emitted app may touch, derived from its capability set. A
/// per-platform *builder* ([`run_jail_argv`] on Linux) turns this into a
/// concrete jail.
///
/// This is the value serialized into an artifact's `ipe.profile`. It is
/// deliberately NOT `Default`-constructible to an all-allowed value — the empty
/// capability set lowers to the maximally-isolated profile
/// ([`profile_from_capabilities`]), and a profile is only ever *built from a
/// set*, never conjured permissive.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SandboxProfile {
    /// `true` iff `network` is granted — the net namespace is shared with the
    /// host. `false` → a fresh empty net namespace (loopback only).
    pub network: bool,
    /// The filesystem scope.
    pub filesystem: FilesystemScope,
    /// The environment variables re-exported into the scrubbed env (the `env`
    /// axis's allowlist). Empty when `env` is absent.
    pub env_allowlist: Vec<String>,
    /// `true` iff `subprocess` is granted — the seccomp filter permits the
    /// task-creation family. `false` → the create family is denied (threads
    /// excepted).
    pub subprocess: bool,
    /// The resource caps.
    pub limits: RunResourceLimits,
}

impl SandboxProfile {
    /// The maximally-isolated profile: nothing granted. This is what the empty
    /// capability set lowers to, and the floor a tampered `ipe.profile` cannot
    /// go below undetected.
    #[must_use]
    pub fn maximally_isolated() -> Self {
        Self {
            network: false,
            filesystem: FilesystemScope::Isolated,
            env_allowlist: Vec::new(),
            subprocess: false,
            limits: RunResourceLimits::default(),
        }
    }

    /// Whether `self`'s filesystem scope isolates at least as much as `floor`'s:
    /// `Isolated` isolates more than (or equal to) any scope; a read-write tree
    /// is only OK if the floor also grants a read-write tree.
    #[must_use]
    const fn fs_at_least_as_isolated(&self, floor: &Self) -> bool {
        matches!(self.filesystem, FilesystemScope::Isolated)
            || matches!(floor.filesystem, FilesystemScope::WorkingTreeReadWrite)
    }

    /// The launcher's floor check against a [`parse_capfloor`]-derived floor.
    ///
    /// This is the single implementation of the profile-vs-floor anti-tamper
    /// predicate. Every axis comparison lives here; adding a new axis forces every
    /// call site to see the change. The embedded floor records the axis grants AND
    /// the exact set of granted env var *names* ([`to_capfloor_line`] serializes
    /// them), so the env axis is compared by name subset — identical to the other
    /// axes: every env var the profile grants must be one the floor also grants. A
    /// tampered `ipe.profile` that swaps *which* env vars it grants (even at the
    /// same count) is refused.
    ///
    /// [`to_capfloor_line`]: Self::to_capfloor_line
    #[must_use]
    pub fn satisfies_capfloor(&self, floor: &Self) -> bool {
        let network_ok = !self.network || floor.network;
        let subprocess_ok = !self.subprocess || floor.subprocess;
        let fs_ok = self.fs_at_least_as_isolated(floor);
        // Every env var the profile grants must be in the floor's allowlist — the
        // same ⊆ subset check the other axes get, now that the floor carries the
        // names, not just a count.
        let env_ok = self
            .env_allowlist
            .iter()
            .all(|v| floor.env_allowlist.contains(v));
        network_ok && subprocess_ok && fs_ok && env_ok
    }

    /// Whether `self` isolates *at least* as much as `floor` on every axis.
    ///
    /// Delegates to [`satisfies_capfloor`] — the single implementation of the
    /// per-axis tamper check — so the two cannot diverge.
    ///
    /// [`satisfies_capfloor`]: Self::satisfies_capfloor
    #[must_use]
    pub fn is_at_least_as_isolated_as(&self, floor: &Self) -> bool {
        self.satisfies_capfloor(floor)
    }

    /// Serialize the profile to the strict, line-oriented `ipe.profile` text.
    ///
    /// A tiny explicit grammar (not a general format) keeps the launcher's
    /// [`parse_profile`] a genuine *parse* — every field is named, every value is
    /// a fixed token, and anything unrecognized is a hard parse failure (⇒
    /// refuse-to-run), never a permissive default.
    #[must_use]
    pub fn to_profile_string(&self) -> String {
        use std::fmt::Write as _;
        let fs = match self.filesystem {
            FilesystemScope::Isolated => "isolated",
            FilesystemScope::WorkingTreeReadWrite => "working-tree-rw",
        };
        let mut s = String::from("ipe-profile 1\n");
        // Writing to a String is infallible, so the `write!` results are ignored.
        let _ = writeln!(s, "network {}", self.network);
        let _ = writeln!(s, "filesystem {fs}");
        let _ = writeln!(s, "subprocess {}", self.subprocess);
        for name in &self.env_allowlist {
            let _ = writeln!(s, "env {name}");
        }
        s
    }

    /// The compact capability-floor token line embedded read-only in the binary's
    /// `.rodata`. It records the *axis* grants AND the exact set of granted env
    /// var names (not resource limits — the launcher rebuilds a comparison floor
    /// from these), so a tampered `ipe.profile` can neither claim fewer axes nor
    /// swap *which* env vars it grants below what the binary was built for.
    ///
    /// Format: `ipe-capfloor 1 net=<b> fs=<isolated|rw> sub=<b> env=<names>` where
    /// `<names>` is the sorted, comma-joined set of granted env names (empty when
    /// none). The names are bound by identity, so the floor compares env by the
    /// SAME ⊆ subset check as the other axes — a same-count name swap no longer
    /// passes. Env var names are POSIX identifiers (`[A-Za-z_][A-Za-z0-9_]*`), so
    /// they never contain a comma or whitespace; [`parse_capfloor`] fails closed
    /// on any name that does.
    #[must_use]
    pub fn to_capfloor_line(&self) -> String {
        let fs = match self.filesystem {
            FilesystemScope::Isolated => "isolated",
            FilesystemScope::WorkingTreeReadWrite => "rw",
        };
        let mut names: Vec<&str> = self.env_allowlist.iter().map(String::as_str).collect();
        names.sort_unstable();
        names.dedup();
        format!(
            "ipe-capfloor 1 net={} fs={fs} sub={} env={}",
            self.network,
            self.subprocess,
            names.join(",")
        )
    }
}

/// Why an `ipe.profile` or capfloor could not be parsed. A parse failure is a
/// **refuse-to-run**, never a permissive fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// A malformed or unrecognized line/field.
    Malformed {
        /// A short reason.
        detail: String,
    },
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed { detail } => write!(f, "malformed profile: {detail}"),
        }
    }
}

impl std::error::Error for ParseError {}

/// Strictly parse an `ipe.profile` string into a [`SandboxProfile`].
///
/// Parse-don't-validate: an unknown key, a malformed boolean, a missing required
/// field, or an unknown filesystem token is a hard [`ParseError`] — the launcher
/// refuses to run rather than fall back to anything permissive. Resource limits
/// are not in the wire format (they are a run-jail default, not a confinement
/// axis); the parsed profile uses [`RunResourceLimits::default`].
///
/// # Errors
///
/// [`ParseError::Malformed`] on any grammar violation.
pub fn parse_profile(text: &str) -> Result<SandboxProfile, ParseError> {
    let malformed = |detail: &str| ParseError::Malformed {
        detail: detail.to_owned(),
    };
    let parse_bool = |v: &str| match v {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(ParseError::Malformed {
            detail: format!("expected true/false, got {other:?}"),
        }),
    };

    let mut lines = text.lines();
    let header = lines.next().ok_or_else(|| malformed("empty profile"))?;
    if header != "ipe-profile 1" {
        return Err(malformed("missing or unsupported `ipe-profile 1` header"));
    }

    // Deny-by-default: start maximally isolated, relax only what the file names.
    let mut network: Option<bool> = None;
    let mut filesystem: Option<FilesystemScope> = None;
    let mut subprocess: Option<bool> = None;
    let mut env_allowlist: Vec<String> = Vec::new();

    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let (key, value) = line
            .split_once(' ')
            .ok_or_else(|| malformed(&format!("expected `key value`, got {line:?}")))?;
        match key {
            "network" => network = Some(parse_bool(value)?),
            "filesystem" => {
                filesystem = Some(match value {
                    "isolated" => FilesystemScope::Isolated,
                    "working-tree-rw" => FilesystemScope::WorkingTreeReadWrite,
                    other => return Err(malformed(&format!("unknown filesystem scope {other:?}"))),
                });
            }
            "subprocess" => subprocess = Some(parse_bool(value)?),
            "env" => env_allowlist.push(value.to_owned()),
            other => return Err(malformed(&format!("unknown key {other:?}"))),
        }
    }

    Ok(SandboxProfile {
        network: network.ok_or_else(|| malformed("missing `network`"))?,
        filesystem: filesystem.ok_or_else(|| malformed("missing `filesystem`"))?,
        subprocess: subprocess.ok_or_else(|| malformed("missing `subprocess`"))?,
        env_allowlist,
        limits: RunResourceLimits::default(),
    })
}

/// Strictly parse a capfloor line into the *comparison floor*.
///
/// The result is a [`SandboxProfile`] whose axes AND env allowlist are the floor
/// the binary was built with. The env names are parsed from the sorted,
/// comma-joined `env=<names>` field ([`SandboxProfile::to_capfloor_line`]), so
/// the launcher can verify the profile grants only env vars the floor also
/// grants — an exact name subset, not a count.
///
/// # Errors
///
/// [`ParseError::Malformed`] on any grammar violation — a floor that cannot be
/// parsed means the binary's authoritative floor is unreadable, so the launcher
/// must refuse (never treat an unreadable floor as "no floor"). An env name that
/// is empty or carries a comma/whitespace (impossible for a POSIX env name) is a
/// malformed floor and refuses.
pub fn parse_capfloor(line: &str) -> Result<SandboxProfile, ParseError> {
    let malformed = |detail: String| ParseError::Malformed { detail };
    let line = line.trim();
    let mut parts = line.split_whitespace();
    if parts.next() != Some("ipe-capfloor") || parts.next() != Some("1") {
        return Err(malformed("missing `ipe-capfloor 1` header".to_owned()));
    }
    let mut network = false;
    let mut filesystem = FilesystemScope::Isolated;
    let mut subprocess = false;
    let mut env_allowlist: Vec<String> = Vec::new();
    for field in parts {
        let (k, v) = field
            .split_once('=')
            .ok_or_else(|| malformed(format!("expected key=value, got {field:?}")))?;
        match k {
            "net" => network = v == "true",
            "fs" => {
                filesystem = match v {
                    "isolated" => FilesystemScope::Isolated,
                    "rw" => FilesystemScope::WorkingTreeReadWrite,
                    other => return Err(malformed(format!("unknown fs {other:?}"))),
                };
            }
            "sub" => subprocess = v == "true",
            "env" => {
                env_allowlist = parse_capfloor_env_names(v)?;
            }
            other => return Err(malformed(format!("unknown capfloor field {other:?}"))),
        }
    }
    Ok(SandboxProfile {
        network,
        filesystem,
        subprocess,
        env_allowlist,
        limits: RunResourceLimits::default(),
    })
}

/// Parse the `env=<names>` field of a capfloor line: a sorted, comma-joined set
/// of env var names, empty when no env var is granted.
///
/// Fail-closed: each name must be a non-empty POSIX env identifier
/// (`[A-Za-z_][A-Za-z0-9_]*`). An empty name (a stray/leading/trailing comma) or
/// any name outside that charset — neither of which [`SandboxProfile::to_capfloor_line`]
/// can emit — is a malformed floor and refuses. Since the writer already sorts
/// and dedups, the parsed set is returned as-is.
///
/// # Errors
///
/// [`ParseError::Malformed`] on an empty or non-identifier name.
fn parse_capfloor_env_names(v: &str) -> Result<Vec<String>, ParseError> {
    if v.is_empty() {
        return Ok(Vec::new());
    }
    let mut names = Vec::new();
    for name in v.split(',') {
        if !is_posix_env_name(name) {
            return Err(ParseError::Malformed {
                detail: format!("malformed env name {name:?} in capfloor"),
            });
        }
        names.push(name.to_owned());
    }
    Ok(names)
}

/// Whether `name` is a non-empty POSIX environment-variable identifier: a first
/// char of `[A-Za-z_]`, then `[A-Za-z0-9_]*`. This is the charset the capfloor
/// `env=` field admits — anything else (a comma, whitespace, a digit-leading
/// name) is rejected, so the comma-joined encoding is unambiguous and a tampered
/// floor cannot smuggle a separator into a name.
fn is_posix_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Lower a capability set to a [`SandboxProfile`].
///
/// The profile set is `inferred ∪ declared` — the union guarantees *no
/// false-deny*: every capability anyone claims the program has is relaxed, so a
/// legitimately-declared effect is never blocked. `db_axis` resolves `database`
/// to a concrete axis (see [`DatabaseAxis`]); it is consulted only when the set
/// includes `database`.
///
/// The `match Capability` is **exhaustive with no `_`**: a new capability
/// variant fails to compile here until classified, so it can never silently
/// default to "allowed".
///
/// # Errors
///
/// [`ProfileError::UnknownDatabaseDriver`] when the set includes `database` but
/// `db_axis` is [`DatabaseAxis::NotApplicable`] (the driver could not be
/// resolved) — fail-closed rather than drop or mis-lower the axis.
pub fn profile_from_capabilities(
    inferred: &BTreeSet<Capability>,
    declared: &BTreeSet<Capability>,
    db_axis: DatabaseAxis,
    env_allowlist: &[String],
) -> Result<SandboxProfile, ProfileError> {
    let mut profile = SandboxProfile::maximally_isolated();

    // The union is the authoritative set (no false-deny).
    let union: BTreeSet<Capability> = inferred.union(declared).copied().collect();

    for &cap in &union {
        // The empty `clock/random` and `native-ffi` arms are DELIBERATELY
        // separate (not merged): each capability variant is explicitly
        // classified here, so a newly-added variant fails to compile until it is
        // given an arm — the deny-by-default structural guarantee. Merging the
        // no-op arms would defeat that, so the identical-arms lint is allowed.
        #[allow(clippy::match_same_arms)]
        match cap {
            Capability::Network => profile.network = true,
            Capability::Filesystem => {
                profile.filesystem = FilesystemScope::WorkingTreeReadWrite;
            }
            Capability::Database => match db_axis {
                DatabaseAxis::Network => profile.network = true,
                DatabaseAxis::Filesystem => {
                    profile.filesystem = FilesystemScope::WorkingTreeReadWrite;
                }
                DatabaseAxis::NotApplicable => return Err(ProfileError::UnknownDatabaseDriver),
            },
            Capability::Env => {
                // The env axis is granted: the manifest-named variables re-enter
                // the scrubbed environment. An empty allowlist here still means
                // "env granted but nothing named to re-export" — the axis is on,
                // the set of exported names is just empty.
                profile.env_allowlist = env_allowlist.to_vec();
            }
            Capability::Subprocess => profile.subprocess = true,
            // `clock`/`random` carry no OS control in the first cut: denying
            // time or RNG syscalls breaks far more than it contains, and neither
            // is a high-value exfiltration axis. Explicit no-op arms, not a
            // catch-all, so a new variant is still forced to be classified.
            Capability::Clock | Capability::Random => {}
            // `native-ffi` widens nothing by itself — its role is epistemic (it
            // means inference is blind, so the declared set is the ceiling). It
            // opens no control here; an explicit no-op arm. `ffi-raw` is the
            // same crossing under an author-asserted signature: pure
            // disclosure, no control of its own.
            Capability::NativeFfi | Capability::FfiRaw => {}
            // `unsafe` marks a value minted by assertion rather than by parse.
            // Like the FFI arms it is pure provenance disclosure, not a resource
            // axis the jail can open or close — an explicit no-op arm.
            Capability::Unsafe => {}
        }
    }

    Ok(profile)
}

// ── the Linux jail argv builder ─────────────────────────────────────────────

/// The paths of the host tools the run jail needs. `bwrap` and `prlimit` are
/// mandatory; `timeout` is only needed when the profile sets a wall-clock cap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunJailTools {
    /// `bwrap` — the namespaces + `--seccomp` loader.
    pub bwrap: PathBuf,
    /// `prlimit` — the resource caps (mandatory).
    pub prlimit: PathBuf,
    /// `timeout` — the wall clock, needed only when `limits.wall_secs` is set.
    pub timeout: Option<PathBuf>,
}

/// Build the `bwrap` argv that runs `payload` under the jail `profile`
/// describes.
///
/// Pure — no process is spawned — so the exact isolation surface is
/// unit-testable, exactly like the build jail's `bwrap_argv`. This does NOT
/// share code with `bwrap_argv`: that builder's non-optional `timeout` prefix is
/// load-bearing for the *build* jail (an untrusted compile must always have a
/// wall clock), whereas the run jail wants no wall clock for a long-lived
/// server. Reusing the *flag vocabulary* is deliberate; sharing the *builder*
/// would couple two different resource-limit policies.
///
/// `scoped_tmp` is the one writable tempdir (used both as the `Isolated`
/// filesystem's sole writable mount and as `TMPDIR`). `working_tree` is bound
/// read-write only under [`FilesystemScope::WorkingTreeReadWrite`].
/// `seccomp_fd` is the file-descriptor number the caller has arranged to carry
/// the compiled seccomp program (passed to `bwrap --seccomp <fd>`); `None` means
/// no filter is attached (the caller must have refused already if a filter was
/// required).
///
/// The env is scrubbed with `--clearenv`; only the fixed minimal allowlist
/// (`PATH`, `TMPDIR`, `LANG`) plus the profile's `env_allowlist` re-enter. There
/// is NO shell token anywhere in the result.
// Every argument is a distinct, load-bearing jail input (tools, profile, the two
// mount roots, the extra binds, the seccomp fd, the env lookup, the payload);
// bundling them into a struct would only move the same fields behind one more
// indirection without reducing the surface. The pure-builder shape is
// deliberately explicit, matching the sibling `bwrap_argv`.
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn run_jail_argv(
    tools: &RunJailTools,
    profile: &SandboxProfile,
    scoped_tmp: &Path,
    working_tree: &Path,
    extra_ro_binds: &[PathBuf],
    seccomp_fd: Option<i32>,
    host_env: &dyn Fn(&str) -> Option<OsString>,
    payload: &[OsString],
) -> Vec<OsString> {
    run_jail_argv_with_delivery(
        tools,
        profile,
        scoped_tmp,
        working_tree,
        extra_ro_binds,
        seccomp_fd,
        None,
        host_env,
        payload,
    )
}

/// [`run_jail_argv`] plus optional in-jail materialisation of the app binary
/// from an inherited descriptor.
///
/// When `app_delivery` is `Some((fd, dest))`, a `--perms 0700 --file <fd>
/// <dest>` pair is emitted AFTER every mount op (so `dest`'s parent tmpfs/bind
/// already exists) and BEFORE the payload separator. bwrap reads the app bytes
/// from the inherited (sealed, non-cloexec) `fd` and writes an owner-execute
/// copy at `dest` inside the sandbox — the delivered bytes are exactly the
/// sealed bytes the caller verified, with no host path lookup to race. The
/// caller then runs `dest` as the payload.
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn run_jail_argv_with_delivery(
    tools: &RunJailTools,
    profile: &SandboxProfile,
    scoped_tmp: &Path,
    working_tree: &Path,
    extra_ro_binds: &[PathBuf],
    seccomp_fd: Option<i32>,
    app_delivery: Option<(i32, &Path)>,
    host_env: &dyn Fn(&str) -> Option<OsString>,
    payload: &[OsString],
) -> Vec<OsString> {
    let mut argv: Vec<OsString> = Vec::new();

    // Optional wall clock (only when the profile sets one AND `timeout` is
    // present; the caller refuses a wall-clock profile with no `timeout`).
    if let (Some(wall), Some(timeout)) = (profile.limits.wall_secs, &tools.timeout) {
        argv.push(timeout.clone().into());
        argv.push("--kill-after=5s".into());
        argv.push(wall.to_string().into());
    }

    argv.push(tools.bwrap.clone().into());

    // The network namespace is unshared UNLESS `network` is granted. IPC/UTS/
    // cgroup are unconditionally unshared (SysV shmem / abstract sockets are
    // covert channels that ride IPC independent of the network axis). PID is
    // unconditionally unshared (no host-PID visibility even when subprocess is
    // granted).
    if !profile.network {
        argv.push("--unshare-net".into());
    }
    for flag in [
        "--unshare-pid",
        "--unshare-uts",
        "--unshare-ipc",
        "--unshare-cgroup",
        "--die-with-parent",
        "--new-session",
        "--clearenv",
    ] {
        argv.push(flag.into());
    }

    // Read-only root, then a FRESH proc mask OVER it. Order matters: the
    // `--ro-bind / /` exposes the host `/proc` (which leaks sibling env via
    // `/proc/<pid>/environ`, defeating `--clearenv`, and is a user-namespace
    // escape lever); the later `--proc /proc` masks it. bwrap applies mount ops
    // in argv order, so proc-after-robind wins.
    argv.push("--ro-bind".into());
    argv.push("/".into());
    argv.push("/".into());
    argv.push("--proc".into());
    argv.push("/proc".into());
    // A fresh minimal devtmpfs (the ro-bound host `/dev` nodes carry no device
    // permissions inside the user namespace).
    argv.push("--dev".into());
    argv.push("/dev".into());
    // Mask the home/tmp trees.
    for tmpfs in ["/home", "/root", "/tmp"] {
        argv.push("--tmpfs".into());
        argv.push(tmpfs.into());
    }

    // Re-expose paths the tmpfs masks would otherwise hide, read-only. The
    // emitted app binary (and, when `filesystem` is absent, so the working tree
    // is NOT already bound, anything it needs at a fixed path) commonly lives
    // under `$HOME` (e.g. a `CARGO_TARGET_DIR` in `~/.cache`), which the
    // `--tmpfs /home` mask hides. Read-only: the payload can execute but never
    // mutate these.
    for dir in extra_ro_binds {
        argv.push("--ro-bind".into());
        argv.push(dir.clone().into());
        argv.push(dir.clone().into());
    }

    // The one writable mount (always), and the working tree read-write only
    // when the filesystem axis is granted.
    argv.push("--bind".into());
    argv.push(scoped_tmp.into());
    argv.push(scoped_tmp.into());
    if profile.filesystem == FilesystemScope::WorkingTreeReadWrite {
        argv.push("--bind".into());
        argv.push(working_tree.into());
        argv.push(working_tree.into());
        argv.push("--chdir".into());
        argv.push(working_tree.into());
    } else {
        argv.push("--chdir".into());
        argv.push(scoped_tmp.into());
    }

    // The seccomp filter (subprocess denial + baseline denials). Attached via a
    // pre-arranged fd. `no_new_privs` is set by bubblewrap by default (it always
    // calls `PR_SET_NO_NEW_PRIVS` unless `--cap-add` is used, which the run jail
    // never does), so the "no privilege gain" claim is mechanical.
    if let Some(fd) = seccomp_fd {
        argv.push("--seccomp".into());
        argv.push(fd.to_string().into());
    }

    // Scrubbed env: the fixed minimal allowlist, then the profile's declared
    // env names re-exported from the host (only when `env` was granted). A named
    // var absent from the host is simply not re-exported (never a placeholder).
    argv.push("--setenv".into());
    argv.push("PATH".into());
    argv.push("/usr/bin:/bin".into());
    argv.push("--setenv".into());
    argv.push("TMPDIR".into());
    argv.push(scoped_tmp.into());
    if let Some(lang) = host_env("LANG") {
        argv.push("--setenv".into());
        argv.push("LANG".into());
        argv.push(lang);
    }
    for name in &profile.env_allowlist {
        if let Some(value) = host_env(name) {
            argv.push("--setenv".into());
            argv.push(name.into());
            argv.push(value);
        }
    }

    // Materialise the app inside the jail from the inherited sealed descriptor,
    // AFTER all mounts (so the destination's parent exists) and BEFORE the
    // payload. `--perms 0700` applies to the `--file` copy that follows it,
    // making the delivered binary owner-executable. bwrap reads the bytes from
    // `fd` (inherited, non-cloexec, sealed) — the delivered file cannot differ
    // from the verified bytes.
    if let Some((fd, dest)) = app_delivery {
        argv.push("--perms".into());
        argv.push("0700".into());
        argv.push("--file".into());
        argv.push(fd.to_string().into());
        argv.push(dest.into());
    }

    // Resource caps via prlimit, then the payload with NO shell. The wall clock
    // (if any) is the outer `timeout`; prlimit bounds AS/CPU/FDs/procs.
    argv.push("--".into());
    argv.push(tools.prlimit.clone().into());
    argv.push(format!("--as={}", profile.limits.as_bytes).into());
    argv.push(format!("--cpu={}", profile.limits.cpu_secs).into());
    argv.push(format!("--nofile={}", profile.limits.fd_cap).into());
    argv.push(format!("--nproc={}", profile.limits.proc_cap).into());
    argv.push("--".into());
    argv.extend(payload.iter().cloned());
    argv
}

// ── refusal + the fail-closed platform decision ─────────────────────────────

/// Why the run jail could not be established, or refused to run. The whole
/// family carries the [`IPE_F4413`] taxonomy code, the run-jail sibling of the
/// build jail's [`crate::SandboxDefect`] / `IPE-F4410`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunJailDefect {
    /// A required jail primitive is absent on this host (`bwrap`/`prlimit`, or a
    /// wall-clock profile with no `timeout`). Fail-closed: refuse, never run
    /// unconfined.
    PrimitiveUnavailable {
        /// The missing primitive name(s).
        missing: Vec<&'static str>,
    },
    /// No sound jail can be built on this platform (not Linux, or seccomp cannot
    /// be compiled for this architecture). The documented refuse-gap: a
    /// non-empty native/high-value set refuses to run here.
    UnsupportedPlatform {
        /// A short reason for the diagnostic.
        reason: &'static str,
    },
    /// The capability profile could not be built (see [`ProfileError`]).
    Profile(ProfileError),
    /// The jailed process could not be spawned.
    Spawn {
        /// The rendered OS error.
        detail: String,
    },
    /// A jail-root mount (or unmount) could not be established while building the
    /// confinement — the read-only root, a read-write scratch/working-tree mount,
    /// the fresh devfs, or the masked `/proc`. Distinct from [`Self::Spawn`] so a
    /// failure to *build* the jail root is never conflated with a failure to
    /// *launch* the payload: a half-built root refuses before any process runs.
    /// Fail-closed — the untrusted payload never runs against an
    /// incompletely-mounted root.
    MountFailed {
        /// The mount target that could not be established.
        target: PathBuf,
        /// The rendered OS error or non-success detail for the mount attempt.
        detail: String,
    },
    /// A tampered `ipe.profile` requested *less* isolation than the capability
    /// floor embedded in the binary. Refuse — a weaker profile cannot widen the
    /// jail below what the binary was built for.
    ProfileWeakerThanFloor,
}

impl RunJailDefect {
    /// The stable taxonomy code (`IPE-F4413` for the whole family).
    #[must_use]
    pub const fn code(&self) -> Code {
        IPE_F4413
    }
}

impl From<RunJailDefect> for SandboxError {
    fn from(d: RunJailDefect) -> Self {
        let detail = match &d {
            RunJailDefect::PrimitiveUnavailable { missing } => format!(
                "cannot establish a runtime jail around the app — missing {} — refusing to run \
                 a capability-bearing program unconfined; install bubblewrap (bwrap) and \
                 util-linux (prlimit)",
                missing.join(", ")
            ),
            RunJailDefect::UnsupportedPlatform { reason } => format!(
                "no runtime jail can be built on this platform ({reason}); refusing to run a \
                 native-capability program unconfined"
            ),
            RunJailDefect::Profile(e) => e.to_string(),
            RunJailDefect::Spawn { detail } => {
                format!("failed to spawn the jailed app: {detail}")
            }
            RunJailDefect::MountFailed { target, detail } => format!(
                "could not establish the jail-root mount at {} ({detail}); refusing to run the \
                 untrusted payload against an incompletely-mounted root",
                target.display()
            ),
            RunJailDefect::ProfileWeakerThanFloor => {
                "the artifact's ipe.profile requests less isolation than the capability floor \
                 embedded in the binary — refusing to run under a weakened profile"
                    .to_owned()
            }
        };
        Self::RunJail { detail }
    }
}

impl From<RunJailDefect> for SharedDiag {
    fn from(d: RunJailDefect) -> Self {
        Self::Sandbox {
            msg: SandboxError::from(d),
        }
    }
}

impl std::fmt::Display for RunJailDefect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let shared: SharedDiag = self.clone().into();
        f.write_str(&ipe_diagnostics::render(&shared, "", ""))
    }
}

impl std::error::Error for RunJailDefect {}

/// The build-time platform verdict: can a sound run jail be built on THIS
/// target at all?
///
/// Linux `x86_64`/`aarch64` (the `bwrap`+seccomp jail), macOS (the `sandbox-exec` SBPL
/// jail), and Windows (the Job Object + `AppContainer` + launcher-scrub jail) →
/// yes. Everything else is the documented refuse-gap.
///
/// "Yes" means a jail ARM is compiled in, not that every axis is confined:
/// Windows is a jailed target with a PARTIAL confined set (see
/// [`platform_confined_axes`]). This is a `const` reflection of the compile
/// target, independent of host tool availability (which [`RunJailTools`] /
/// `sandbox-exec` probing covers). Both this constant and the real
/// [`exec_in_run_jail`] arm are stamped by the SAME [`on_jailed_target`] macro,
/// so the verdict is `true` EXACTLY on the targets a jail is compiled for. There
/// is no second hand-kept copy to drift: the FFI admit path can never claim a
/// jail that is not compiled for the target, and a target with only the stub
/// `exec_in_run_jail` reports "refuse".
#[must_use]
pub const fn platform_supports_jail() -> bool {
    JAIL_COMPILED_IN
}

on_jailed_target! {
    yes: {
        /// True on the targets [`exec_in_run_jail`] has a real (non-stub) arm.
        /// Stamped by [`on_jailed_target`] so it cannot disagree with the arm.
        const JAIL_COMPILED_IN: bool = true;
    }
    no: {
        /// False on the targets [`exec_in_run_jail`] is only the refuse stub.
        const JAIL_COMPILED_IN: bool = false;
    }
}

/// The runtime-enforced axes the compiled-in [`exec_in_run_jail`] arm confines.
///
/// This is the single source the FFI admit path keys off, so it can never claim
/// an axis the jail does not enforce on this target.
///
/// The set is per-OS `#[cfg]` (NOT stamped by [`on_jailed_target`], because a
/// jailed target need not confine every axis):
///
/// - **Linux** (`bwrap`+seccomp) and **macOS** (`sandbox-exec`+launcher-scrub)
///   confine the FULL set — network + filesystem (net namespace / SBPL deny;
///   `--ro-bind`+tmpfs / SBPL deny-write), subprocess (seccomp / SBPL
///   process-deny), env (`--clearenv` / launcher scrub) — and native-ffi is
///   contained by the whole-process jail regardless of what native code does.
/// - **Windows** is PARTIAL: the Job Object confines subprocess and the launcher
///   scrub confines env unconditionally; filesystem and network are confined
///   only under `AppContainer` on an ACL volume (see the design doc's refuse-gap
///   policy). The compiled-in Windows arm establishes `AppContainer` + an ACL
///   scratch, so it lists filesystem and network too — the restricted-token /
///   non-ACL-volume refuse-gaps are runtime conditions the arm fails-closed on,
///   not a compile-time axis removal.
/// - Off every jailed target the stub arm confines NOTHING (the fail-closed
///   empty set), so a capability-bearing wrapper is refused, never run
///   unconfined.
///
/// **`database` is DERIVED, never a standalone asserted bit.** `database` lowers
/// to `network` (a TCP driver) or `filesystem` (a file driver) before it reaches
/// the jail; which one is a per-project runtime fact unknown here. So the honest
/// platform predicate is "database is confined iff BOTH the axes it can lower
/// into are confined" — [`database_confined`] over this target's net + fs
/// membership. On a FULL target (Linux/macOS) net and fs are both confined, so
/// database is confined exactly as before. On a PARTIAL target that confined, say,
/// filesystem but not network, database would be OMITTED — a file-backed database
/// would still be admitted through its `filesystem` lowering, but the standalone
/// `database` claim would over-promise for a TCP driver, so it is not made. This
/// is guardian Nit-1 from the per-axis review.
///
/// The list is `Capability` values so the FFI admit path folds them straight
/// into its confined-axis set; the ordering is irrelevant (folded into a set).
#[must_use]
pub const fn platform_confined_axes() -> &'static [Capability] {
    CONFINED_AXES
}

/// Whether the `database` axis is confined, DERIVED from whether both axes it can
/// lower into — `network` (TCP driver) and `filesystem` (file driver) — are
/// confined on this target.
///
/// `database` carries no OS control of its own; it is confined iff EVERY axis it
/// could lower into is confined, so that neither a TCP nor a file driver escapes.
/// This is the single source that keeps `database` from being over-claimed on a
/// partial target (guardian Nit-1): it is never asserted directly in
/// [`CONFINED_AXES`] — it appears there only when this derivation holds.
#[must_use]
pub const fn database_confined(network_confined: bool, filesystem_confined: bool) -> bool {
    network_confined && filesystem_confined
}

/// The `FILE_PERSISTENT_ACLS` filesystem-capability bit as reported by
/// `GetVolumeInformationW`'s `lpFileSystemFlags`. A volume that clears this bit
/// (FAT/exFAT, some redirected `%TEMP%` and network shares) neither persists nor
/// enforces DACLs: `SetNamedSecurityInfoW` returns `ERROR_SUCCESS` while
/// persisting/enforcing nothing.
///
/// It is written here as the raw Win32 value (kept in lockstep with
/// `windows_sys::Win32::System::SystemServices::FILE_PERSISTENT_ACLS`) so the
/// [`volume_flags_confine_filesystem`] decision is a pure, cross-platform
/// function unit-testable on any host, not gated behind `cfg(windows)`.
///
/// Consumed by the Windows arm (the `const _` lockstep assertion) and by the
/// cross-platform unit tests; on a non-Windows non-test build it has no caller,
/// hence the scoped `dead_code` allow.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub(crate) const FILE_PERSISTENT_ACLS_FLAG: u32 = 0x0000_0008;

/// The typed "parse the volume capability" decision: given the filesystem flags
/// `GetVolumeInformationW` reports for a volume, is the ACL boundary the Windows
/// run-jail arm relies on actually enforceable there?
///
/// The Windows arm confines `filesystem` (and, via [`database_confined`],
/// `database`) by `ACLing` the scratch/working-tree DACL to the container SID.
/// That boundary is a NO-OP on a volume without `FILE_PERSISTENT_ACLS`, where
/// `SetNamedSecurityInfoW` succeeds without persisting or enforcing anything. So
/// the ACL claim is honest only when this bit is present.
///
/// `true` ⇒ the volume persists+enforces DACLs, so the arm may proceed to ACL and
/// launch. `false` ⇒ the arm must fail closed (never launch on a volume where the
/// ACL boundary the admit path already trusted is a no-op). This is the parse
/// step: probe once → a typed proceed/refuse decision, not an inference from a
/// success return that does not mean what the caller assumed.
#[must_use]
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub(crate) const fn volume_flags_confine_filesystem(filesystem_flags: u32) -> bool {
    filesystem_flags & FILE_PERSISTENT_ACLS_FLAG != 0
}

/// Linux/macOS confine the full set. `database` is present because
/// [`database_confined`] holds (both net and fs are confined); the
/// `database_membership_is_derived_from_net_and_fs` test proves the list matches
/// the derivation rather than asserting `database` standalone.
#[cfg(any(
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ),
    target_os = "macos"
))]
const CONFINED_AXES: &[Capability] = &[
    Capability::Network,
    Capability::Filesystem,
    Capability::Database,
    Capability::Env,
    Capability::Subprocess,
    Capability::NativeFfi,
];

/// Windows confines subprocess + env unconditionally, and filesystem + network
/// under the `AppContainer` + ACL-scratch arm the launcher establishes.
/// `database` is present because [`database_confined`] holds over Windows's net +
/// fs membership (both are in the list). Were a future Windows configuration to
/// drop `network` or `filesystem`, `database` would have to leave too — the
/// `database_membership_is_derived_from_net_and_fs` test enforces exactly that,
/// so the derivation cannot silently over-claim. native-ffi is contained by the
/// whole-process Job Object + token, so it is listed.
#[cfg(target_os = "windows")]
const CONFINED_AXES: &[Capability] = &[
    Capability::Network,
    Capability::Filesystem,
    Capability::Database,
    Capability::Env,
    Capability::Subprocess,
    Capability::NativeFfi,
];

/// The stub arm confines nothing — the fail-closed empty set.
#[cfg(not(any(
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ),
    target_os = "macos",
    target_os = "windows"
)))]
const CONFINED_AXES: &[Capability] = &[];

/// Probe the host for the run-jail primitives and decide whether a jail can be
/// built, returning the tools or the fail-closed refusal.
///
/// The required primitives are per-OS, so this function is cfg-split to match the
/// same platforms [`exec_in_run_jail`] confines: on Linux (`x86_64`/`aarch64`) it requires
/// `bwrap` + `prlimit` (+ `timeout` when the profile sets a wall clock); on macOS
/// it requires `sandbox-exec`. Off both it is the refuse-gap. `wants_wall_clock`
/// selects whether `timeout` is additionally required (Linux only).
///
/// # Errors
///
/// [`RunJailDefect::UnsupportedPlatform`] off every jailed target;
/// [`RunJailDefect::PrimitiveUnavailable`] when a required primitive is absent.
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
pub fn probe_run_jail_tools(wants_wall_clock: bool) -> Result<RunJailTools, RunJailDefect> {
    let caps = crate::probe();
    let mut missing: Vec<&'static str> = Vec::new();
    if caps.bwrap.is_none() {
        missing.push("bwrap");
    }
    if caps.prlimit.is_none() {
        missing.push("prlimit");
    }
    if wants_wall_clock && caps.timeout.is_none() {
        missing.push("timeout");
    }
    if !missing.is_empty() {
        return Err(RunJailDefect::PrimitiveUnavailable { missing });
    }
    // The probes above guarantee these are `Some`.
    let (Some(bwrap), Some(prlimit)) = (caps.bwrap, caps.prlimit) else {
        return Err(RunJailDefect::PrimitiveUnavailable {
            missing: vec!["bwrap", "prlimit"],
        });
    };
    Ok(RunJailTools {
        bwrap,
        prlimit,
        timeout: caps.timeout,
    })
}

/// macOS: probe for `sandbox-exec`, the run jail's only primitive.
///
/// The `bwrap`/`prlimit` tools the Linux jail needs do not apply. The macOS
/// [`exec_in_run_jail`] arm re-resolves `sandbox-exec` itself and does NOT read
/// the returned tools, so the returned [`RunJailTools`] is an inert "primitive
/// present" token whose fields all hold the resolved `sandbox-exec` path (never
/// used for a Linux tool invocation on macOS). Fail-closed: an absent
/// `sandbox-exec` refuses.
///
/// `_wants_wall_clock` is unused on macOS: the SBPL run jail carries no external
/// wall-clock helper.
///
/// # Errors
///
/// [`RunJailDefect::PrimitiveUnavailable`] when `sandbox-exec` is absent.
#[cfg(target_os = "macos")]
pub fn probe_run_jail_tools(_wants_wall_clock: bool) -> Result<RunJailTools, RunJailDefect> {
    let Some(sandbox_exec) = crate::build_jail::find_in_path("sandbox-exec") else {
        return Err(RunJailDefect::PrimitiveUnavailable {
            missing: vec!["sandbox-exec"],
        });
    };
    Ok(RunJailTools {
        bwrap: sandbox_exec.clone(),
        prlimit: sandbox_exec.clone(),
        timeout: Some(sandbox_exec),
    })
}

/// Windows: the run jail's primitives are Win32 kernel objects (a Job Object, an
/// AppContainer lowbox token), built directly through the Windows API rather than
/// by exec'ing an external tool. There is no `bwrap`/`prlimit`/`sandbox-exec`
/// binary to locate, so this returns an inert "primitives are the kernel API"
/// token whose paths are never used for a tool invocation; the real
/// constructibility check — and the fail-closed refusal when a primitive cannot
/// be created — happens inside [`exec_in_run_jail`] at launch, where the objects
/// are actually built.
///
/// `_wants_wall_clock` is unused: the Windows jail carries no external wall-clock
/// helper (a wall clock, if ever added, would be a Job Object time limit, not a
/// separate tool).
///
/// # Errors
///
/// Never on Windows — this probe cannot fail because it locates no external tool;
/// the primitive construction that CAN fail is in [`exec_in_run_jail`], which
/// refuses (never runs unconfined) on any failure.
#[cfg(target_os = "windows")]
#[allow(
    clippy::missing_const_for_fn,
    clippy::unnecessary_wraps,
    clippy::doc_markdown,
    clippy::too_long_first_doc_paragraph
)]
pub fn probe_run_jail_tools(_wants_wall_clock: bool) -> Result<RunJailTools, RunJailDefect> {
    // An inert token: the fields are the same placeholder path, never invoked as a
    // Unix tool. The Windows arm builds its kernel objects directly and does not
    // read these.
    let placeholder = PathBuf::from("windows-run-jail-native");
    Ok(RunJailTools {
        bwrap: placeholder.clone(),
        prlimit: placeholder.clone(),
        timeout: Some(placeholder),
    })
}

/// Off every jailed target the run jail is a documented refuse-gap: no primitive
/// this crate builds would confine the app here.
///
/// # Errors
///
/// Always [`RunJailDefect::UnsupportedPlatform`].
#[cfg(not(any(
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ),
    target_os = "macos",
    target_os = "windows"
)))]
#[allow(clippy::missing_const_for_fn)]
pub fn probe_run_jail_tools(_wants_wall_clock: bool) -> Result<RunJailTools, RunJailDefect> {
    Err(RunJailDefect::UnsupportedPlatform {
        reason: "runtime jail is compiled only for Linux (x86_64/aarch64), macOS, and Windows",
    })
}

/// Run the emitted `app` binary inside the run jail described by `profile`,
/// replacing the current process on success (Unix `exec`).
///
/// This compiles the seccomp program for the profile's subprocess axis, places
/// it on an inheritable file descriptor, builds the `bwrap` argv referencing
/// that fd, and `exec`s it. The seccomp fd is deliberately left WITHOUT the
/// close-on-exec flag so `bwrap` inherits it; every other fd stays cloexec.
///
/// On a non-Linux target this is a compile-time refusal shape — the whole body
/// is `cfg(target_os = "linux")`; other targets return
/// [`RunJailDefect::UnsupportedPlatform`].
///
/// # Errors
///
/// Any [`RunJailDefect`]; on success (Linux) it does not return.
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
pub fn exec_in_run_jail(
    tools: &RunJailTools,
    profile: &SandboxProfile,
    scoped_tmp: &Path,
    working_tree: &Path,
    app: &Path,
    app_args: &[OsString],
) -> Result<std::convert::Infallible, RunJailDefect> {
    use std::os::unix::process::CommandExt as _;

    // Compile the seccomp program for this profile. `None` ⇒ this architecture
    // has no filter we can emit — refuse (fail-closed), never run unfiltered.
    let Some(program) = seccomp::subprocess_deny_program(profile.subprocess) else {
        return Err(RunJailDefect::UnsupportedPlatform {
            reason: "no seccomp filter can be compiled for this architecture",
        });
    };
    let bytes = seccomp::program_bytes(&program);
    let seccomp_fd = write_seccomp_memfd(&bytes)?;

    let mut payload: Vec<OsString> = Vec::with_capacity(app_args.len() + 1);
    payload.push(app.as_os_str().to_owned());
    payload.extend(app_args.iter().cloned());

    // Re-expose the app binary's directory read-only past the home/tmp tmpfs
    // masks (a `CARGO_TARGET_DIR` under `~/.cache` is otherwise hidden). The
    // binary's *parent* is bound so a relocated dynamic loader path or a
    // co-located artifact resolves; the read-only bind means the app can exec
    // it but never mutate it.
    let mut extra_ro_binds: Vec<PathBuf> = Vec::new();
    if let Some(app_dir) = app.parent() {
        extra_ro_binds.push(app_dir.to_path_buf());
    }

    let host_env = |k: &str| std::env::var_os(k);
    let argv = run_jail_argv(
        tools,
        profile,
        scoped_tmp,
        working_tree,
        &extra_ro_binds,
        Some(seccomp_fd),
        &host_env,
        &payload,
    );

    let (program_path, rest) = argv.split_first().ok_or_else(|| RunJailDefect::Spawn {
        detail: "empty jail argv".to_owned(),
    })?;
    let mut cmd = std::process::Command::new(program_path);
    cmd.args(rest);
    // The seccomp fd MUST survive exec so bwrap can read the program from it;
    // clear its close-on-exec flag right before exec via a pre_exec hook.
    let fd = seccomp_fd;
    // SAFETY: `pre_exec` runs in the child between fork and exec. `fcntl` with
    // `F_SETFD`/`0` is async-signal-safe and touches only this process's fd
    // table; no allocation, no lock. A failure returns an error that aborts the
    // exec, so a jail that could not un-cloexec its filter fd refuses rather
    // than running the app without the filter.
    unsafe {
        cmd.pre_exec(move || {
            let flags = libc_fcntl_getfd(fd)?;
            let cleared = flags & !FD_CLOEXEC;
            libc_fcntl_setfd(fd, cleared)?;
            Ok(())
        });
    }
    let err = cmd.exec();
    Err(RunJailDefect::Spawn {
        detail: err.to_string(),
    })
}

/// Exec an embedded app held only in a sealed anonymous descriptor.
///
/// Delivering the app into the jail from that descriptor rather than from a host
/// path closes the verify/exec identity gap for `ipe-wrapper` embed mode.
///
/// The wrapper writes the embedded bytes to a sealed memfd
/// ([`write_sealed_app_memfd`]), verifies the capability floor by reading the
/// SEALED fd, then calls this. bwrap inherits the (non-cloexec, sealed) fd
/// across the process replacement and materialises the app inside the jail via
/// `--file` at a fixed sandbox path — so the bytes executed are provably the
/// sealed bytes that were verified; a same-uid attacker has no host path to
/// pre-seed or swap.
///
/// # Errors
///
/// Any [`RunJailDefect`]; on success (Linux) it does not return.
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
pub fn exec_embedded_in_run_jail(
    tools: &RunJailTools,
    profile: &SandboxProfile,
    scoped_tmp: &Path,
    working_tree: &Path,
    app: &SealedApp,
    app_args: &[OsString],
) -> Result<std::convert::Infallible, RunJailDefect> {
    use std::os::unix::process::CommandExt as _;

    let Some(program) = seccomp::subprocess_deny_program(profile.subprocess) else {
        return Err(RunJailDefect::UnsupportedPlatform {
            reason: "no seccomp filter can be compiled for this architecture",
        });
    };
    let bytes = seccomp::program_bytes(&program);
    let seccomp_fd = write_seccomp_memfd(&bytes)?;
    let app_fd = app.as_raw_fd();

    // The in-jail path the app is materialised at. It sits under `scoped_tmp`,
    // the one always-writable bind, so bwrap can create it after the mounts.
    let dest = scoped_tmp.join("ipe-app");

    let mut payload: Vec<OsString> = Vec::with_capacity(app_args.len() + 1);
    payload.push(dest.as_os_str().to_owned());
    payload.extend(app_args.iter().cloned());

    let host_env = |k: &str| std::env::var_os(k);
    let argv = run_jail_argv_with_delivery(
        tools,
        profile,
        scoped_tmp,
        working_tree,
        &[],
        Some(seccomp_fd),
        Some((app_fd, &dest)),
        &host_env,
        &payload,
    );

    let (program_path, rest) = argv.split_first().ok_or_else(|| RunJailDefect::Spawn {
        detail: "empty jail argv".to_owned(),
    })?;
    let mut cmd = std::process::Command::new(program_path);
    cmd.args(rest);
    // Both the seccomp filter fd and the sealed app fd MUST survive the exec so
    // bwrap can read them; clear their close-on-exec flags right before exec.
    let seccomp_fd_move = seccomp_fd;
    let app_fd_move = app_fd;
    // SAFETY: `pre_exec` runs in the child between fork and exec (here it is the
    // process-replacing `exec`, so there is no fork — the closure runs in this
    // process just before execve). `clear_cloexec` performs only
    // async-signal-safe `fcntl` calls on owned fds; a failure aborts the exec,
    // so a jail that could not un-cloexec a required fd refuses rather than
    // running the app without its filter or without a delivered binary.
    unsafe {
        cmd.pre_exec(move || {
            clear_cloexec(seccomp_fd_move)?;
            clear_cloexec(app_fd_move)?;
            Ok(())
        });
    }
    let err = cmd.exec();
    Err(RunJailDefect::Spawn {
        detail: err.to_string(),
    })
}

/// Run the emitted `app` binary inside the macOS `sandbox-exec` Seatbelt jail
/// described by `profile`, replacing the current process on success (Unix
/// `exec`).
///
/// The macOS counterpart to the `Linux` [`exec_in_run_jail`]: it lowers
/// the SAME [`SandboxProfile`] to a Seatbelt SBPL profile via the SAME
/// [`crate::build_jail::sbpl_from_profile`] the Tier-2 `build_in_jail` uses —
/// there is ONE SBPL generator, so what confines a Tier-2 build and what confines
/// the shipped app at run time cannot drift. It writes the profile into the
/// always-writable scratch and `exec`s `sandbox-exec -f <profile> <app> <args>`.
/// There is NO shell token anywhere — the payload is a direct argv, so the
/// quoting/injection class does not exist.
///
/// The SBPL enforces the network, filesystem, and subprocess axes; the `env` axis
/// is enforced HERE in the launcher (Seatbelt cannot scrub env): the environment
/// is cleared and only the profile's allowlisted names re-exported, via the SAME
/// [`crate::build_jail::macos_scrubbed_env`] the build jail and the e2e use — so
/// all four runtime-enforced axes are contained, matching the Linux jail, and the
/// FFI admit path's `Holds` verdict is honest on macOS.
///
/// `scoped_tmp` is the one always-writable scratch (also where the SBPL profile
/// is written); `working_tree` is writable only when the profile grants the
/// filesystem axis. Fail-closed: an absent `sandbox-exec`, an unwritable profile,
/// or a failed `exec` REFUSES — the capability-bearing app is never run
/// unconfined.
///
/// The jail's actual deny behaviour is proven by the `macos-run-jail` CI job on a
/// real macOS runner; this crate builds the arm and unit-tests the pure SBPL
/// lowering on any host, but only a macOS runner exercises `sandbox-exec`.
///
/// # Errors
///
/// Any [`RunJailDefect`]; on success it does not return.
#[cfg(target_os = "macos")]
pub fn exec_in_run_jail(
    _tools: &RunJailTools,
    profile: &SandboxProfile,
    scoped_tmp: &Path,
    working_tree: &Path,
    app: &Path,
    app_args: &[OsString],
) -> Result<std::convert::Infallible, RunJailDefect> {
    use std::os::unix::process::CommandExt as _;

    // `sandbox-exec` is the mandatory macOS jail primitive. Absent ⇒ refuse; the
    // capability-bearing app is never run unconfined.
    let Some(sandbox_exec) = crate::build_jail::find_in_path("sandbox-exec") else {
        return Err(RunJailDefect::PrimitiveUnavailable {
            missing: vec!["sandbox-exec"],
        });
    };

    // Lower the SAME profile through the SAME SBPL generator the build jail uses,
    // and write it into the always-writable scratch (never a shared temp path
    // that could race or persist).
    let sbpl = crate::build_jail::sbpl_from_profile(profile, scoped_tmp, working_tree);
    let profile_file = scoped_tmp.join("ipe-run.sb");
    if let Err(e) = std::fs::write(&profile_file, sbpl.as_bytes()) {
        return Err(RunJailDefect::Spawn {
            detail: format!("could not write the SBPL run-jail profile: {e}"),
        });
    }

    // argv: sandbox-exec -f <profile> <app> <app_args…>. Direct argv, no shell.
    let mut cmd = std::process::Command::new(&sandbox_exec);
    cmd.arg("-f").arg(&profile_file).arg(app).args(app_args);
    // Enforce the `env` axis in the launcher (Seatbelt cannot scrub env): clear
    // the inherited environment and re-export ONLY the scrubbed base plus the
    // profile's allowlisted names, mirroring the Linux jail's `--clearenv`. The
    // scrub is the SAME `macos_scrubbed_env` the e2e proves, so what confines the
    // env at run time and what the test asserts cannot drift.
    let host_env = |k: &str| std::env::var_os(k);
    cmd.env_clear();
    for (name, value) in crate::build_jail::macos_scrubbed_env(profile, scoped_tmp, &host_env) {
        cmd.env(name, value);
    }
    let err = cmd.exec();
    // `exec` only returns on failure; the scratch profile is inert either way.
    Err(RunJailDefect::Spawn {
        detail: err.to_string(),
    })
}

/// A verified embedded app binary held in memory.
///
/// macOS has no `memfd_create` and `sandbox-exec` cannot exec an inherited
/// descriptor, so the sealed-fd delivery the Linux arm uses is unavailable. The
/// bytes are held here after the single verification read; the exec arm writes
/// them ONCE to an exclusively-created (`O_EXCL`, mode 0700, unpredictably
/// named) file inside `scoped_tmp` and execs that. `scoped_tmp` is itself an
/// exclusively-created, unpredictable, owner-only directory, so no other path
/// exists to pre-seed or swap between the write and the exec.
#[cfg(target_os = "macos")]
pub struct SealedApp {
    bytes: Vec<u8>,
}

#[cfg(target_os = "macos")]
impl SealedApp {
    /// Read the app bytes for the capability-floor verification scan.
    #[must_use]
    pub fn read_sealed_bytes(&self) -> Result<Vec<u8>, RunJailDefect> {
        Ok(self.bytes.clone())
    }
}

/// Hold `bytes` for a later exclusive write-then-exec inside the jail scratch.
///
/// The Linux counterpart seals a memfd; macOS keeps the verified bytes in
/// memory because it has no sealing memfd and cannot fd-exec through
/// `sandbox-exec`.
///
/// # Errors
///
/// Infallible in practice; returns `Result` for arm-parity with the Linux
/// [`write_sealed_app_memfd`].
#[cfg(target_os = "macos")]
pub fn write_sealed_app_memfd(bytes: &[u8]) -> Result<SealedApp, RunJailDefect> {
    Ok(SealedApp {
        bytes: bytes.to_vec(),
    })
}

/// macOS embed-mode exec: write the verified bytes to an exclusively-created
/// file inside the exclusive `scoped_tmp` scratch and exec it under
/// `sandbox-exec`.
///
/// `sandbox-exec` cannot exec an inherited descriptor, so the same-inode
/// guarantee is delivered structurally instead: `scoped_tmp` is an
/// exclusively-created, unpredictable, owner-only directory, and the app file is
/// created with `O_EXCL` under a random name, so between the write and the exec
/// there is no predictable path an attacker can pre-seed or swap.
///
/// # Errors
///
/// Any [`RunJailDefect`]; on success it does not return.
#[cfg(target_os = "macos")]
pub fn exec_embedded_in_run_jail(
    tools: &RunJailTools,
    profile: &SandboxProfile,
    scoped_tmp: &Path,
    working_tree: &Path,
    app: &SealedApp,
    app_args: &[OsString],
) -> Result<std::convert::Infallible, RunJailDefect> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;

    // Exclusive-create the app file under the exclusive scratch dir. A random
    // name plus `create_new` (O_EXCL) means a pre-seeded entry fails rather than
    // being followed.
    let mut entropy = [0u8; 16];
    if std::fs::File::open("/dev/urandom")
        .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut entropy))
        .is_err()
    {
        return Err(RunJailDefect::Spawn {
            detail: "could not read OS entropy for the embedded app path".to_owned(),
        });
    }
    let mut hex = String::with_capacity(32);
    for b in entropy {
        use std::fmt::Write as _;
        let _ = write!(hex, "{b:02x}");
    }
    let app_path = scoped_tmp.join(format!("ipe-app-{hex}"));
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(&app_path)
    {
        Ok(f) => f,
        Err(e) => {
            return Err(RunJailDefect::Spawn {
                detail: format!("could not create the embedded app file: {e}"),
            });
        }
    };
    if let Err(e) = file.write_all(&app.bytes) {
        return Err(RunJailDefect::Spawn {
            detail: format!("could not write the embedded app file: {e}"),
        });
    }
    drop(file);

    exec_in_run_jail(
        tools,
        profile,
        scoped_tmp,
        working_tree,
        &app_path,
        app_args,
    )
}

/// Run the emitted `app` binary inside the Windows run jail described by
/// `profile` — a Job Object (subprocess axis) around an AppContainer-tokened
/// child (filesystem + network axes) whose environment the launcher scrubs (env
/// axis), assembled per the Windows run-jail design.
///
/// Unlike the Unix arms there is no `exec`-replace on Windows: the launcher stays
/// alive as the Job Object owner (so `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` holds
/// for the app's whole lifetime) and propagates the child's exit code. The launch
/// is create-suspended → assign-to-job → resume, so the app never runs an
/// instruction outside the job.
///
/// Per-axis confinement (see [`platform_confined_axes`]):
/// - **subprocess** — the Job Object caps the active-process count (1 when the
///   axis is withheld, so no child can spawn) and denies breakaway, so no process
///   escapes the container.
/// - **env** — the child's environment block is built from
///   [`windows_scrubbed_env`] and passed explicitly to `CreateProcess`; the
///   launcher's environment is never inherited.
/// - **filesystem + network** — the child runs under an AppContainer lowbox token
///   (deny-by-default): the network capability SID (`internetClient`) is granted
///   only when the profile grants network, and the scratch (plus the working tree
///   when the filesystem axis is granted) is ACLed to the container SID. On a
///   non-ACL volume or where AppContainer cannot be established the arm FAILS
///   CLOSED (refuses) rather than run with an unenforced boundary.
///
/// Fail-closed at every step: a Job Object, token, capability SID, ACL, or
/// `CreateProcess` that cannot be built REFUSES — the capability-bearing app is
/// never run unconfined. Every Win32 handle (job, process, thread, token) is
/// RAII-closed by an owned wrapper, and every allocated SID / attribute list is
/// freed on every path, so a failure cannot leak a handle or leave the app
/// running outside the jail.
///
/// The jail's actual deny behaviour is proven by the `windows-run-jail` CI job on
/// a real `windows-2022` runner (a hosted runner, no Docker daemon needed);
/// this crate builds the arm and unit-tests the pure pieces (the env scrub, the
/// UTF-16 block) on any host.
///
/// # Errors
///
/// Any [`RunJailDefect`]. On success the launcher waits for the jailed child and
/// this returns via [`RunJailDefect::Spawn`] carrying the propagated exit — it is
/// the one arm that returns on SUCCESS (Windows has no `exec`-replace), so the
/// caller reads the exit code from the returned defect's detail. (The signature
/// keeps `Infallible` for arm-parity with the Unix arms; the launcher process
/// exits with the child's code before returning in the success path.)
#[cfg(target_os = "windows")]
#[allow(clippy::doc_markdown, clippy::too_long_first_doc_paragraph)]
pub fn exec_in_run_jail(
    _tools: &RunJailTools,
    profile: &SandboxProfile,
    scoped_tmp: &Path,
    working_tree: &Path,
    app: &Path,
    app_args: &[OsString],
) -> Result<std::convert::Infallible, RunJailDefect> {
    windows_jail::launch(profile, scoped_tmp, working_tree, app, app_args)
}

/// Test-only seam: run `app` under the SAME Windows run jail
/// [`exec_in_run_jail`] uses, but RETURN the child's exit code instead of
/// exiting the process. The `windows-run-jail` CI E2E drives the enforce-vs-
/// control duality through this so it can assert on the jailed vs unjailed exit
/// codes without the launcher replacing the test process. It is exactly the
/// production `run_confined` sequence (one jail source, no fork).
///
/// # Errors
///
/// Any [`RunJailDefect`] — a jail that could not be established refuses here just
/// as the production launcher does.
#[cfg(target_os = "windows")]
#[doc(hidden)]
pub fn run_windows_jailed_for_test(
    profile: &SandboxProfile,
    scoped_tmp: &Path,
    working_tree: &Path,
    app: &Path,
    app_args: &[OsString],
) -> Result<u32, RunJailDefect> {
    windows_jail::run_confined(profile, scoped_tmp, working_tree, app, app_args)
}

/// Run `payload` under the SAME Windows jail sequence [`exec_in_run_jail`] uses —
/// a Job Object (subprocess) around an AppContainer-tokened, env-scrubbed child
/// (filesystem + network + env) — and RETURN the child's exit code, so the
/// returning Tier-2 build jail ([`crate::build_jail::build_in_jail`]) can decode
/// it into a [`crate::build_jail::JailOutcome`] rather than replacing the process.
///
/// `payload[0]` is the program and the rest its arguments (the same
/// `&[OsString]` shape the Linux/macOS build-jail arms take). The confinement is
/// the production [`windows_jail::run_confined`] sequence — one jail source, no
/// fork — so a build observed under Tier-2 is confined exactly as the shipped
/// artifact is at run time. Every kernel object (job / token / SID / attribute
/// list) is RAII-released on every path, so the audit's per-axis tightening loop
/// leaks nothing across calls.
///
/// # Errors
///
/// Any [`RunJailDefect`] — a jail that cannot be established (a missing primitive,
/// a non-ACL scratch volume, a `CreateProcessW` failure) refuses here exactly as
/// the production launcher does; the untrusted build is never run unconfined.
#[cfg(target_os = "windows")]
pub(crate) fn build_windows_jailed(
    profile: &SandboxProfile,
    scoped_tmp: &Path,
    working_tree: &Path,
    payload: &[OsString],
) -> Result<u32, RunJailDefect> {
    let Some((app, args)) = payload.split_first() else {
        return Err(RunJailDefect::Spawn {
            detail: "empty build-jail payload".to_owned(),
        });
    };
    windows_jail::run_confined(profile, scoped_tmp, working_tree, Path::new(app), args)
}

/// The scrubbed `(name, value)` environment pairs the Windows launcher passes to
/// `CreateProcess` as `lpEnvironment` — the `env` axis enforced launcher-side,
/// mirroring [`crate::build_jail::macos_scrubbed_env`] and the Linux jail's
/// `--clearenv` + re-export.
///
/// A pure function of the profile and a host-env lookup, so the exact set of
/// variables that survive into the child is unit-testable on any host (the
/// UTF-16 block-building that consumes it is the only Windows-specific step). The
/// child never inherits the launcher's environment: only this fixed minimal base
/// plus the profile's allowlisted names (and only when the host actually sets
/// them — a granted-but-unset name is simply absent, never a placeholder).
///
/// The base is the Windows-shaped analogue of the Unix arms' `PATH`/`TMPDIR`:
/// `SystemRoot` (Win32 API calls fail without it), `PATH` (system tool
/// resolution), and `TMP`/`TEMP` pointed at the always-writable scratch. `LANG`
/// re-exports when the host sets it, matching the Unix arms.
///
/// The returned pairs are sorted by name (case-insensitively) so the UTF-16 block
/// built from them satisfies `CreateProcessW`'s `CREATE_UNICODE_ENVIRONMENT`
/// sorted-block requirement — an unsorted block fails process creation with
/// `ERROR_ENVVAR_NOT_FOUND` (203).
#[must_use]
#[allow(clippy::too_long_first_doc_paragraph)]
pub fn windows_scrubbed_env(
    profile: &SandboxProfile,
    scoped_tmp: &Path,
    host_env: &dyn Fn(&str) -> Option<OsString>,
) -> Vec<(OsString, OsString)> {
    let mut env: Vec<(OsString, OsString)> = Vec::new();
    // `SystemRoot` is load-bearing on Windows: without it many Win32 calls (and
    // the loader) fail. Re-export the host's value when present, else the
    // conventional default, so the child is never left without it.
    let system_root = host_env("SystemRoot").unwrap_or_else(|| OsString::from("C:\\Windows"));
    env.push((OsString::from("SystemRoot"), system_root));
    // A minimal system PATH (the loader/tool resolution base), re-exported from
    // the host when set so a relocated system dir still resolves.
    if let Some(path) = host_env("PATH") {
        env.push((OsString::from("PATH"), path));
    }
    // Both TMP and TEMP point at the always-writable scratch (the Windows env has
    // two temp variables; different runtimes read different ones).
    env.push((OsString::from("TMP"), scoped_tmp.as_os_str().to_owned()));
    env.push((OsString::from("TEMP"), scoped_tmp.as_os_str().to_owned()));
    if let Some(lang) = host_env("LANG") {
        env.push((OsString::from("LANG"), lang));
    }
    // Only the profile's declared env names re-enter, and only when the host
    // actually sets them. An empty name can never form a valid `NAME=VALUE` entry
    // (Windows rejects a block containing one), so a granted-but-empty name is
    // dropped fail-closed rather than emitted as a malformed entry.
    for name in &profile.env_allowlist {
        if name.is_empty() {
            continue;
        }
        if let Some(value) = host_env(name) {
            env.push((OsString::from(name), value));
        }
    }
    // `CreateProcessW` with `CREATE_UNICODE_ENVIRONMENT` requires the environment
    // block sorted by name, case-insensitively in Windows' UPPERCASE-ordinal
    // collation — the same order the child's CRT/loader expects when it does an
    // ordered lookup on the block it is handed. An out-of-order block makes that
    // lookup miss a variable the child needs to initialise (notably `SystemRoot`),
    // and `CreateProcessW` fails with `ERROR_ENVVAR_NOT_FOUND` (203) before the
    // child runs. The distinction is load-bearing: lowercasing puts `_` (0x5F)
    // BEFORE the letters (`a`..=`z` = 0x61..=0x7A), whereas Windows uppercases and
    // so puts `_` AFTER the letters (`A`..=`Z` = 0x41..=0x5A) — an env name with an
    // underscore (the common case) sorts differently under the two, and only the
    // uppercase order matches what the child scans. Sort by name only (an env name
    // cannot contain `=`).
    env.sort_by_key(|(name, _)| env_name_collation_key(name));
    // The environment is case-insensitive on Windows, so a block holding two names
    // that collide under the collation key is malformed — the child's ordered lookup
    // sees an ambiguous key and `CreateProcessW` can 203. The profile parser accepts
    // an `env` name that collides with a fixed base name (e.g. a granted `SystemRoot`
    // or `Path`), so drop any later collision, keeping the first: the fixed base wins
    // over an allowlist re-grant, so a granted name can never displace a scrubbed base
    // to a different value. A stable sort keeps the base entry ahead of an allowlist
    // duplicate (the base was pushed first), so first-kept is fail-closed.
    env.dedup_by_key(|(name, _)| env_name_collation_key(name));
    env
}

/// The case-insensitive collation key `CreateProcessW`'s `CREATE_UNICODE_ENVIRONMENT`
/// block must be sorted on: Windows compares environment names by UPPERCASE ordinal,
/// so an env block the child can scan in order is one sorted on the ASCII-uppercased
/// name (matching the child CRT/loader's own ordered lookup). Sorting on the
/// lowercased name instead reorders any name containing a character that case-folds
/// across the letter range — `_` most notably — and the child then misses a required
/// variable, so process creation fails with `ERROR_ENVVAR_NOT_FOUND` (203).
///
/// Pure and host-independent (the produced pairs are unit-tested on any host); env
/// names are ASCII, so ASCII-uppercasing reproduces Windows' uppercase-ordinal order
/// over the range names actually use.
fn env_name_collation_key(name: &OsStr) -> String {
    name.to_string_lossy().to_ascii_uppercase()
}

/// An `OsStr` as UTF-16LE code units for the environment block. On Windows this is
/// the exact `encode_wide` of the underlying wide string (a path may hold a wide
/// unit no UTF-8 round-trip preserves, so the child receives it losslessly); off
/// Windows (the unit-test hosts) the lossy UTF-8 view suffices because the block
/// bytes are only asserted for ASCII names/values that round-trip identically.
#[cfg(windows)]
fn os_to_utf16(s: &OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt as _;
    s.encode_wide().collect()
}

// Compiled only for the unit tests off Windows (the block builder that calls it is
// Windows-production / any-host-test); the Windows arm always uses the exact
// `encode_wide` variant above.
#[cfg(all(not(windows), test))]
fn os_to_utf16(s: &OsStr) -> Vec<u16> {
    s.to_string_lossy().encode_utf16().collect()
}

/// The scrubbed environment as a doubly-NUL-terminated UTF-16LE block for
/// `CreateProcess` `lpEnvironment` under `CREATE_UNICODE_ENVIRONMENT`: each surviving
/// pair as `NAME=VALUE\0`, in the case-insensitive uppercase-ordinal name order
/// Windows requires, closed by a final extra NUL (so a non-empty block ends `\0\0`
/// and an empty block is a lone `\0\0`).
///
/// Pure and host-independent: it consumes the pairs [`windows_scrubbed_env`] already
/// scrubbed and sorted, so the exact bytes handed to `CreateProcessW` are asserted by
/// unit tests on any host — the Windows arm's `env_block_utf16` is a thin wrapper over
/// this. An entry with an empty name can never form a valid `NAME=VALUE` (Windows
/// rejects a block containing one), so the pair source drops empty names before this
/// point; this builder therefore only ever encodes well-formed entries.
// Windows production (the arm's `env_block_utf16` wraps it) plus the any-host unit
// tests that assert its exact bytes; nothing else on a non-Windows host calls it.
#[cfg(any(windows, test))]
#[must_use]
fn env_block_from_pairs(pairs: &[(OsString, OsString)]) -> Vec<u16> {
    let mut block: Vec<u16> = Vec::new();
    for (name, value) in pairs {
        block.extend(os_to_utf16(name));
        block.push(u16::from(b'='));
        block.extend(os_to_utf16(value));
        block.push(0);
    }
    // A non-empty block already ends in the last entry's own NUL; this appends the
    // block terminator (giving `\0\0`). An empty block needs both NULs written here
    // so `CreateProcess` reads a valid empty environment rather than running past a
    // lone terminator.
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    block
}

/// Off every jailed target the run jail is a documented refuse-gap.
///
/// # Errors
///
/// Always [`RunJailDefect::UnsupportedPlatform`]: no sound run jail can be built
/// here, so a capability-bearing program refuses to run rather than run
/// unconfined. This arm is gated on the negation of the [`on_jailed_target`]
/// predicate, so it compiles EXACTLY where [`platform_supports_jail`] is false.
// Kept a plain `fn` (not `const fn`) so its signature matches the real
// `exec_in_run_jail` arms, which cannot be `const`.
#[cfg(not(any(
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ),
    target_os = "macos",
    target_os = "windows"
)))]
#[allow(clippy::missing_const_for_fn)]
pub fn exec_in_run_jail(
    _tools: &RunJailTools,
    _profile: &SandboxProfile,
    _scoped_tmp: &Path,
    _working_tree: &Path,
    _app: &Path,
    _app_args: &[OsString],
) -> Result<std::convert::Infallible, RunJailDefect> {
    Err(RunJailDefect::UnsupportedPlatform {
        reason: "runtime jail is compiled only for Linux (x86_64/aarch64), macOS, and Windows",
    })
}

/// Embedded-app holder on platforms without the sealed-fd / exclusive-scratch
/// delivery path (Windows and unsupported targets). Embed mode is a Unix
/// deploy feature; this arm keeps the wrapper compiling everywhere and refuses
/// at run time rather than running unconfined.
#[cfg(not(any(
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ),
    target_os = "macos"
)))]
pub struct SealedApp {
    _bytes: Vec<u8>,
}

#[cfg(not(any(
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ),
    target_os = "macos"
)))]
impl SealedApp {
    /// Return the held bytes for the capability-floor verification scan.
    ///
    /// # Errors
    ///
    /// Never; returns `Result` for arm-parity with the Linux variant.
    pub fn read_sealed_bytes(&self) -> Result<Vec<u8>, RunJailDefect> {
        Ok(self._bytes.clone())
    }
}

/// Hold the embedded app bytes on a platform without sealed-fd delivery.
///
/// # Errors
///
/// Never; returns `Result` for arm-parity with the Linux variant.
#[cfg(not(any(
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ),
    target_os = "macos"
)))]
pub fn write_sealed_app_memfd(bytes: &[u8]) -> Result<SealedApp, RunJailDefect> {
    Ok(SealedApp {
        _bytes: bytes.to_vec(),
    })
}

/// Embedded exec is a documented refuse-gap on platforms without a run jail.
///
/// # Errors
///
/// Always [`RunJailDefect::UnsupportedPlatform`].
#[cfg(not(any(
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ),
    target_os = "macos"
)))]
#[allow(clippy::missing_const_for_fn)]
pub fn exec_embedded_in_run_jail(
    _tools: &RunJailTools,
    _profile: &SandboxProfile,
    _scoped_tmp: &Path,
    _working_tree: &Path,
    _app: &SealedApp,
    _app_args: &[OsString],
) -> Result<std::convert::Infallible, RunJailDefect> {
    Err(RunJailDefect::UnsupportedPlatform {
        reason: "runtime jail is compiled only for Linux (x86_64/aarch64), macOS, and Windows",
    })
}

// The two `fcntl` operations the pre_exec hook needs, wrapped so the raw
// `extern "C"` surface is contained. `FD_CLOEXEC` is the close-on-exec flag.
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
const FD_CLOEXEC: i32 = 1;

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
unsafe extern "C" {
    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
    fn memfd_create(name: *const core::ffi::c_char, flags: core::ffi::c_uint) -> i32;
    fn write(fd: i32, buf: *const core::ffi::c_void, count: usize) -> isize;
    fn read(fd: i32, buf: *mut core::ffi::c_void, count: usize) -> isize;
    fn lseek(fd: i32, offset: i64, whence: i32) -> i64;
    fn close(fd: i32) -> i32;
}

// memfd sealing constants (`<linux/memfd.h>` / `<linux/fcntl.h>`). A sealing
// memfd is created with `MFD_ALLOW_SEALING`; `F_ADD_SEALS` then applies the
// seal set. `F_SEAL_SEAL` forbids further seals — after it the byte content and
// size are frozen and cannot be re-opened writable by anyone holding the fd.
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
const MFD_ALLOW_SEALING: core::ffi::c_uint = 0x0002;
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
const F_ADD_SEALS: i32 = 1033;
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
const F_SEAL_SEAL: i32 = 0x0001;
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
const F_SEAL_SHRINK: i32 = 0x0002;
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
const F_SEAL_GROW: i32 = 0x0004;
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
const F_SEAL_WRITE: i32 = 0x0008;

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
const F_GETFD: i32 = 1;
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
const F_SETFD: i32 = 2;

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
fn libc_fcntl_getfd(fd: i32) -> std::io::Result<i32> {
    // SAFETY: a plain fcntl(F_GETFD) query on an owned fd; no memory is touched.
    let r = unsafe { fcntl(fd, F_GETFD) };
    if r < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(r)
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
fn libc_fcntl_setfd(fd: i32, flags: i32) -> std::io::Result<()> {
    // SAFETY: fcntl(F_SETFD, flags) on an owned fd; the variadic arg is a plain
    // int as the ABI requires.
    let r = unsafe { fcntl(fd, F_SETFD, flags) };
    if r < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Clear the close-on-exec flag on `fd` so an inherited fd (the seccomp memfd)
/// survives an exec. Async-signal-safe — safe to call from a `pre_exec` hook.
///
/// Shared by the run jail's own launcher and the captured-child build jail's
/// subprocess-denied variant, so the fd-inheritance handling is defined once.
///
/// # Errors
///
/// [`std::io::Error`] when either `fcntl` fails.
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
pub(crate) fn clear_cloexec(fd: i32) -> std::io::Result<()> {
    let flags = libc_fcntl_getfd(fd)?;
    libc_fcntl_setfd(fd, flags & !FD_CLOEXEC)
}

/// Write the compiled seccomp program to an anonymous in-memory file and return
/// its file descriptor, rewound to offset 0, ready for `bwrap --seccomp <fd>`.
///
/// A `memfd` is used rather than a temp file so the program bytes never touch
/// the filesystem (nothing to race or tamper on disk) and the fd is
/// self-cleaning when closed.
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
pub(crate) fn write_seccomp_memfd(bytes: &[u8]) -> Result<i32, RunJailDefect> {
    let spawn = |detail: String| RunJailDefect::Spawn { detail };
    let name = c"ipe-seccomp";
    // SAFETY: `memfd_create` with a valid NUL-terminated name and 0 flags
    // returns a new fd or -1; no memory is shared.
    let fd = unsafe { memfd_create(name.as_ptr(), 0) };
    if fd < 0 {
        return Err(spawn(format!(
            "memfd_create for the seccomp program failed: {}",
            std::io::Error::last_os_error()
        )));
    }
    // Write the whole program. A short write is a hard error — a truncated
    // seccomp program would be a malformed/rejected filter, so refuse.
    let mut written: usize = 0;
    while written < bytes.len() {
        let Some(remaining) = bytes.get(written..) else {
            break;
        };
        // SAFETY: `write` reads `remaining.len()` bytes from a valid slice
        // pointer into the owned memfd; the slice outlives the call.
        let n = unsafe {
            write(
                fd,
                remaining.as_ptr().cast::<core::ffi::c_void>(),
                remaining.len(),
            )
        };
        if n <= 0 {
            return Err(spawn(format!(
                "writing the seccomp program to the memfd failed: {}",
                std::io::Error::last_os_error()
            )));
        }
        written += usize::try_from(n).unwrap_or(0);
    }
    // Rewind so bwrap reads the program from the start.
    // SAFETY: lseek to absolute offset 0 (SEEK_SET = 0) on the owned fd.
    if unsafe { lseek(fd, 0, 0) } < 0 {
        return Err(spawn(format!(
            "rewinding the seccomp memfd failed: {}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(fd)
}

/// A sealed anonymous file holding the embedded app binary, owned by its raw
/// descriptor.  The descriptor is closed on drop.
///
/// The bytes are frozen by `F_SEAL_WRITE | F_SEAL_SHRINK | F_SEAL_GROW |
/// F_SEAL_SEAL`, so what a caller verifies by reading the fd is exactly what the
/// jail delivers from the same fd — there is no on-disk name to race, and no
/// writable re-open is possible even for a process holding the fd.
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
pub struct SealedApp {
    fd: i32,
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
impl SealedApp {
    /// The raw descriptor of the sealed anonymous file.
    #[must_use]
    pub const fn as_raw_fd(&self) -> i32 {
        self.fd
    }

    /// Read the full sealed contents by reading through the fd.
    ///
    /// Reads from offset 0 without disturbing the caller's later use of the fd
    /// (bwrap re-reads it from 0 itself via `--file`, but this rewinds after to
    /// be safe).  The bytes returned are the sealed bytes — the same inode the
    /// jail will deliver.
    ///
    /// # Errors
    ///
    /// [`RunJailDefect::Spawn`] on any seek or read failure.
    pub fn read_sealed_bytes(&self) -> Result<Vec<u8>, RunJailDefect> {
        let spawn = |detail: String| RunJailDefect::Spawn { detail };
        // SAFETY: lseek to absolute offset 0 (SEEK_SET = 0) on the owned fd.
        if unsafe { lseek(self.fd, 0, 0) } < 0 {
            return Err(spawn(format!(
                "rewinding the sealed app memfd failed: {}",
                std::io::Error::last_os_error()
            )));
        }
        let mut out: Vec<u8> = Vec::new();
        // Heap-allocated read buffer (a large on-stack array is a stack-size
        // hazard).
        let mut chunk = vec![0u8; 65536];
        loop {
            // SAFETY: `read` writes at most `chunk.len()` bytes into the owned,
            // fully-initialised `chunk` buffer; the pointer and length describe
            // exactly that buffer.
            let n = unsafe {
                read(
                    self.fd,
                    chunk.as_mut_ptr().cast::<core::ffi::c_void>(),
                    chunk.len(),
                )
            };
            if n < 0 {
                return Err(spawn(format!(
                    "reading the sealed app memfd failed: {}",
                    std::io::Error::last_os_error()
                )));
            }
            if n == 0 {
                break;
            }
            let read_len = usize::try_from(n).unwrap_or(0);
            if let Some(slice) = chunk.get(..read_len) {
                out.extend_from_slice(slice);
            }
        }
        // Rewind so a subsequent consumer reads from the start.
        // SAFETY: lseek to absolute offset 0 on the owned fd.
        if unsafe { lseek(self.fd, 0, 0) } < 0 {
            return Err(spawn(format!(
                "rewinding the sealed app memfd after read failed: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(out)
    }
}

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
impl Drop for SealedApp {
    fn drop(&mut self) {
        // SAFETY: `close` on the owned fd; after this the descriptor is not used.
        unsafe {
            close(self.fd);
        }
    }
}

/// Write `bytes` to an anonymous, sealing-capable in-memory file, seal it
/// against any further write/resize, and return the owned [`SealedApp`].
///
/// The returned fd is NON-close-on-exec so it is inherited across the
/// wrapper→bwrap process replacement, letting bwrap materialise the app inside
/// the jail from the same sealed inode via `--file`.  Sealing (`F_SEAL_WRITE |
/// F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_SEAL`) makes the verified-then-executed
/// bytes provably identical: no path lookup, no writable re-open.
///
/// # Errors
///
/// [`RunJailDefect::Spawn`] on any syscall failure.
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
pub fn write_sealed_app_memfd(bytes: &[u8]) -> Result<SealedApp, RunJailDefect> {
    let spawn = |detail: String| RunJailDefect::Spawn { detail };
    let name = c"ipe-embedded-app";
    // SAFETY: `memfd_create` with a valid NUL-terminated name and the
    // `MFD_ALLOW_SEALING` flag returns a new fd or -1; no memory is shared.
    // `MFD_CLOEXEC` is deliberately NOT set: the fd must survive the exec into
    // bwrap so bwrap can read the app from it.
    let fd = unsafe { memfd_create(name.as_ptr(), MFD_ALLOW_SEALING) };
    if fd < 0 {
        return Err(spawn(format!(
            "memfd_create for the embedded app failed: {}",
            std::io::Error::last_os_error()
        )));
    }
    let sealed = SealedApp { fd };
    // Write the whole binary. A short write is a hard error — a truncated app
    // would be a corrupt executable, so refuse.
    let mut written: usize = 0;
    while written < bytes.len() {
        let Some(remaining) = bytes.get(written..) else {
            break;
        };
        // SAFETY: `write` reads `remaining.len()` bytes from a valid slice
        // pointer into the owned memfd; the slice outlives the call.
        let n = unsafe {
            write(
                fd,
                remaining.as_ptr().cast::<core::ffi::c_void>(),
                remaining.len(),
            )
        };
        if n <= 0 {
            return Err(spawn(format!(
                "writing the embedded app to the memfd failed: {}",
                std::io::Error::last_os_error()
            )));
        }
        written += usize::try_from(n).unwrap_or(0);
    }
    // Seal against write, shrink, grow, and further sealing. After this the
    // byte content and size are frozen for the lifetime of the fd.
    let seals = F_SEAL_WRITE | F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_SEAL;
    // SAFETY: fcntl(F_ADD_SEALS, seals) on the owned sealing-capable memfd; the
    // variadic arg is a plain int as the ABI requires.
    if unsafe { fcntl(fd, F_ADD_SEALS, seals) } < 0 {
        return Err(spawn(format!(
            "sealing the embedded app memfd failed: {}",
            std::io::Error::last_os_error()
        )));
    }
    // Rewind so the first reader (verification scan) starts at the beginning.
    // SAFETY: lseek to absolute offset 0 (SEEK_SET = 0) on the owned fd.
    if unsafe { lseek(fd, 0, 0) } < 0 {
        return Err(spawn(format!(
            "rewinding the embedded app memfd failed: {}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(sealed)
}

/// The marker that begins the capability-floor line embedded in the binary's
/// `.rodata`.
///
/// See the CLI's `capfloor_static_source`; [`scan_capfloor`] finds the floor by
/// this marker, which survives `strip` (`.rodata` is allocated).
pub const CAPFLOOR_MARKER: &str = "ipe-capfloor 1 ";

/// Scan a binary's bytes for the embedded capability-floor line and parse it —
/// the tamper-safe floor read that does NOT execute the binary and survives
/// `strip`.
///
/// The floor is a `#[used]` static in `.rodata` (an allocated section `strip`
/// keeps), so it is present as raw bytes in the file. This scans for the unique
/// [`CAPFLOOR_MARKER`] prefix and parses the line up to the first NUL or
/// newline. A binary with no marker yields `None` — the launcher treats that as
/// "no readable floor" and refuses (never as "no floor, anything goes").
///
/// The floor is a *ceiling on grants*: the profile must not grant more than the
/// floor ([`SandboxProfile::satisfies_capfloor`]). So if the binary contains
/// MULTIPLE marker occurrences (it should not — the static is emitted once), the
/// STRICTEST (least-granting) floor wins: an attacker who appends a more
/// permissive floor line cannot raise the ceiling and relax the jail. Concretely
/// the floors are intersected (an axis is in the merged floor only if EVERY
/// occurrence grants it, and an env name survives only if EVERY occurrence grants
/// it — the name-set intersection).
#[must_use]
pub fn scan_capfloor(bytes: &[u8]) -> Option<SandboxProfile> {
    let marker = CAPFLOOR_MARKER.as_bytes();
    let mut floors: Vec<SandboxProfile> = Vec::new();
    // Every window that starts with the marker begins a candidate floor line.
    for (start, _) in bytes
        .windows(marker.len())
        .enumerate()
        .filter(|(_, w)| *w == marker)
    {
        // The line runs from the marker to the first NUL or newline.
        let rest = bytes.get(start..).unwrap_or(&[]);
        let end = rest
            .iter()
            .position(|&b| b == 0 || b == b'\n')
            .unwrap_or(rest.len());
        if let Ok(line) = std::str::from_utf8(rest.get(..end).unwrap_or(&[]))
            && let Ok(p) = parse_capfloor(line)
        {
            floors.push(p);
        }
    }
    let (first, rest) = floors.split_first()?;
    // Intersect to the strictest floor: an axis stays granted only if EVERY
    // occurrence grants it; the env ceiling is the NAME-SET intersection (a name
    // survives only if every occurrence grants it). A forged permissive copy
    // cannot raise the ceiling nor swap in a name the legitimate floor omits.
    let mut merged = first.clone();
    let mut env_names: BTreeSet<&str> = first.env_allowlist.iter().map(String::as_str).collect();
    for f in rest {
        if !f.network {
            merged.network = false;
        }
        if matches!(f.filesystem, FilesystemScope::Isolated) {
            merged.filesystem = FilesystemScope::Isolated;
        }
        if !f.subprocess {
            merged.subprocess = false;
        }
        let occurrence: BTreeSet<&str> = f.env_allowlist.iter().map(String::as_str).collect();
        env_names.retain(|n| occurrence.contains(n));
    }
    merged.env_allowlist = env_names.into_iter().map(str::to_owned).collect();
    Some(merged)
}

/// The Windows run-jail launcher: assembles the Job Object + AppContainer token +
/// scrubbed environment and launches the app confined, fail-closed at every step.
///
/// Every raw Win32 call is wrapped in a small checked helper here; every HANDLE
/// is owned by [`OwnedHandle`] (closed on drop, so no failure path leaks a handle
/// or leaves the child running outside the job), every allocated SID / attribute
/// list is freed on every path, and any construction failure returns a
/// [`RunJailDefect`] — the capability-bearing app is NEVER launched unconfined.
// The launcher's doc comments name Win32 APIs and multi-word OS proper nouns
// (Job Object, AppContainer, CreateProcess) throughout; backticking every
// occurrence adds noise without clarity, and the Win32 argument-count is
// intrinsic to the API surface. Scoped allows for exactly those doc/style lints;
// every soundness lint stays enforced.
#[cfg(target_os = "windows")]
#[allow(
    clippy::doc_markdown,
    clippy::too_long_first_doc_paragraph,
    clippy::too_many_arguments
)]
mod windows_jail {
    use super::{FilesystemScope, RunJailDefect, SandboxProfile, windows_scrubbed_env};
    use std::ffi::{OsStr, OsString};
    use std::os::windows::ffi::OsStrExt as _;
    use std::path::Path;

    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, INVALID_HANDLE_VALUE, LocalFree,
        S_OK, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::Security::Authorization::{
        EXPLICIT_ACCESS_W, SET_ACCESS, SetEntriesInAclW, SetNamedSecurityInfoW, TRUSTEE_IS_SID,
        TRUSTEE_IS_USER, TRUSTEE_IS_WELL_KNOWN_GROUP, TRUSTEE_W,
    };
    use windows_sys::Win32::Security::Isolation::{
        CreateAppContainerProfile, DeleteAppContainerProfile,
        DeriveAppContainerSidFromAppContainerName,
    };
    use windows_sys::Win32::Security::{
        ACL, DACL_SECURITY_INFORMATION, FreeSid, GetTokenInformation, NO_INHERITANCE, PSID,
        SECURITY_CAPABILITIES, SECURITY_MAX_SID_SIZE, SID_AND_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
        TokenUser, WELL_KNOWN_SID_TYPE, WinCapabilityInternetClientSid,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_TRAVERSE, GetVolumeInformationW,
        GetVolumePathNameW,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_BASIC_LIMIT_INFORMATION,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    use windows_sys::Win32::System::SystemServices::{FILE_PERSISTENT_ACLS, SE_GROUP_ENABLED};
    use windows_sys::Win32::System::Threading::{
        CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
        DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess,
        GetExitCodeProcess, InitializeProcThreadAttributeList, OpenProcessToken,
        PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, PROCESS_INFORMATION, ResumeThread,
        STARTUPINFOEXW, UpdateProcThreadAttribute, WaitForSingleObject,
    };

    /// Access rights ACLed onto a granted path for the container SID (read+write).
    const FILE_RW: u32 = FILE_GENERIC_READ | FILE_GENERIC_WRITE;

    /// Keep the pure, cross-platform [`super::FILE_PERSISTENT_ACLS_FLAG`] (used by
    /// the host-independent volume-capability decision + its unit tests) in
    /// lockstep with the real Win32 value from `windows-sys`. If they ever diverge
    /// this fails the Windows build.
    // IPE-RUST-AUDIT:ACCEPTED (Arthur Maciel) — compile-time `const` assertion (not a runtime panic); it fails the build if the cross-platform flag drifts from the Win32 constant [ledger #boundary]
    const _: () = assert!(super::FILE_PERSISTENT_ACLS_FLAG == FILE_PERSISTENT_ACLS);

    /// An owned Win32 `HANDLE` closed on drop — RAII so no error path leaks a
    /// handle. A null/`INVALID_HANDLE_VALUE` handle is treated as "nothing to
    /// close".
    struct OwnedHandle(HANDLE);

    impl OwnedHandle {
        const fn get(&self) -> HANDLE {
            self.0
        }
    }

    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
                // SAFETY: `self.0` is a live handle this type owns; closing it once
                // on drop is the RAII contract, and no copy of it is used after.
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }
    }

    /// An owned AppContainer SID (`FreeSid` on drop). Allocated by
    /// `DeriveAppContainerSidFromAppContainerName`.
    struct OwnedSid(PSID);

    impl Drop for OwnedSid {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: `self.0` was allocated by a SID-allocating Win32 call this
                // type owns; `FreeSid` releases it exactly once on drop.
                unsafe {
                    FreeSid(self.0);
                }
            }
        }
    }

    /// A NUL-terminated UTF-16 string for a Win32 wide-string argument.
    fn wide(s: &OsStr) -> Vec<u16> {
        let mut v: Vec<u16> = s.encode_wide().collect();
        v.push(0);
        v
    }

    fn spawn(detail: impl Into<String>) -> RunJailDefect {
        RunJailDefect::Spawn {
            detail: detail.into(),
        }
    }

    fn last_error_spawn(context: &str) -> RunJailDefect {
        // SAFETY: `GetLastError` reads this thread's last-error slot; no memory
        // is accessed.
        let code = unsafe { GetLastError() };
        spawn(format!("{context} (GetLastError = {code})"))
    }

    /// The launcher entry point: run the app confined, then exit this launcher
    /// with the child's code (Windows has no `exec`-replace, so the launcher stays
    /// alive as the job owner and propagates the exit). Fails closed on any step.
    pub(super) fn launch(
        profile: &SandboxProfile,
        scoped_tmp: &Path,
        working_tree: &Path,
        app: &Path,
        app_args: &[OsString],
    ) -> Result<std::convert::Infallible, RunJailDefect> {
        let code = run_confined(profile, scoped_tmp, working_tree, app, app_args)?;
        // IPE-RUST-AUDIT:ACCEPTED (Arthur Maciel) — jail exec process control: the launcher is the job owner and replaces itself with the confined child's exit code (returns Infallible) [ledger #boundary]
        std::process::exit(i32::from_ne_bytes(code.to_ne_bytes()));
    }

    /// Run the app confined and RETURN its exit code (never exits the launcher) —
    /// the whole Job-Object + AppContainer + scrubbed-env sequence, fail-closed at
    /// every step. [`launch`] wraps this and exits with the code; the CI E2E calls
    /// the [`super::run_windows_jailed_for_test`] seam so it can assert on the
    /// enforce-vs-control exit codes without the launcher replacing the test
    /// process.
    pub(super) fn run_confined(
        profile: &SandboxProfile,
        scoped_tmp: &Path,
        working_tree: &Path,
        app: &Path,
        app_args: &[OsString],
    ) -> Result<u32, RunJailDefect> {
        // 1. Derive a per-run AppContainer SID from a unique per-run name. The
        //    profile is created (idempotently) so the SID is registerable, then
        //    deleted after the SID is derived — the SID outlives the profile.
        let container_name = per_run_container_name();
        let container = AppContainer::create(&container_name)?;

        // 2. Capability SIDs: internetClient iff the profile grants network. An
        //    absent capability ⇒ outbound connect denied by AppContainer network
        //    isolation (deny-by-default).
        let mut capabilities = CapabilitySids::new();
        if profile.network {
            capabilities.push_well_known(WinCapabilityInternetClientSid)?;
        }

        // 3. ACL the granted resources to the container SID (deny-by-default: only
        //    what is ACLed is reachable). Always the scratch; the working tree only
        //    under the filesystem axis. A failed ACL refuses (never run with an
        //    unenforced write boundary).
        //
        //    Before ACLing, PROVE each path lives on a volume that persists+enforces
        //    DACLs (`FILE_PERSISTENT_ACLS`). On a non-ACL volume (FAT/exFAT) the ACL
        //    is a silent no-op — `SetNamedSecurityInfoW` returns success while
        //    enforcing nothing — so the filesystem boundary the admit path already
        //    trusted would not exist. The probe fails closed, keeping the
        //    always-confined `Filesystem` claim honest.
        probe_volume_persists_acls(scoped_tmp)?;
        acl_path_for_container(scoped_tmp, container.sid())?;
        // The AppContainer token must be able to TRAVERSE every ancestor directory
        // between the volume root and the scratch in order for CreateProcessW to
        // resolve `scoped_tmp` as `lpCurrentDirectory`. ACLing the scratch itself
        // is not enough: Windows path resolution walks each component, and a
        // directory that denies `FILE_TRAVERSE` to the container SID makes the walk
        // stop, returning ERROR_ENVVAR_NOT_FOUND (203) from CreateProcessW before
        // the child starts. This grants the minimal traverse right on each ancestor
        // up to (but not including) the volume root, which already allows traversal
        // to everyone by default. The grant is additive (not a replace-DACL), so
        // it never removes existing permissions.
        grant_traverse_to_ancestors(scoped_tmp, container.sid())?;
        if profile.filesystem == FilesystemScope::WorkingTreeReadWrite {
            probe_volume_persists_acls(working_tree)?;
            acl_path_for_container(working_tree, container.sid())?;
        }

        // 4. The Job Object: kill-on-close, no breakaway (never set), active-process
        //    cap = 1 when subprocess withheld else the profile's proc cap.
        let job = create_job(profile)?;

        // 5. The scrubbed environment block (never inherit the launcher's).
        let host_env = |k: &str| std::env::var_os(k);
        let env_block = env_block_utf16(profile, scoped_tmp, &host_env);

        // 6. CreateProcess suspended, with the AppContainer security-capabilities
        //    attribute and the scrubbed environment. The child's current directory
        //    is the always-ACLed scratch — the ONE directory the container token can
        //    always reach. The working tree cannot be the CWD: under a
        //    filesystem-withholding profile it is NOT ACLed to the container SID, so
        //    CreateProcessW would fail resolving the current directory with
        //    ERROR_ENVVAR_NOT_FOUND (203) before the child ran. The CWD is not a
        //    capability, so scratch-as-CWD neither grants nor widens any axis (and
        //    matches the Unix arms, which do not chdir into the working tree either).
        let child = create_suspended_appcontainer_process(
            app,
            app_args,
            scoped_tmp,
            &container,
            &mut capabilities,
            &env_block,
        )?;

        // 7. Assign to the job BEFORE resuming, so no instruction runs un-jobbed.
        assign_to_job(job.get(), child.process.get())?;

        // 8. Resume the main thread.
        resume(child.thread.get())?;

        // 9. Wait for the jailed child; the job handle stays open (kill-on-close
        //    holds for the child's lifetime). Then tear down the container profile
        //    and return the child's code.
        let code = wait_and_exit_code(child.process.get())?;
        drop(child);
        drop(job);
        drop(capabilities);
        container.delete();
        Ok(code)
    }

    /// A per-run AppContainer name: unique enough that concurrent `ipe run`
    /// invocations do not collide on the same container profile.
    fn per_run_container_name() -> OsString {
        let pid = std::process::id();
        // A monotonic-ish suffix from the process id and a coarse time; the SID is
        // per-run and torn down, so uniqueness within the host at this instant is
        // all that is needed.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        OsString::from(format!("ipe.run.jail.{pid}.{nanos}"))
    }

    /// An AppContainer profile + its derived container SID. The profile is deleted
    /// explicitly via [`Self::delete`] after launch; the SID is freed on drop.
    struct AppContainer {
        name: Vec<u16>,
        sid: OwnedSid,
    }

    impl AppContainer {
        fn create(name: &OsStr) -> Result<Self, RunJailDefect> {
            let wname = wide(name);
            // CreateAppContainerProfile registers the container so its SID is
            // derivable. ERROR_ALREADY_EXISTS is tolerated (a stale same-name
            // profile), any other failure refuses.
            // The derived SID from CreateAppContainerProfile is discarded here (we
            // re-derive it by name below into the owned SID); it must still be a
            // valid out-pointer.
            let mut created_sid: PSID = std::ptr::null_mut();
            // SAFETY: `wname` is a live NUL-terminated wide string; the display
            // name / description point at the same buffer (only used for
            // registration); no capabilities are attached at profile creation;
            // `created_sid` is a live out-pointer.
            let hr = unsafe {
                CreateAppContainerProfile(
                    wname.as_ptr(),
                    wname.as_ptr(),
                    wname.as_ptr(),
                    std::ptr::null(),
                    0,
                    std::ptr::from_mut(&mut created_sid),
                )
            };
            // The profile-creation SID (when returned) is owned by the caller;
            // free it — the launcher uses the by-name derivation below.
            if !created_sid.is_null() {
                // SAFETY: `created_sid` was allocated by CreateAppContainerProfile.
                unsafe {
                    FreeSid(created_sid);
                }
            }
            if hr != S_OK {
                // HRESULT_FROM_WIN32(ERROR_ALREADY_EXISTS) is acceptable.
                let already = HRESULT_ALREADY_EXISTS;
                if hr != already {
                    return Err(spawn(format!(
                        "CreateAppContainerProfile failed (HRESULT = {hr:#x}); refusing to run \
                         unconfined"
                    )));
                }
            }
            // Derive the container SID from the registered name.
            let mut sid: PSID = std::ptr::null_mut();
            // SAFETY: `wname` is a live NUL-terminated wide string; `sid` receives a
            // freshly allocated SID on success (freed by OwnedSid on drop).
            let hr = unsafe {
                DeriveAppContainerSidFromAppContainerName(
                    wname.as_ptr(),
                    std::ptr::from_mut(&mut sid),
                )
            };
            if hr != S_OK || sid.is_null() {
                // Best-effort profile cleanup before refusing.
                // SAFETY: `wname` is a live NUL-terminated wide string.
                unsafe {
                    DeleteAppContainerProfile(wname.as_ptr());
                }
                return Err(spawn(format!(
                    "DeriveAppContainerSidFromAppContainerName failed (HRESULT = {hr:#x}); \
                     refusing to run unconfined"
                )));
            }
            Ok(Self {
                name: wname,
                sid: OwnedSid(sid),
            })
        }

        const fn sid(&self) -> PSID {
            self.sid.0
        }

        fn delete(&self) {
            // SAFETY: `self.name` is the live NUL-terminated wide name used to
            // create the profile; deleting it releases the registered container.
            unsafe {
                DeleteAppContainerProfile(self.name.as_ptr());
            }
        }
    }

    /// `HRESULT_FROM_WIN32(ERROR_ALREADY_EXISTS)` — the tolerated
    /// already-registered profile result. Built by the standard HRESULT-from-Win32
    /// bit layout (`0x8007_0000 | (code & 0xFFFF)`) as a bit pattern reinterpreted
    /// into the signed `HRESULT` (`i32`), avoiding a wrapping `as` cast.
    const HRESULT_ALREADY_EXISTS: i32 =
        i32::from_ne_bytes((0x8007_0000u32 | (ERROR_ALREADY_EXISTS & 0xFFFF)).to_ne_bytes());

    /// The AppContainer capability SIDs (network etc.), each a well-known SID this
    /// type allocates and frees on drop, held alongside the `SID_AND_ATTRIBUTES`
    /// array `SECURITY_CAPABILITIES` points at.
    struct CapabilitySids {
        sids: Vec<OwnedWellKnownSid>,
        attrs: Vec<SID_AND_ATTRIBUTES>,
    }

    impl CapabilitySids {
        const fn new() -> Self {
            Self {
                sids: Vec::new(),
                attrs: Vec::new(),
            }
        }

        fn push_well_known(&mut self, kind: WELL_KNOWN_SID_TYPE) -> Result<(), RunJailDefect> {
            let sid = OwnedWellKnownSid::create(kind)?;
            self.sids.push(sid);
            Ok(())
        }

        /// Build the `SID_AND_ATTRIBUTES` array (each enabled) and return a
        /// `SECURITY_CAPABILITIES` referencing the container SID + this array. The
        /// returned struct borrows `self`, so `self` must outlive the CreateProcess
        /// call — the launcher keeps it alive until after the process is created.
        fn security_capabilities(&mut self, container_sid: PSID) -> SECURITY_CAPABILITIES {
            self.attrs.clear();
            for owned in &self.sids {
                self.attrs.push(SID_AND_ATTRIBUTES {
                    Sid: owned.as_psid(),
                    // `SE_GROUP_ENABLED` is declared `i32`; the field is `u32`.
                    #[allow(clippy::cast_sign_loss)]
                    Attributes: SE_GROUP_ENABLED as u32,
                });
            }
            // The capability count is at most a handful of SIDs — never near
            // u32::MAX — so the conversion is total in practice; use a saturating
            // try_from rather than a wrapping cast.
            let cap_count = u32::try_from(self.attrs.len()).unwrap_or(u32::MAX);
            let (cap_ptr, cap_count) = if self.attrs.is_empty() {
                (std::ptr::null_mut(), 0)
            } else {
                (self.attrs.as_mut_ptr(), cap_count)
            };
            SECURITY_CAPABILITIES {
                AppContainerSid: container_sid,
                Capabilities: cap_ptr,
                CapabilityCount: cap_count,
                Reserved: 0,
            }
        }
    }

    /// A well-known capability SID allocated with `CreateWellKnownSid` into an
    /// owned heap buffer; no separate free is needed (the buffer owns the SID's
    /// storage, released when the buffer drops). The `PSID` is derived from the
    /// buffer on demand, so the pointer can never outlive the allocation.
    struct OwnedWellKnownSid {
        buf: Vec<u8>,
    }

    impl OwnedWellKnownSid {
        fn create(kind: WELL_KNOWN_SID_TYPE) -> Result<Self, RunJailDefect> {
            use windows_sys::Win32::Security::CreateWellKnownSid;
            let mut buf: Vec<u8> = vec![0u8; SECURITY_MAX_SID_SIZE as usize];
            // The buffer is SECURITY_MAX_SID_SIZE bytes (a small constant), so the
            // length always fits a u32.
            let mut size: u32 = u32::try_from(buf.len()).unwrap_or(u32::MAX);
            // SAFETY: `buf` is a live buffer of SECURITY_MAX_SID_SIZE bytes; the
            // call writes the SID into it and updates `size`.
            let ok = unsafe {
                CreateWellKnownSid(
                    kind,
                    std::ptr::null_mut(),
                    buf.as_mut_ptr().cast(),
                    std::ptr::from_mut(&mut size),
                )
            };
            if ok == 0 {
                return Err(last_error_spawn("CreateWellKnownSid failed"));
            }
            Ok(Self { buf })
        }

        /// The SID pointer into the owned buffer. Valid for as long as `self` (and
        /// so the buffer) lives — the caller keeps the `OwnedWellKnownSid` alive
        /// across the `CreateProcess` call that reads it.
        const fn as_psid(&self) -> PSID {
            self.buf.as_ptr().cast::<std::ffi::c_void>().cast_mut()
        }
    }

    /// The size of a `T` as a Win32 `u32` byte count. Every struct passed to a
    /// Win32 call here is far smaller than `u32::MAX`, so the conversion is total;
    /// `try_from` avoids a wrapping `as` cast and never panics.
    fn size_u32<T>() -> u32 {
        u32::try_from(std::mem::size_of::<T>()).unwrap_or(u32::MAX)
    }

    /// Create the Job Object and set its extended limits: kill-on-close, no
    /// breakaway, active-process cap.
    fn create_job(profile: &SandboxProfile) -> Result<OwnedHandle, RunJailDefect> {
        // SAFETY: a nameless, default-security Job Object; returns a handle or null.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(last_error_spawn("CreateJobObjectW failed"));
        }
        let job = OwnedHandle(handle);

        let active_cap = active_process_cap(profile);

        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation = JOBOBJECT_BASIC_LIMIT_INFORMATION {
            // KILL_ON_JOB_CLOSE: every process dies when the launcher's job handle
            // closes. ACTIVE_PROCESS: the count cap. BREAKAWAY_OK is deliberately
            // NOT set — a child cannot escape the job.
            LimitFlags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
            ActiveProcessLimit: active_cap,
            ..info.BasicLimitInformation
        };

        // SAFETY: `info` is a fully-initialized extended-limit struct; the call
        // reads `size_of::<…>()` bytes from it and applies them to the owned job.
        let ok = unsafe {
            SetInformationJobObject(
                job.get(),
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&info).cast(),
                size_u32::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>(),
            )
        };
        if ok == 0 {
            return Err(last_error_spawn("SetInformationJobObject failed"));
        }
        Ok(job)
    }

    /// ACL a path's DACL to grant read+write to exactly two trustees — the
    /// AppContainer SID (so the sandboxed process can reach its scratch/working
    /// tree) and the launcher's own user SID (so this process and SYSTEM keep the
    /// access post-run cleanup — `remove_dir_all(scoped_tmp)` — needs). Everyone
    /// else stays implicitly denied: this is a fresh DACL with only these two
    /// grants, so deny-by-default holds. The launcher grant does NOT widen the
    /// sandboxed app's reach: the AppContainer process runs as the container SID,
    /// never as the launcher user. A failure refuses — never run with an
    /// unenforced write boundary.
    ///
    /// The caller must have already established, via
    /// [`probe_volume_persists_acls`], that `path` lives on a volume with
    /// `FILE_PERSISTENT_ACLS`; on a non-ACL volume (FAT/exFAT) `SetNamedSecurityInfoW`
    /// would return success while enforcing nothing, so the probe fails closed
    /// upstream rather than letting this establish a no-op boundary. That
    /// probe → refuse decision is unit-tested through the pure
    /// [`super::volume_flags_confine_filesystem`].
    fn acl_path_for_container(path: &Path, container_sid: PSID) -> Result<(), RunJailDefect> {
        // The launcher's user SID, kept alive in an owned buffer for the whole
        // `SetEntriesInAclW` call (the EXPLICIT_ACCESS entry borrows the SID by
        // pointer).
        let launcher_sid_buf = launcher_user_sid_buffer()?;
        // SAFETY: `launcher_sid_buf` holds a live `TOKEN_USER` whose `User.Sid`
        // points into the same buffer; the read is a plain field access of a
        // populated, correctly aligned structure that outlives every use below.
        let launcher_sid: PSID = unsafe {
            let token_user = launcher_sid_buf.as_ptr().cast::<TOKEN_USER>();
            (*token_user).User.Sid
        };
        if launcher_sid.is_null() {
            return Err(spawn(format!(
                "launcher user SID was null for {}; refusing to run without a cleanup grant",
                path.display()
            )));
        }
        let entries = [
            EXPLICIT_ACCESS_W {
                grfAccessPermissions: FILE_RW,
                grfAccessMode: SET_ACCESS,
                grfInheritance: NO_INHERITANCE,
                Trustee: TRUSTEE_W {
                    pMultipleTrustee: std::ptr::null_mut(),
                    MultipleTrusteeOperation: 0,
                    TrusteeForm: TRUSTEE_IS_SID,
                    TrusteeType: TRUSTEE_IS_WELL_KNOWN_GROUP,
                    ptstrName: container_sid.cast(),
                },
            },
            EXPLICIT_ACCESS_W {
                grfAccessPermissions: FILE_RW,
                grfAccessMode: SET_ACCESS,
                grfInheritance: NO_INHERITANCE,
                Trustee: TRUSTEE_W {
                    pMultipleTrustee: std::ptr::null_mut(),
                    MultipleTrusteeOperation: 0,
                    TrusteeForm: TRUSTEE_IS_SID,
                    TrusteeType: TRUSTEE_IS_USER,
                    ptstrName: launcher_sid.cast(),
                },
            },
        ];
        let mut new_acl: *mut ACL = std::ptr::null_mut();
        // SAFETY: two EXPLICIT_ACCESS entries in a live array; both trustee SIDs
        // outlive the call (`container_sid` owned by the caller, `launcher_sid`
        // pinned by `launcher_sid_buf`). `SetEntriesInAclW` allocates `new_acl`
        // (freed via LocalFree below). A null existing ACL means "start fresh".
        let err = unsafe {
            SetEntriesInAclW(
                2,
                entries.as_ptr(),
                std::ptr::null_mut(),
                std::ptr::from_mut(&mut new_acl),
            )
        };
        if err != 0 || new_acl.is_null() {
            return Err(spawn(format!(
                "SetEntriesInAclW failed for {} (error = {err}); refusing to run without an \
                 enforced filesystem boundary",
                path.display()
            )));
        }
        let wpath = wide(path.as_os_str());
        // SAFETY: `wpath` is a live NUL-terminated wide path; `new_acl` is the ACL
        // just built; only the DACL is set. SE_FILE_OBJECT = 1.
        let status = unsafe {
            SetNamedSecurityInfoW(
                wpath.as_ptr(),
                1, // SE_FILE_OBJECT
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                new_acl,
                std::ptr::null_mut(),
            )
        };
        // Free the ACL regardless of outcome.
        // SAFETY: `new_acl` was allocated by SetEntriesInAclW; LocalFree releases it.
        unsafe {
            LocalFree(new_acl.cast());
        }
        if status != 0 {
            return Err(spawn(format!(
                "SetNamedSecurityInfoW failed for {} (status = {status}); refusing to run without \
                 an enforced filesystem boundary",
                path.display()
            )));
        }
        Ok(())
    }

    /// Grant `FILE_TRAVERSE` to the AppContainer SID on every ancestor directory
    /// of `path` up to (but not including) the volume root.
    ///
    /// `CreateProcessW` resolves `lpCurrentDirectory` by walking every path
    /// component through the AppContainer token. If any ancestor directory does
    /// not have `FILE_TRAVERSE` granted to the container SID, the walk stops and
    /// the call returns `ERROR_ENVVAR_NOT_FOUND` (203) before the child process
    /// starts. ACLing the scratch directory itself (done by `acl_path_for_container`)
    /// is therefore not enough: every ancestor up to the volume root must also allow
    /// the container SID to traverse it.
    ///
    /// The grant is additive: the function merges a single `GRANT_ACCESS`
    /// `FILE_TRAVERSE` entry into the existing DACL of each ancestor via
    /// `SetEntriesInAclW` with a non-null existing ACL, preserving every other
    /// entry. This never removes or narrows any existing permission. Fail-closed:
    /// any failed Win32 call refuses so the caller never proceeds with an
    /// incompletely granted ancestor chain.
    fn grant_traverse_to_ancestors(path: &Path, container_sid: PSID) -> Result<(), RunJailDefect> {
        use windows_sys::Win32::Security::Authorization::{GRANT_ACCESS, GetNamedSecurityInfoW};

        // Resolve the volume root so we know when to stop walking.
        const MAX_PATH_WCHARS: u32 = 260;
        let wpath = wide(path.as_os_str());
        let mut root_buf: Vec<u16> = vec![0u16; MAX_PATH_WCHARS as usize];
        // SAFETY: `wpath` is a live NUL-terminated wide path; `root_buf` receives
        // the volume mount-point (e.g. "C:\") NUL-terminated into a MAX_PATH buffer.
        let got_root =
            unsafe { GetVolumePathNameW(wpath.as_ptr(), root_buf.as_mut_ptr(), MAX_PATH_WCHARS) };
        if got_root == 0 {
            return Err(last_error_spawn(&format!(
                "GetVolumePathNameW failed for {} while granting ancestor traversal; refusing",
                path.display()
            )));
        }
        // Trim to the actual NUL-terminated string length for comparison.
        let root_len = root_buf
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(root_buf.len());
        let volume_root: Vec<u16> = root_buf[..root_len].to_vec();

        // Walk from `path`'s parent up toward (but not including) the volume root.
        let mut current = path.to_path_buf();
        loop {
            current = match current.parent() {
                Some(p) => p.to_path_buf(),
                None => break,
            };
            // Stop at the volume root — it already allows traversal to everyone.
            let wcurrent = wide(current.as_os_str());
            let current_len = wcurrent.len().saturating_sub(1); // exclude NUL
            if current_len == volume_root.len()
                && wcurrent[..current_len]
                    .iter()
                    .zip(&volume_root)
                    .all(|(a, b)| {
                        // `u16` has no `to_ascii_uppercase`; fold through `u8` for the
                        // ASCII range (volume roots are always ASCII, e.g. "C:\").
                        let au = if *a < 0x80 {
                            (*a as u8).to_ascii_uppercase() as u16
                        } else {
                            *a
                        };
                        let bu = if *b < 0x80 {
                            (*b as u8).to_ascii_uppercase() as u16
                        } else {
                            *b
                        };
                        au == bu
                    })
            {
                break;
            }

            // Fetch the existing DACL via GetNamedSecurityInfoW so we can merge
            // into it (GRANT_ACCESS preserves existing entries).
            let mut existing_dacl: *mut ACL = std::ptr::null_mut();
            // GetNamedSecurityInfoW writes a PSECURITY_DESCRIPTOR (*mut c_void)
            // here; LocalFree releases it when we are done. The DACL pointer
            // (`existing_dacl`) points into this allocation and is valid until
            // `sd_ptr` is freed.
            let mut sd_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
            // SAFETY: `wcurrent` is a live NUL-terminated wide path; the call
            // allocates the security descriptor into `sd_ptr` (freed via
            // LocalFree below) and sets `existing_dacl` as a pointer into it.
            // Only the DACL is requested; other out-params receive null.
            let get_err = unsafe {
                GetNamedSecurityInfoW(
                    wcurrent.as_ptr(),
                    1, // SE_FILE_OBJECT
                    DACL_SECURITY_INFORMATION,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &raw mut existing_dacl,
                    std::ptr::null_mut(),
                    &raw mut sd_ptr,
                )
            };
            if get_err != 0 {
                // Free the security descriptor even on error if it was partially
                // allocated, then fail closed.
                if !sd_ptr.is_null() {
                    // SAFETY: `sd_ptr` was allocated by GetNamedSecurityInfoW.
                    unsafe { LocalFree(sd_ptr.cast()) };
                }
                return Err(spawn(format!(
                    "GetNamedSecurityInfoW failed for {} (error = {get_err}) while granting \
                     ancestor traversal; refusing",
                    current.display()
                )));
            }

            // Merge a GRANT_ACCESS FILE_TRAVERSE entry into the existing DACL.
            let entry = EXPLICIT_ACCESS_W {
                grfAccessPermissions: FILE_TRAVERSE,
                grfAccessMode: GRANT_ACCESS,
                grfInheritance: NO_INHERITANCE,
                Trustee: TRUSTEE_W {
                    pMultipleTrustee: std::ptr::null_mut(),
                    MultipleTrusteeOperation: 0,
                    TrusteeForm: TRUSTEE_IS_SID,
                    TrusteeType: TRUSTEE_IS_WELL_KNOWN_GROUP,
                    ptstrName: container_sid.cast(),
                },
            };
            let entries = [entry];
            let mut new_acl: *mut ACL = std::ptr::null_mut();
            // SAFETY: one EXPLICIT_ACCESS entry in a live array; `container_sid`
            // outlives this call (owned by the caller); `existing_dacl` points into
            // `sd_ptr` which is still live. `SetEntriesInAclW` allocates `new_acl`
            // (freed via LocalFree below).
            let acl_err = unsafe {
                SetEntriesInAclW(
                    1,
                    entries.as_ptr(),
                    existing_dacl,
                    std::ptr::from_mut(&mut new_acl),
                )
            };
            // Release the security descriptor from GetNamedSecurityInfoW.
            if !sd_ptr.is_null() {
                // SAFETY: allocated by GetNamedSecurityInfoW.
                unsafe { LocalFree(sd_ptr.cast()) };
            }
            if acl_err != 0 || new_acl.is_null() {
                return Err(spawn(format!(
                    "SetEntriesInAclW failed for {} (error = {acl_err}) while granting ancestor \
                     traversal; refusing",
                    current.display()
                )));
            }

            // Apply the merged DACL.
            // SAFETY: `wcurrent` is a live NUL-terminated wide path; `new_acl` is
            // the ACL just built. Only the DACL is set.
            let set_err = unsafe {
                SetNamedSecurityInfoW(
                    wcurrent.as_ptr(),
                    1, // SE_FILE_OBJECT
                    DACL_SECURITY_INFORMATION,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    new_acl,
                    std::ptr::null_mut(),
                )
            };
            // Free the merged ACL regardless of outcome.
            // SAFETY: allocated by SetEntriesInAclW.
            unsafe { LocalFree(new_acl.cast()) };
            if set_err != 0 {
                return Err(spawn(format!(
                    "SetNamedSecurityInfoW failed for {} (status = {set_err}) while granting \
                     ancestor traversal; refusing",
                    current.display()
                )));
            }
        }
        Ok(())
    }

    /// Query the launcher process's own user SID into an owned buffer. The
    /// returned `Vec<u64>` holds a `TOKEN_USER` whose `User.Sid` points back into
    /// the same allocation, so the buffer must outlive every use of that pointer.
    /// `u64` elements give the allocation 8-byte alignment, which `TOKEN_USER`
    /// (containing a pointer) requires. Fail-closed: any failed Win32 call refuses.
    fn launcher_user_sid_buffer() -> Result<Vec<u64>, RunJailDefect> {
        let mut token: HANDLE = std::ptr::null_mut();
        // SAFETY: `GetCurrentProcess` is a pseudo-handle (never closed); we request
        // TOKEN_QUERY and receive an owned token handle in `token`.
        let opened = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) };
        if opened == 0 || token.is_null() {
            return Err(last_error_spawn(
                "OpenProcessToken failed for the launcher token",
            ));
        }
        let token = OwnedHandle(token);
        // First call: learn the required buffer length (expected to fail with
        // ERROR_INSUFFICIENT_BUFFER while writing `needed`).
        let mut needed: u32 = 0;
        // SAFETY: null buffer with zero length is the documented size-probe form;
        // `needed` receives the byte count.
        unsafe {
            GetTokenInformation(
                token.get(),
                TokenUser,
                std::ptr::null_mut(),
                0,
                &raw mut needed,
            );
        }
        if needed == 0 {
            return Err(last_error_spawn(
                "GetTokenInformation size probe reported zero length for the launcher token",
            ));
        }
        // Round the byte count up to whole `u64` words so the allocation is both
        // large enough and 8-byte aligned.
        let words = (needed as usize).div_ceil(std::mem::size_of::<u64>());
        let mut buffer: Vec<u64> = vec![0u64; words];
        let mut written: u32 = 0;
        // SAFETY: `buffer` is at least `needed` bytes and 8-byte aligned;
        // `GetTokenInformation` fills a `TOKEN_USER` (its embedded SID points
        // within the same allocation).
        let ok = unsafe {
            GetTokenInformation(
                token.get(),
                TokenUser,
                buffer.as_mut_ptr().cast(),
                needed,
                &raw mut written,
            )
        };
        if ok == 0 {
            return Err(last_error_spawn(
                "GetTokenInformation failed to read the launcher user SID",
            ));
        }
        Ok(buffer)
    }

    /// Establish, at the boundary, that `path` lives on a volume whose filesystem
    /// persists and enforces DACLs (`FILE_PERSISTENT_ACLS`) — the precondition the
    /// ACL-mediated [`acl_path_for_container`] boundary silently depends on.
    ///
    /// On a volume WITHOUT that bit (FAT/exFAT — a USB stick, a redirected
    /// `%TEMP%`, some network shares), `SetNamedSecurityInfoW` returns success
    /// while persisting/enforcing NOTHING, so the ACL would be a no-op and the
    /// sandboxed app would run with the filesystem UNCONFINED even though the admit
    /// path already trusted it. Probe once with `GetVolumeInformationW`, parse the
    /// flags through the pure [`super::volume_flags_confine_filesystem`], and refuse
    /// (fail closed) when the bit is absent. Any probe failure also refuses.
    fn probe_volume_persists_acls(path: &Path) -> Result<(), RunJailDefect> {
        const MAX_PATH_WCHARS: u32 = 260;
        // `GetVolumeInformationW` needs a volume root, not an arbitrary path;
        // resolve the mount point of `path` first.
        let wpath = wide(path.as_os_str());
        let mut root: Vec<u16> = vec![0u16; MAX_PATH_WCHARS as usize];
        // SAFETY: `wpath` is a live NUL-terminated wide path; `root` is a
        // `MAX_PATH_WCHARS`-element buffer receiving the NUL-terminated volume
        // mount point.
        let got_root =
            unsafe { GetVolumePathNameW(wpath.as_ptr(), root.as_mut_ptr(), MAX_PATH_WCHARS) };
        if got_root == 0 {
            return Err(last_error_spawn(&format!(
                "GetVolumePathNameW failed for {}; refusing to run without proving the volume \
                 persists ACLs",
                path.display()
            )));
        }
        let mut fs_flags: u32 = 0;
        // SAFETY: `root` is a live NUL-terminated wide root path; we pass null for
        // the name/serial/component outputs we do not need and a live `fs_flags`
        // out-param; no volume/filesystem name buffers are requested.
        let got_info = unsafe {
            GetVolumeInformationW(
                root.as_ptr(),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &raw mut fs_flags,
                std::ptr::null_mut(),
                0,
            )
        };
        if got_info == 0 {
            return Err(last_error_spawn(&format!(
                "GetVolumeInformationW failed for {}; refusing to run without proving the volume \
                 persists ACLs",
                path.display()
            )));
        }
        if super::volume_flags_confine_filesystem(fs_flags) {
            Ok(())
        } else {
            Err(spawn(format!(
                "the volume backing {} lacks FILE_PERSISTENT_ACLS (flags = {fs_flags:#x}); its \
                 ACLs are not enforced, so the filesystem boundary would be a no-op — refusing to \
                 run",
                path.display()
            )))
        }
    }

    /// The scrubbed environment as a doubly-NUL-terminated UTF-16 block for
    /// `CreateProcess` `lpEnvironment` (with `CREATE_UNICODE_ENVIRONMENT`).
    fn env_block_utf16(
        profile: &SandboxProfile,
        scoped_tmp: &Path,
        host_env: &dyn Fn(&str) -> Option<OsString>,
    ) -> Vec<u16> {
        // The scrub + the block layout (sorted, doubly-NUL-terminated, no empty-name
        // entries) is one host-independent source: `windows_scrubbed_env` +
        // `env_block_from_pairs`. Unit tests on any host assert the exact bytes this
        // hands to `CreateProcessW`, so the arm cannot drift from what is proven.
        super::env_block_from_pairs(&windows_scrubbed_env(profile, scoped_tmp, host_env))
    }

    /// The created child's owned process + thread handles.
    struct Child {
        process: OwnedHandle,
        thread: OwnedHandle,
    }

    /// CreateProcess suspended with the AppContainer security-capabilities
    /// attribute and the scrubbed environment. The command line is built from the
    /// app path + args with proper quoting; there is no shell.
    ///
    /// `current_dir` is the child's `lpCurrentDirectory`. It MUST be a directory the
    /// AppContainer token can access — otherwise `CreateProcessW` fails resolving
    /// the current directory with `ERROR_ENVVAR_NOT_FOUND` (203) before the child
    /// starts. The caller passes the always-ACLed scratch (never the working tree,
    /// which is unreachable to the container when the filesystem axis is withheld).
    /// The CWD is not a capability: the child can still reach only what is ACLed to
    /// the container SID, so pointing it at the scratch neither grants nor widens
    /// any axis.
    fn create_suspended_appcontainer_process(
        app: &Path,
        app_args: &[OsString],
        current_dir: &Path,
        container: &AppContainer,
        capabilities: &mut CapabilitySids,
        env_block: &[u16],
    ) -> Result<Child, RunJailDefect> {
        // The attribute list carrying the security-capabilities (AppContainer)
        // attribute.
        let mut attr_size: usize = 0;
        // SAFETY: first call with a null list and a zeroed size queries the required
        // size (documented pattern); it returns 0 with ERROR_INSUFFICIENT_BUFFER.
        unsafe {
            InitializeProcThreadAttributeList(
                std::ptr::null_mut(),
                1,
                0,
                std::ptr::from_mut(&mut attr_size),
            );
        }
        if attr_size == 0 {
            return Err(spawn("InitializeProcThreadAttributeList sizing failed"));
        }
        let mut attr_buf: Vec<u8> = vec![0u8; attr_size];
        let attr_list = attr_buf.as_mut_ptr().cast();
        // SAFETY: `attr_buf` is `attr_size` bytes; initialises the list for 1 attr.
        let ok = unsafe {
            InitializeProcThreadAttributeList(attr_list, 1, 0, std::ptr::from_mut(&mut attr_size))
        };
        if ok == 0 {
            return Err(last_error_spawn("InitializeProcThreadAttributeList failed"));
        }
        // The list must be deleted on every path once initialised.
        let _attr_guard = AttrListGuard(attr_list);

        let mut sec_caps = capabilities.security_capabilities(container.sid());
        // SAFETY: `attr_list` is initialised; `sec_caps` is a live struct that
        // outlives the CreateProcess call (kept in this scope), pointed at by the
        // attribute. The size is that struct's size.
        let ok = unsafe {
            UpdateProcThreadAttribute(
                attr_list,
                0,
                PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
                std::ptr::from_mut(&mut sec_caps).cast(),
                std::mem::size_of::<SECURITY_CAPABILITIES>(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(last_error_spawn("UpdateProcThreadAttribute failed"));
        }

        let mut si: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
        si.StartupInfo.cb = size_u32::<STARTUPINFOEXW>();
        si.lpAttributeList = attr_list;

        let mut cmdline = build_command_line(app, app_args);
        let mut wdir = wide(current_dir.as_os_str());
        // The env block is `*const c_void` (cast from the u16 slice).
        let env_ptr = env_block.as_ptr().cast::<std::ffi::c_void>().cast_mut();

        let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: `cmdline` is a mutable NUL-terminated wide buffer (CreateProcessW
        // may write to it); `env_ptr` is the doubly-NUL wide env block;
        // `wdir` is a live wide path; `si` carries the initialised attribute list;
        // `pi` receives the process/thread handles. CREATE_SUSPENDED so the child
        // does not run before it is assigned to the job.
        let ok = unsafe {
            CreateProcessW(
                std::ptr::null(),
                cmdline.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0, // do not inherit handles
                CREATE_SUSPENDED | EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
                env_ptr,
                wdir.as_mut_ptr(),
                std::ptr::from_mut(&mut si).cast(),
                std::ptr::from_mut(&mut pi),
            )
        };
        if ok == 0 {
            return Err(last_error_spawn("CreateProcessW (AppContainer) failed"));
        }
        Ok(Child {
            process: OwnedHandle(pi.hProcess),
            thread: OwnedHandle(pi.hThread),
        })
    }

    /// A proc-thread attribute list deleted on drop.
    struct AttrListGuard(*mut std::ffi::c_void);

    impl Drop for AttrListGuard {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: `self.0` is an initialised attribute list this guard owns.
                unsafe {
                    DeleteProcThreadAttributeList(self.0);
                }
            }
        }
    }

    /// Build the child command line: the app path plus each arg, each token quoted
    /// per the Win32 command-line convention. There is NO shell — this is the
    /// argument vector CreateProcess parses, so the shell-injection class does not
    /// exist.
    fn build_command_line(app: &Path, app_args: &[OsString]) -> Vec<u16> {
        let mut s = OsString::new();
        push_quoted(&mut s, app.as_os_str());
        for arg in app_args {
            s.push(OsStr::new(" "));
            push_quoted(&mut s, arg);
        }
        wide(&s)
    }

    /// Append `arg` to `out` quoted per the CommandLineToArgvW rules (wrap in
    /// double quotes, backslash-escape embedded quotes and trailing backslashes).
    fn push_quoted(out: &mut OsString, arg: &OsStr) {
        out.push(OsStr::new("\""));
        let text = arg.to_string_lossy();
        let mut backslashes = 0usize;
        let mut escaped = String::new();
        for ch in text.chars() {
            match ch {
                '\\' => {
                    backslashes += 1;
                    escaped.push('\\');
                }
                '"' => {
                    // Double the run of backslashes preceding a quote, then escape
                    // the quote itself.
                    for _ in 0..backslashes {
                        escaped.push('\\');
                    }
                    backslashes = 0;
                    escaped.push('\\');
                    escaped.push('"');
                }
                other => {
                    backslashes = 0;
                    escaped.push(other);
                }
            }
        }
        // Double a trailing backslash run so the closing quote is not escaped.
        for _ in 0..backslashes {
            escaped.push('\\');
        }
        out.push(OsStr::new(&escaped));
        out.push(OsStr::new("\""));
    }

    fn assign_to_job(job: HANDLE, process: HANDLE) -> Result<(), RunJailDefect> {
        // SAFETY: both are live handles owned by the caller; assigning the process
        // to the job before resume is the documented no-un-jobbed-instruction order.
        let ok = unsafe { AssignProcessToJobObject(job, process) };
        if ok == 0 {
            return Err(last_error_spawn("AssignProcessToJobObject failed"));
        }
        Ok(())
    }

    fn resume(thread: HANDLE) -> Result<(), RunJailDefect> {
        // SAFETY: `thread` is the live suspended main thread handle owned by the
        // caller; resuming it starts the (now jobbed, tokened) child.
        let prev = unsafe { ResumeThread(thread) };
        if prev == u32::MAX {
            return Err(last_error_spawn("ResumeThread failed"));
        }
        Ok(())
    }

    /// Wait for the child and read its exit code. The job handle must stay open in
    /// the caller across this wait so KILL_ON_JOB_CLOSE holds for the child's life.
    fn wait_and_exit_code(process: HANDLE) -> Result<u32, RunJailDefect> {
        // SAFETY: `process` is a live handle owned by the caller; an infinite wait
        // blocks until the child exits.
        let w = unsafe { WaitForSingleObject(process, u32::MAX) };
        if w != WAIT_OBJECT_0 {
            return Err(last_error_spawn(
                "WaitForSingleObject on the jailed child failed",
            ));
        }
        let mut code: u32 = 0;
        // SAFETY: `process` is a live handle; `code` receives the exit code.
        let ok = unsafe { GetExitCodeProcess(process, std::ptr::from_mut(&mut code)) };
        if ok == 0 {
            return Err(last_error_spawn("GetExitCodeProcess failed"));
        }
        Ok(code)
    }

    /// The Job Object active-process cap for a profile: 1 when subprocess is
    /// withheld (only the app itself), else the profile's proc cap (min 1).
    /// Extracted so the cap policy is unit-testable without a live Job Object.
    fn active_process_cap(profile: &SandboxProfile) -> u32 {
        if profile.subprocess {
            u32::try_from(profile.limits.proc_cap)
                .unwrap_or(u32::MAX)
                .max(1)
        } else {
            1
        }
    }

    #[cfg(test)]
    mod tests {
        use super::super::{RunResourceLimits, SandboxProfile};
        use super::*;

        fn profile_with_env(names: &[&str]) -> SandboxProfile {
            SandboxProfile {
                env_allowlist: names.iter().map(|s| (*s).to_owned()).collect(),
                ..SandboxProfile::maximally_isolated()
            }
        }

        #[test]
        fn env_block_utf16_matches_the_shared_host_independent_builder() {
            // The Windows arm's `env_block_utf16` is a thin wrapper: it must produce
            // exactly what the host-independent `windows_scrubbed_env` +
            // `env_block_from_pairs` (asserted by the outer module's tests on every
            // host) produce, so the bytes proven there are the bytes handed to
            // `CreateProcessW`. This guards the SSOT on Windows too — a wide-string
            // divergence (a non-UTF-8 path unit) would surface as a byte mismatch.
            let profile = profile_with_env(&["ALLOWED"]);
            let host = |k: &str| match k {
                "ALLOWED" => Some(OsString::from("yes")),
                "SECRET" => Some(OsString::from("leak")),
                "SystemRoot" => Some(OsString::from("C:\\Windows")),
                _ => None,
            };
            let via_arm = env_block_utf16(&profile, Path::new("C:\\scratch"), &host);
            let via_shared = super::super::env_block_from_pairs(&windows_scrubbed_env(
                &profile,
                Path::new("C:\\scratch"),
                &host,
            ));
            assert_eq!(
                via_arm, via_shared,
                "arm must not drift from the SSOT block"
            );
        }

        #[test]
        fn command_line_quotes_each_arg_and_has_no_shell() {
            let cmd = build_command_line(
                Path::new("C:\\apps\\my app.exe"),
                &[OsString::from("plain"), OsString::from("has space")],
            );
            let s = String::from_utf16_lossy(&cmd);
            // The app path with a space is quoted.
            assert!(s.starts_with("\"C:\\apps\\my app.exe\""), "{s:?}");
            assert!(s.contains("\"plain\""), "{s:?}");
            assert!(s.contains("\"has space\""), "{s:?}");
            // No shell metacharacters are interpreted — it is a direct argv.
            assert!(!s.contains("cmd.exe"), "{s:?}");
            assert!(!s.contains(" /c "), "{s:?}");
        }

        #[test]
        fn embedded_quotes_are_escaped() {
            let mut out = OsString::new();
            push_quoted(&mut out, OsStr::new("a\"b"));
            let s = out.to_string_lossy();
            assert_eq!(s, "\"a\\\"b\"", "{s:?}");
        }

        #[test]
        fn active_process_cap_is_one_when_subprocess_withheld() {
            // A pure profile (subprocess absent) must cap the job at 1 process, so
            // no child can spawn.
            let withheld = SandboxProfile::maximally_isolated();
            assert!(!withheld.subprocess);
            let granted = SandboxProfile {
                subprocess: true,
                limits: RunResourceLimits {
                    proc_cap: 8,
                    ..RunResourceLimits::default()
                },
                ..SandboxProfile::maximally_isolated()
            };
            assert_eq!(active_process_cap(&withheld), 1);
            assert_eq!(active_process_cap(&granted), 8);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(caps: &[Capability]) -> BTreeSet<Capability> {
        caps.iter().copied().collect()
    }

    // The volume-capability decision the Windows run-jail arm uses to keep its
    // always-confined `Filesystem` claim honest. These exercise the pure
    // `volume_flags_confine_filesystem` on the raw filesystem-flags value, so they
    // run on ANY host — no self-hosted FAT/exFAT runner is needed for the negative
    // proof. The Windows arm's `probe_volume_persists_acls` feeds
    // `GetVolumeInformationW`'s flags straight into this function and fails closed
    // on `false`.

    #[test]
    fn volume_without_persistent_acls_refuses() {
        // FAT/exFAT-style flags: the FILE_PERSISTENT_ACLS bit is clear. Even with
        // other capability bits set (case-preserving, unicode-on-disk), the
        // decision is refuse — the ACL boundary would be a no-op there.
        let no_acls = 0x0000_0001 | 0x0000_0002 | 0x0000_0004; // not FILE_PERSISTENT_ACLS
        assert!(
            !volume_flags_confine_filesystem(no_acls),
            "a volume without FILE_PERSISTENT_ACLS must refuse (flags = {no_acls:#x})"
        );
        // Exactly zero flags also refuses.
        assert!(!volume_flags_confine_filesystem(0));
    }

    #[test]
    fn volume_with_persistent_acls_proceeds() {
        // NTFS-style flags: the FILE_PERSISTENT_ACLS bit is set.
        assert!(volume_flags_confine_filesystem(FILE_PERSISTENT_ACLS_FLAG));
        // Set alongside unrelated capability bits, it still proceeds.
        let ntfs_like = FILE_PERSISTENT_ACLS_FLAG | 0x0000_0001 | 0x0000_0002 | 0x0010_0000;
        assert!(volume_flags_confine_filesystem(ntfs_like));
    }

    #[test]
    fn persistent_acls_flag_is_the_win32_bit() {
        // The pure flag value must equal the documented Win32 FILE_PERSISTENT_ACLS
        // (0x8). On Windows a `const _` assertion additionally ties it to
        // `windows-sys`; this keeps the value pinned on every host.
        assert_eq!(FILE_PERSISTENT_ACLS_FLAG, 0x0000_0008);
    }

    #[test]
    fn empty_set_lowers_to_maximally_isolated() {
        let p = profile_from_capabilities(
            &BTreeSet::new(),
            &BTreeSet::new(),
            DatabaseAxis::NotApplicable,
            &[],
        )
        .expect("empty set lowers");
        assert_eq!(p, SandboxProfile::maximally_isolated());
        assert!(!p.network);
        assert_eq!(p.filesystem, FilesystemScope::Isolated);
        assert!(!p.subprocess);
        assert!(p.env_allowlist.is_empty());
    }

    #[test]
    fn network_capability_grants_network_only() {
        let p = profile_from_capabilities(
            &set(&[Capability::Network]),
            &BTreeSet::new(),
            DatabaseAxis::NotApplicable,
            &[],
        )
        .expect("lowers");
        assert!(p.network);
        assert_eq!(p.filesystem, FilesystemScope::Isolated);
        assert!(!p.subprocess);
    }

    #[test]
    fn the_union_of_inferred_and_declared_is_used() {
        // inferred = {filesystem}, declared = {network}: both must be granted
        // (no false-deny — a declared axis is relaxed even if not inferred).
        let p = profile_from_capabilities(
            &set(&[Capability::Filesystem]),
            &set(&[Capability::Network]),
            DatabaseAxis::NotApplicable,
            &[],
        )
        .expect("lowers");
        assert!(p.network);
        assert_eq!(p.filesystem, FilesystemScope::WorkingTreeReadWrite);
    }

    #[test]
    fn database_lowers_to_network_or_filesystem_per_driver() {
        let net = profile_from_capabilities(
            &set(&[Capability::Database]),
            &BTreeSet::new(),
            DatabaseAxis::Network,
            &[],
        )
        .expect("lowers");
        assert!(net.network);
        assert_eq!(net.filesystem, FilesystemScope::Isolated);

        let file = profile_from_capabilities(
            &set(&[Capability::Database]),
            &BTreeSet::new(),
            DatabaseAxis::Filesystem,
            &[],
        )
        .expect("lowers");
        assert!(!file.network);
        assert_eq!(file.filesystem, FilesystemScope::WorkingTreeReadWrite);
    }

    #[test]
    fn database_with_an_unknown_driver_fails_closed() {
        let r = profile_from_capabilities(
            &set(&[Capability::Database]),
            &BTreeSet::new(),
            DatabaseAxis::NotApplicable,
            &[],
        );
        assert_eq!(r, Err(ProfileError::UnknownDatabaseDriver));
    }

    #[test]
    fn env_capability_re_exports_the_named_allowlist() {
        let p = profile_from_capabilities(
            &set(&[Capability::Env]),
            &BTreeSet::new(),
            DatabaseAxis::NotApplicable,
            &["DATABASE_URL".to_owned(), "API_KEY".to_owned()],
        )
        .expect("lowers");
        assert_eq!(p.env_allowlist, vec!["DATABASE_URL", "API_KEY"]);
    }

    #[test]
    fn clock_and_random_carry_no_control() {
        let p = profile_from_capabilities(
            &set(&[Capability::Clock, Capability::Random]),
            &BTreeSet::new(),
            DatabaseAxis::NotApplicable,
            &[],
        )
        .expect("lowers");
        // No axis is opened by clock/random.
        assert_eq!(p, SandboxProfile::maximally_isolated());
    }

    #[test]
    fn native_ffi_alone_opens_no_control() {
        let p = profile_from_capabilities(
            &set(&[Capability::NativeFfi]),
            &BTreeSet::new(),
            DatabaseAxis::NotApplicable,
            &[],
        )
        .expect("lowers");
        assert_eq!(p, SandboxProfile::maximally_isolated());
    }

    #[test]
    fn floor_comparison_refuses_a_widened_profile() {
        let floor = SandboxProfile::maximally_isolated();
        // A profile that grants network is weaker than a maximally-isolated
        // floor → refuse.
        let widened = SandboxProfile {
            network: true,
            ..SandboxProfile::maximally_isolated()
        };
        assert!(!widened.is_at_least_as_isolated_as(&floor));
        // The floor itself is at least as isolated as itself.
        assert!(floor.is_at_least_as_isolated_as(&floor));
    }

    #[test]
    fn floor_comparison_allows_a_tighter_profile() {
        // floor grants network; a profile that does NOT grant it is tighter →
        // allowed (more isolation than the floor is never a violation).
        let floor = SandboxProfile {
            network: true,
            filesystem: FilesystemScope::WorkingTreeReadWrite,
            ..SandboxProfile::maximally_isolated()
        };
        let tighter = SandboxProfile::maximally_isolated();
        assert!(tighter.is_at_least_as_isolated_as(&floor));
    }

    #[test]
    fn floor_comparison_checks_env_var_subset() {
        let floor = SandboxProfile {
            env_allowlist: vec!["A".to_owned()],
            ..SandboxProfile::maximally_isolated()
        };
        // Granting an env var the floor does not is a violation.
        let extra_env = SandboxProfile {
            env_allowlist: vec!["A".to_owned(), "SECRET".to_owned()],
            ..SandboxProfile::maximally_isolated()
        };
        assert!(!extra_env.is_at_least_as_isolated_as(&floor));
        // A subset is fine.
        let subset = SandboxProfile {
            env_allowlist: vec!["A".to_owned()],
            ..SandboxProfile::maximally_isolated()
        };
        assert!(subset.is_at_least_as_isolated_as(&floor));
    }

    fn tools() -> RunJailTools {
        RunJailTools {
            bwrap: PathBuf::from("/usr/bin/bwrap"),
            prlimit: PathBuf::from("/usr/bin/prlimit"),
            timeout: Some(PathBuf::from("/usr/bin/timeout")),
        }
    }

    fn rendered(profile: &SandboxProfile, seccomp_fd: Option<i32>) -> Vec<String> {
        let no_env = |_: &str| None;
        run_jail_argv(
            &tools(),
            profile,
            Path::new("/work/tmp-1"),
            Path::new("/work/tree"),
            &[],
            seccomp_fd,
            &no_env,
            &[OsString::from("/work/tree/target/debug/ipe-app")],
        )
        .into_iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
    }

    #[test]
    fn maximally_isolated_argv_denies_net_masks_proc_and_scrubs_env() {
        let argv = rendered(&SandboxProfile::maximally_isolated(), Some(10));
        let joined = argv.join(" ");
        // No wall clock (default RunResourceLimits has wall_secs = None), so
        // bwrap is the first program.
        assert!(joined.starts_with("/usr/bin/bwrap"), "{joined}");
        // Net unshared (network absent), namespaces fresh, env scrubbed.
        for flag in [
            "--unshare-net",
            "--unshare-pid",
            "--unshare-ipc",
            "--clearenv",
        ] {
            assert!(argv.contains(&flag.to_owned()), "missing {flag}: {joined}");
        }
        // Fresh proc mask AFTER the ro-bind of root — the ordering that masks
        // the host /proc.
        let ro_root = joined.find("--ro-bind / /").expect("ro-bind root");
        let proc = joined.find("--proc /proc").expect("proc mask");
        assert!(
            proc > ro_root,
            "proc mask must follow the ro-bind: {joined}"
        );
        // Seccomp filter attached.
        assert!(joined.contains("--seccomp 10"), "{joined}");
        // Resource caps then the payload, no shell.
        assert!(joined.contains("-- /usr/bin/prlimit --as="), "{joined}");
        assert!(
            joined.ends_with("-- /work/tree/target/debug/ipe-app"),
            "{joined}"
        );
        assert!(!joined.contains("sh -c"), "{joined}");
    }

    #[test]
    fn app_delivery_emits_perms_file_after_mounts_before_payload() {
        let no_env = |_: &str| None;
        let dest = Path::new("/work/tmp-1/ipe-app");
        let argv: Vec<String> = run_jail_argv_with_delivery(
            &tools(),
            &SandboxProfile::maximally_isolated(),
            Path::new("/work/tmp-1"),
            Path::new("/work/tree"),
            &[],
            Some(10),
            Some((7, dest)),
            &no_env,
            &[OsString::from("/work/tmp-1/ipe-app")],
        )
        .into_iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
        let joined = argv.join(" ");
        // The sealed-fd delivery pair: owner-execute perms then a copy from the
        // inherited fd to the in-jail app path.
        assert!(
            joined.contains("--perms 0700 --file 7 /work/tmp-1/ipe-app"),
            "delivery pair missing: {joined}"
        );
        // It must come AFTER the writable bind (so the dest parent exists) and
        // BEFORE the `-- /usr/bin/prlimit` payload separator.
        let bind = joined
            .find("--bind /work/tmp-1 /work/tmp-1")
            .expect("scratch bind");
        let file = joined.find("--file 7").expect("delivery");
        let payload = joined.find("-- /usr/bin/prlimit").expect("payload sep");
        assert!(
            file > bind,
            "delivery must follow the scratch bind: {joined}"
        );
        assert!(
            file < payload,
            "delivery must precede the payload: {joined}"
        );
        // The payload execs the delivered in-jail path, not any host path.
        assert!(
            joined.ends_with("-- /work/tmp-1/ipe-app"),
            "payload must exec the delivered path: {joined}"
        );
    }

    #[test]
    fn no_delivery_emits_no_file_op() {
        // The default `run_jail_argv` (no delivery) must not emit `--file`.
        let joined = rendered(&SandboxProfile::maximally_isolated(), Some(10)).join(" ");
        assert!(!joined.contains("--file"), "unexpected --file: {joined}");
        assert!(!joined.contains("--perms"), "unexpected --perms: {joined}");
    }

    #[test]
    fn network_granted_shares_the_net_namespace() {
        let p = SandboxProfile {
            network: true,
            ..SandboxProfile::maximally_isolated()
        };
        let argv = rendered(&p, None);
        assert!(
            !argv.contains(&"--unshare-net".to_owned()),
            "network granted must NOT unshare net: {}",
            argv.join(" ")
        );
        // IPC is still unshared unconditionally.
        assert!(argv.contains(&"--unshare-ipc".to_owned()));
    }

    #[test]
    fn filesystem_granted_binds_the_working_tree_read_write() {
        let p = SandboxProfile {
            filesystem: FilesystemScope::WorkingTreeReadWrite,
            ..SandboxProfile::maximally_isolated()
        };
        let joined = rendered(&p, None).join(" ");
        assert!(
            joined.contains("--bind /work/tree /work/tree"),
            "working tree not bound rw: {joined}"
        );
        assert!(joined.contains("--chdir /work/tree"), "{joined}");
    }

    #[test]
    fn env_allowlist_re_exports_only_named_present_vars() {
        let p = SandboxProfile {
            env_allowlist: vec!["DATABASE_URL".to_owned(), "ABSENT".to_owned()],
            ..SandboxProfile::maximally_isolated()
        };
        let host = |k: &str| {
            if k == "DATABASE_URL" {
                Some(OsString::from("postgres://x"))
            } else {
                None
            }
        };
        let argv = run_jail_argv(
            &tools(),
            &p,
            Path::new("/work/tmp-1"),
            Path::new("/work/tree"),
            &[],
            None,
            &host,
            &[OsString::from("app")],
        );
        let joined: Vec<String> = argv
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let s = joined.join(" ");
        assert!(s.contains("--setenv DATABASE_URL postgres://x"), "{s}");
        // An absent named var is simply not re-exported.
        assert!(!s.contains("ABSENT"), "{s}");
    }

    #[test]
    fn scan_capfloor_finds_the_embedded_marker() {
        let p = SandboxProfile {
            network: true,
            filesystem: FilesystemScope::WorkingTreeReadWrite,
            env_allowlist: vec!["A".to_owned(), "B".to_owned()],
            subprocess: false,
            limits: RunResourceLimits::default(),
        };
        // Simulate a binary: arbitrary bytes, the floor line in .rodata, more bytes.
        let mut buf: Vec<u8> = vec![0xde, 0xad, 0xbe, 0xef];
        buf.extend_from_slice(p.to_capfloor_line().as_bytes());
        buf.push(0); // NUL-terminated as in .rodata
        buf.extend_from_slice(&[0x11, 0x22]);
        let floor = scan_capfloor(&buf).expect("found");
        assert!(floor.network);
        assert_eq!(floor.filesystem, FilesystemScope::WorkingTreeReadWrite);
        assert_eq!(floor.env_allowlist, vec!["A".to_owned(), "B".to_owned()]);
    }

    #[test]
    fn scan_capfloor_intersects_env_names_of_multiple_copies() {
        // A legitimate floor grants {A, B}; an appended forged copy grants {B, C}
        // (same count, swapped name). The strictest merged floor is the name-set
        // intersection {B}, so the forged copy cannot smuggle C into the ceiling.
        let legit = SandboxProfile {
            env_allowlist: vec!["A".to_owned(), "B".to_owned()],
            ..SandboxProfile::maximally_isolated()
        };
        let forged = SandboxProfile {
            env_allowlist: vec!["B".to_owned(), "C".to_owned()],
            ..SandboxProfile::maximally_isolated()
        };
        let mut buf = Vec::new();
        buf.extend_from_slice(legit.to_capfloor_line().as_bytes());
        buf.push(b'\n');
        buf.extend_from_slice(forged.to_capfloor_line().as_bytes());
        buf.push(0);
        let floor = scan_capfloor(&buf).expect("found");
        assert_eq!(floor.env_allowlist, vec!["B".to_owned()]);
        // A profile granting C is refused: C is not in the intersected floor.
        let wants_c = SandboxProfile {
            env_allowlist: vec!["C".to_owned()],
            ..SandboxProfile::maximally_isolated()
        };
        assert!(!wants_c.satisfies_capfloor(&floor));
    }

    #[test]
    fn scan_capfloor_takes_the_strictest_of_multiple_copies() {
        // A legitimate strict floor, plus a forged permissive one appended by an
        // attacker: the strictest (least-granting) must win so the forgery cannot
        // relax the ceiling.
        let strict = SandboxProfile::maximally_isolated();
        let forged = SandboxProfile {
            network: true,
            subprocess: true,
            filesystem: FilesystemScope::WorkingTreeReadWrite,
            ..SandboxProfile::maximally_isolated()
        };
        let mut buf = Vec::new();
        buf.extend_from_slice(strict.to_capfloor_line().as_bytes());
        buf.push(b'\n');
        buf.extend_from_slice(forged.to_capfloor_line().as_bytes());
        buf.push(0);
        let floor = scan_capfloor(&buf).expect("found");
        // The strict floor wins: no axis granted.
        assert!(!floor.network);
        assert!(!floor.subprocess);
        assert_eq!(floor.filesystem, FilesystemScope::Isolated);
    }

    #[test]
    fn scan_capfloor_absent_is_none() {
        assert_eq!(scan_capfloor(b"no floor here"), None);
    }

    #[test]
    fn profile_string_round_trips() {
        let p = SandboxProfile {
            network: true,
            filesystem: FilesystemScope::WorkingTreeReadWrite,
            env_allowlist: vec!["DATABASE_URL".to_owned(), "API_KEY".to_owned()],
            subprocess: true,
            limits: RunResourceLimits::default(),
        };
        let text = p.to_profile_string();
        let parsed = parse_profile(&text).expect("round-trips");
        assert_eq!(parsed, p);
    }

    #[test]
    fn parse_profile_rejects_unknown_keys_and_missing_fields() {
        // Unknown key → refuse.
        assert!(parse_profile("ipe-profile 1\nnetwork true\nbogus x\n").is_err());
        // Missing header → refuse.
        assert!(parse_profile("network true\n").is_err());
        // Missing required field → refuse.
        assert!(parse_profile("ipe-profile 1\nnetwork true\n").is_err());
        // Malformed boolean → refuse.
        assert!(
            parse_profile("ipe-profile 1\nnetwork yes\nfilesystem isolated\nsubprocess false\n")
                .is_err()
        );
    }

    #[test]
    fn capfloor_line_round_trips_axes_and_env_names() {
        let p = SandboxProfile {
            network: true,
            filesystem: FilesystemScope::WorkingTreeReadWrite,
            // Out of order on purpose: the line is sorted, so the round-trip is
            // canonical regardless of the source order.
            env_allowlist: vec!["B".to_owned(), "A".to_owned()],
            subprocess: false,
            limits: RunResourceLimits::default(),
        };
        let line = p.to_capfloor_line();
        assert_eq!(line, "ipe-capfloor 1 net=true fs=rw sub=false env=A,B");
        let floor = parse_capfloor(&line).expect("round-trips");
        assert!(floor.network);
        assert_eq!(floor.filesystem, FilesystemScope::WorkingTreeReadWrite);
        assert!(!floor.subprocess);
        // The names round-trip exactly (sorted), not merely their count.
        assert_eq!(floor.env_allowlist, vec!["A".to_owned(), "B".to_owned()]);
    }

    #[test]
    fn capfloor_line_empty_env_round_trips() {
        let p = SandboxProfile::maximally_isolated();
        let line = p.to_capfloor_line();
        assert_eq!(line, "ipe-capfloor 1 net=false fs=isolated sub=false env=");
        let floor = parse_capfloor(&line).expect("round-trips");
        assert!(floor.env_allowlist.is_empty());
    }

    #[test]
    fn satisfies_capfloor_refuses_a_widened_profile() {
        // floor = maximally isolated; a profile granting network must be refused.
        let floor = parse_capfloor(&SandboxProfile::maximally_isolated().to_capfloor_line())
            .expect("floor");
        let widened = SandboxProfile {
            network: true,
            ..SandboxProfile::maximally_isolated()
        };
        assert!(!widened.satisfies_capfloor(&floor));
        assert!(SandboxProfile::maximally_isolated().satisfies_capfloor(&floor));
    }

    #[test]
    fn satisfies_capfloor_refuses_more_env_than_the_floor() {
        // floor grants 1 env var; a profile granting 2 exceeds it → refuse.
        let floor_profile = SandboxProfile {
            env_allowlist: vec!["A".to_owned()],
            ..SandboxProfile::maximally_isolated()
        };
        let floor = parse_capfloor(&floor_profile.to_capfloor_line()).expect("floor");
        let two_env = SandboxProfile {
            env_allowlist: vec!["A".to_owned(), "B".to_owned()],
            ..SandboxProfile::maximally_isolated()
        };
        assert!(!two_env.satisfies_capfloor(&floor));
        // A profile granting exactly the floor's named var is accepted.
        let same_env = SandboxProfile {
            env_allowlist: vec!["A".to_owned()],
            ..SandboxProfile::maximally_isolated()
        };
        assert!(same_env.satisfies_capfloor(&floor));
    }

    #[test]
    fn satisfies_capfloor_refuses_a_same_count_env_name_swap() {
        // The env-swap attack: the source proves it needs {PATH, HOME}, so the
        // floor records those two names. A doctored ipe.profile swaps in a
        // DIFFERENT pair of the SAME count ({AWS_SECRET_ACCESS_KEY,
        // SSH_AUTH_SOCK}). The count matches (2 == 2), so a count-only check would
        // pass; the name subset check must REFUSE, because neither swapped name is
        // in the floor.
        let floor_profile = SandboxProfile {
            env_allowlist: vec!["PATH".to_owned(), "HOME".to_owned()],
            ..SandboxProfile::maximally_isolated()
        };
        let floor = parse_capfloor(&floor_profile.to_capfloor_line()).expect("floor");
        let swapped = SandboxProfile {
            env_allowlist: vec![
                "AWS_SECRET_ACCESS_KEY".to_owned(),
                "SSH_AUTH_SOCK".to_owned(),
            ],
            ..SandboxProfile::maximally_isolated()
        };
        assert!(
            !swapped.satisfies_capfloor(&floor),
            "a same-count env name swap must be refused"
        );
        // A single swapped name (one legitimate, one smuggled) is also refused —
        // the smuggled one is not in the floor.
        let partial_swap = SandboxProfile {
            env_allowlist: vec!["PATH".to_owned(), "AWS_SECRET_ACCESS_KEY".to_owned()],
            ..SandboxProfile::maximally_isolated()
        };
        assert!(!partial_swap.satisfies_capfloor(&floor));
        // The legitimate subset (⊆ the floor's names) still passes.
        let legit = SandboxProfile {
            env_allowlist: vec!["HOME".to_owned()],
            ..SandboxProfile::maximally_isolated()
        };
        assert!(legit.satisfies_capfloor(&floor));
        // Granting exactly the floor's names passes.
        assert!(floor_profile.satisfies_capfloor(&floor));
    }

    #[test]
    fn parse_capfloor_refuses_a_malformed_env_name() {
        // A name with a stray comma (empty element) or a non-POSIX char cannot be
        // emitted by `to_capfloor_line`; if a tampered floor carries one, the
        // launcher must refuse rather than silently parse a smuggled separator.
        assert!(parse_capfloor("ipe-capfloor 1 net=false fs=isolated sub=false env=A,,B").is_err());
        assert!(parse_capfloor("ipe-capfloor 1 net=false fs=isolated sub=false env=,A").is_err());
        assert!(parse_capfloor("ipe-capfloor 1 net=false fs=isolated sub=false env=1BAD").is_err());
        assert!(parse_capfloor("ipe-capfloor 1 net=false fs=isolated sub=false env=A-B").is_err());
    }

    #[test]
    fn parse_capfloor_refuses_an_unreadable_floor() {
        assert!(parse_capfloor("garbage").is_err());
        assert!(parse_capfloor("ipe-capfloor 2 net=true").is_err());
        assert!(parse_capfloor("ipe-capfloor 1 fs=bogus").is_err());
    }

    #[test]
    fn platform_supports_jail_matches_the_compiled_in_jail_arm() {
        // The single-source guard: `platform_supports_jail()` returns exactly the
        // `on_jailed_target!` predicate (the value stamped onto `JAIL_COMPILED_IN`
        // by the same macro that selects the real `exec_in_run_jail` arm). This
        // spells that predicate independently and asserts equality, so a future
        // edit that flips one without the other fails this test.
        let compiled_in_here = cfg!(any(
            all(
                target_os = "linux",
                any(target_arch = "x86_64", target_arch = "aarch64")
            ),
            target_os = "macos",
            target_os = "windows"
        ));
        assert_eq!(
            platform_supports_jail(),
            compiled_in_here,
            "platform_supports_jail() must equal the compiled-in run-jail predicate"
        );
    }

    #[test]
    fn database_membership_is_derived_from_net_and_fs() {
        // Guardian Nit-1: `database` is confined iff BOTH axes it can lower into
        // (network for a TCP driver, filesystem for a file driver) are confined —
        // never a standalone asserted bit. This asserts the compiled-in
        // `CONFINED_AXES` on THIS host agrees with the `database_confined`
        // derivation, so a partial target that dropped net or fs could not keep
        // an over-claimed `database`.
        let axes = platform_confined_axes();
        let net = axes.contains(&Capability::Network);
        let fs = axes.contains(&Capability::Filesystem);
        let db = axes.contains(&Capability::Database);
        assert_eq!(
            db,
            database_confined(net, fs),
            "database membership must equal database_confined(net, fs): net={net}, fs={fs}"
        );
    }

    #[test]
    fn database_confined_requires_both_net_and_fs() {
        // The derivation itself: only both-confined yields a confined database.
        assert!(database_confined(true, true));
        assert!(!database_confined(true, false));
        assert!(!database_confined(false, true));
        assert!(!database_confined(false, false));
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn windows_is_a_jailed_target_with_the_partial_arm_axes() {
        // Windows has a real (non-stub) run-jail arm, so it is a jailed target.
        assert!(platform_supports_jail());
        // The compiled-in Windows arm establishes the Job Object (subprocess), the
        // launcher scrub (env), and AppContainer + an ACL scratch (filesystem +
        // network), and fails closed when AppContainer/ACL is unavailable — so it
        // lists those axes plus the whole-process-contained native-ffi and the
        // net+fs-derived database.
        let axes = platform_confined_axes();
        for cap in [
            Capability::Subprocess,
            Capability::Env,
            Capability::Filesystem,
            Capability::Network,
            Capability::NativeFfi,
            Capability::Database,
        ] {
            assert!(axes.contains(&cap), "Windows must confine {cap:?}");
        }
    }

    #[test]
    #[cfg(all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ))]
    fn linux_x86_64_is_a_jail_holds_target() {
        // On the Linux/x86_64 build host the run jail is compiled in, so the FFI
        // admit predicate must see a jail-holds target.
        assert!(platform_supports_jail());
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn macos_is_a_jail_holds_target() {
        // On macOS the sandbox-exec SBPL run-jail arm is compiled in, so the FFI
        // admit predicate flips to jail-holds.
        assert!(platform_supports_jail());
    }

    #[test]
    fn a_wall_clock_profile_wraps_in_timeout() {
        let p = SandboxProfile {
            limits: RunResourceLimits {
                wall_secs: Some(30),
                ..RunResourceLimits::default()
            },
            ..SandboxProfile::maximally_isolated()
        };
        let joined = rendered(&p, None).join(" ");
        assert!(
            joined.starts_with("/usr/bin/timeout --kill-after=5s 30 /usr/bin/bwrap"),
            "{joined}"
        );
    }

    // ── the Windows `CreateProcessW` environment block (host-independent) ──────────
    //
    // The Windows run-jail + build-jail both pass this UTF-16 block to
    // `CreateProcessW` as `lpEnvironment` under `CREATE_UNICODE_ENVIRONMENT`. A block
    // that is not sorted in Windows' uppercase-ordinal name order, holds an empty-name
    // entry, or is not doubly-NUL-terminated makes the child's CRT miss a variable it
    // needs (notably `SystemRoot`) and process creation fails with
    // `ERROR_ENVVAR_NOT_FOUND` (203) before the child runs. These assert the exact
    // bytes on ANY host, so the invariant the real windows-2022 runner depends on is
    // guarded even though the true E2E only runs there.

    fn env_profile(names: &[&str]) -> SandboxProfile {
        SandboxProfile {
            env_allowlist: names.iter().map(|n| (*n).to_owned()).collect(),
            ..SandboxProfile::maximally_isolated()
        }
    }

    /// Decode the block into its `NAME=VALUE` entries (split on the NUL separators,
    /// dropping the terminating empties), for order/content assertions.
    fn decode_block(block: &[u16]) -> Vec<String> {
        String::from_utf16_lossy(block)
            .split('\0')
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn env_block_is_doubly_nul_terminated_and_scrubs_non_allowlisted() {
        let profile = env_profile(&["ALLOWED"]);
        let host = |k: &str| match k {
            "ALLOWED" => Some(OsString::from("yes")),
            "SECRET" => Some(OsString::from("leak")),
            "SystemRoot" => Some(OsString::from("C:\\Windows")),
            _ => None,
        };
        let pairs = windows_scrubbed_env(&profile, Path::new("C:\\scratch"), &host);
        let block = env_block_from_pairs(&pairs);
        // Doubly-NUL terminated: the last entry's own NUL plus the block terminator.
        assert!(
            block.ends_with(&[0, 0]),
            "must be doubly-NUL-terminated (\\0\\0)"
        );
        let entries = decode_block(&block);
        assert!(
            entries.iter().any(|e| e == "ALLOWED=yes"),
            "allowlisted var survives: {entries:?}"
        );
        // A non-allowlisted host var must NOT reach the child.
        assert!(
            !entries
                .iter()
                .any(|e| e.contains("SECRET") || e.contains("leak")),
            "non-allowlisted var must be scrubbed: {entries:?}"
        );
        // The required AppContainer base is present.
        assert!(
            entries.iter().any(|e| e.starts_with("SystemRoot=")),
            "SystemRoot must be present for AppContainer: {entries:?}"
        );
    }

    #[test]
    fn env_block_names_are_sorted_in_windows_uppercase_ordinal_order() {
        // `_` (0x5F) sorts AFTER the letters under Windows' uppercase-ordinal
        // collation (`A`..=`Z` = 0x41..=0x5A) but BEFORE them if the block were
        // lowercased (`a`..=`z` = 0x61..=0x7A). `APP_KEY` vs `APPLE` distinguishes
        // the two orders: correct Windows order is `APPLE` before `APP_KEY`.
        let profile = env_profile(&["APP_KEY", "APPLE", "zeta"]);
        let host = |k: &str| match k {
            "APP_KEY" | "APPLE" | "zeta" => Some(OsString::from("v")),
            "SystemRoot" => Some(OsString::from("C:\\Windows")),
            _ => None,
        };
        let pairs = windows_scrubbed_env(&profile, Path::new("C:\\scratch"), &host);
        let names: Vec<String> = pairs
            .iter()
            .map(|(n, _)| n.to_string_lossy().into_owned())
            .collect();
        let apple = names.iter().position(|n| n == "APPLE");
        let app_key = names.iter().position(|n| n == "APP_KEY");
        assert!(
            apple < app_key,
            "uppercase-ordinal order puts APPLE before APP_KEY (got {names:?})"
        );
        // The whole block is sorted on the uppercase-ordinal key.
        let keys: Vec<String> = pairs
            .iter()
            .map(|(n, _)| env_name_collation_key(n))
            .collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(
            keys, sorted,
            "block must be name-sorted (uppercase ordinal)"
        );
    }

    #[test]
    fn env_block_has_no_empty_name_entry_even_when_an_empty_name_is_granted() {
        // An empty allowlist name can never form a valid `NAME=VALUE`; Windows rejects
        // a block containing one. Even if a profile carries an empty name it must be
        // dropped fail-closed, never emitted as a `=VALUE` entry.
        let profile = env_profile(&["", "REAL"]);
        let host = |k: &str| match k {
            "" => Some(OsString::from("wat")),
            "REAL" => Some(OsString::from("ok")),
            "SystemRoot" => Some(OsString::from("C:\\Windows")),
            _ => None,
        };
        let pairs = windows_scrubbed_env(&profile, Path::new("C:\\scratch"), &host);
        let block = env_block_from_pairs(&pairs);
        for entry in decode_block(&block) {
            assert!(
                !entry.starts_with('='),
                "no entry may have an empty name: {entry:?}"
            );
            assert!(
                entry.contains('='),
                "every entry is a NAME=VALUE pair: {entry:?}"
            );
        }
        assert!(
            decode_block(&block).iter().any(|e| e == "REAL=ok"),
            "the non-empty allowlisted var still survives"
        );
    }

    #[test]
    fn empty_pairs_still_yield_a_valid_empty_block() {
        // No pairs → a lone `\0\0`, the valid empty environment `CreateProcess`
        // expects (never a single NUL it would read past).
        let block = env_block_from_pairs(&[]);
        assert_eq!(block, vec![0u16, 0u16]);
    }

    #[test]
    fn an_allowlisted_name_colliding_case_insensitively_with_a_base_is_deduped() {
        // Windows environments are case-insensitive: a block holding two names that
        // fold to the same key is malformed and can 203. A profile can grant a name
        // that collides with a fixed base (`SystemRoot`), so the collision must
        // collapse to ONE entry — the base value, never the allowlist re-grant's.
        let profile = env_profile(&["systemroot"]);
        let host = |k: &str| match k {
            "SystemRoot" => Some(OsString::from("C:\\Windows")),
            "systemroot" => Some(OsString::from("C:\\Attacker")),
            _ => None,
        };
        let pairs = windows_scrubbed_env(&profile, Path::new("C:\\scratch"), &host);
        let system_roots: Vec<&OsString> = pairs
            .iter()
            .filter(|(n, _)| env_name_collation_key(n) == "SYSTEMROOT")
            .map(|(_, value)| value)
            .collect();
        assert_eq!(
            system_roots,
            vec![&OsString::from("C:\\Windows")],
            "a case-insensitive collision collapses to one entry, base value winning: {pairs:?}"
        );
    }

    #[test]
    fn a_value_containing_an_equals_sign_is_preserved_whole() {
        // A `=` in a VALUE is not a separator: Windows splits a block entry on the
        // FIRST `=`, so `NAME=a=b` is name `NAME`, value `a=b`. The builder must not
        // mangle it.
        let profile = env_profile(&["CONN"]);
        let host = |k: &str| match k {
            "CONN" => Some(OsString::from("k=v;x=y")),
            "SystemRoot" => Some(OsString::from("C:\\Windows")),
            _ => None,
        };
        let block = env_block_from_pairs(&windows_scrubbed_env(
            &profile,
            Path::new("C:\\scratch"),
            &host,
        ));
        assert!(
            decode_block(&block).iter().any(|e| e == "CONN=k=v;x=y"),
            "a `=` inside a value must survive whole: {:?}",
            decode_block(&block)
        );
    }
}
