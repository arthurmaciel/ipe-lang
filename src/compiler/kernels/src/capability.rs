//! The capability vocabulary: what a program is permitted to do on the
//! security-relevant axis.
//!
//! A capability is coarse (whole-capability) for v1: [`Capability::Network`] is
//! *any* network access, not per-host; [`Capability::Filesystem`] is *any* file
//! access, not per-path. Each stdlib kernel is tagged with the one capability it
//! exercises (or none) via [`crate::StdlibKernel::capability`]; a whole program's
//! capability set is the union over its transitively-reachable kernels. Finer
//! granularity (per-host, per-path) is a tracked follow-up.

/// What a program is permitted to do, on the security-relevant axis.
///
/// The axes a sandbox can isolate independently. A kernel maps to at most
/// one; a program's set is the union over its reachable kernels plus
/// [`Capability::NativeFfi`] when it crosses into `Rust.` code (and
/// additionally [`Capability::FfiRaw`] when a crossing rides an
/// author-asserted `Rust.Ffi.call` signature).
#[derive(
    Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum Capability {
    /// Outbound or inbound network access (HTTP client/server, WebSocket,
    /// email send).
    Network,
    /// Reading or writing the filesystem (files, directories, an `.env` or
    /// config file). Does not include database access — see [`Self::Database`].
    Filesystem,
    /// Structured database access (SQL queries, migrations, row decoders).
    ///
    /// Resolved by SP4 sandbox to filesystem or network per the ipe.toml driver.
    Database,
    /// Reading or writing process environment (environment variables, argv).
    Env,
    /// Spawning or controlling a child process.
    Subprocess,
    /// Reading wall-clock or monotonic time, or sleeping / firing on a timer.
    Clock,
    /// Drawing non-deterministic randomness (RNG, random tokens, UUIDs).
    Random,
    /// Crossing into native `Rust.` code, which is opaque to capability
    /// inference. Its presence is the signal that a program's true capability
    /// set cannot be inferred from Ipê alone.
    NativeFfi,
    /// Crossing into native `Rust.` code through an author-asserted signature
    /// (`Rust.Ffi.call`) rather than an inspected binding. Always accompanied
    /// by [`Self::NativeFfi`] (every asserted call is a native crossing); its
    /// own presence discloses that the foreign signature was vouched by the
    /// author, not derived from crate introspection.
    FfiRaw,
    /// Reaching for a trust-escape hatch: the program imports an `Ipe.<M>.Unsafe`
    /// submodule, whose members mint a security-tier value by assertion rather
    /// than by parse. Like [`Self::NativeFfi`], this is a provenance disclosure,
    /// not a resource axis an OS jail can isolate — its presence marks that the
    /// program contains a value the compiler could not prove safe.
    Unsafe,
}

impl Capability {
    /// Every capability, in declaration order. The vocabulary is closed; a new
    /// axis is added here and, by the exhaustive match in
    /// [`crate::StdlibKernel::capability`], classified for every kernel.
    pub const ALL: &'static [Self] = &[
        Self::Network,
        Self::Filesystem,
        Self::Database,
        Self::Env,
        Self::Subprocess,
        Self::Clock,
        Self::Random,
        Self::NativeFfi,
        Self::FfiRaw,
        Self::Unsafe,
    ];

    /// Whether this capability carries no OS-isolatable resource surface — the
    /// low-value axes `clock`/`random`/`unsafe`.
    ///
    /// `clock` and `random` are non-determinism, not exfiltration; `unsafe` is a
    /// provenance disclosure over Ipê-level escape hatches, not a native OS
    /// effect. None of the three is a jail confinement axis, so none can ever be
    /// a member of a confined set. This is the SSOT for that grouping: every site
    /// that distinguishes the low-value axes from the runtime-enforced ones
    /// (network/filesystem/database/env/subprocess/native-ffi) reads it here
    /// rather than re-listing the trio and risking drift.
    #[must_use]
    pub const fn is_low_value(self) -> bool {
        matches!(self, Self::Clock | Self::Random | Self::Unsafe)
    }

    /// The stable lowercase wire name, used in the `ipe capabilities` report and
    /// the generated manifest.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::Filesystem => "filesystem",
            Self::Database => "database",
            Self::Env => "env",
            Self::Subprocess => "subprocess",
            Self::Clock => "clock",
            Self::Random => "random",
            Self::NativeFfi => "native-ffi",
            Self::FfiRaw => "ffi-raw",
            Self::Unsafe => "unsafe",
        }
    }
}

/// Parse a capability from its wire name, the inverse of [`Capability::as_str`].
/// An unrecognised name is [`UnknownCapability`] rather than a silent drop — a
/// typo'd `[capabilities]` entry in a manifest must be a loud rejection, never a
/// capability the sandbox then fails to enforce.
impl std::str::FromStr for Capability {
    type Err = UnknownCapability;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        match name {
            "network" => Ok(Self::Network),
            "filesystem" => Ok(Self::Filesystem),
            "database" => Ok(Self::Database),
            "env" => Ok(Self::Env),
            "subprocess" => Ok(Self::Subprocess),
            "clock" => Ok(Self::Clock),
            "random" => Ok(Self::Random),
            "native-ffi" => Ok(Self::NativeFfi),
            "ffi-raw" => Ok(Self::FfiRaw),
            "unsafe" => Ok(Self::Unsafe),
            other => Err(UnknownCapability(other.to_owned())),
        }
    }
}

/// An unrecognised capability wire name, from [`Capability`]'s
/// [`FromStr`](std::str::FromStr). Carries the offending token so the caller can
/// name it in a diagnostic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownCapability(pub String);

impl std::fmt::Display for UnknownCapability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "unknown capability {:?} (expected one of: network, filesystem, \
             database, env, subprocess, clock, random, native-ffi, ffi-raw, unsafe)",
            self.0
        )
    }
}

impl std::error::Error for UnknownCapability {}

/// The trait bound a collection kernel imposes on its ELEMENT type — the
/// soundness axis for storing a value inside a `List`/`Dict`/`Set`.
///
/// A stored function value is carried on the `Clone` `Arc<dyn Fn>` carrier, which
/// is `Clone` but neither `PartialEq`/`PartialOrd` nor `Ord`/`Hash`. A kernel
/// whose emitted Rust operates on the element only by move/clone
/// ([`Self::CloneOk`]) is therefore sound over a function element; a kernel that
/// compares elements for equality ([`Self::RequiresPartialEq`]) or orders them
/// ([`Self::RequiresOrd`]) is NOT, and a function-embedding element must be
/// rejected at `ipe` time with the equality/ordering diagnostic rather than
/// emitting Rust that `cargo` rejects (`Arc<dyn Fn>: !PartialEq`).
///
/// This makes the element requirement an explicit registry fact rather than an
/// implicit property of the hand-written runtime signature
/// (make-invalid-states-unrepresentable). Every `List`/`Dict`/`Set` kernel
/// carries one, verified by a coherence test.
///
/// The three forbidding variants encode *why* a function element is unsound for
/// a given kernel, so the set is exhaustive rather than an implicit allowlist of
/// the kernels whose function-element frontier happens to be closed:
///
/// - [`Self::RequiresPartialEq`] / [`Self::RequiresOrd`]: the emitted Rust
///   compares or orders the element — no `Arc<dyn Fn>` representation exists.
/// - [`Self::MapperFrontierOpen`]: the kernel passes the element into a mapper
///   closure whose parameter carrier the lowerer has NOT aligned to the stored
///   `Arc<dyn Fn>`, so a function element would emit an `Arc`-vs-`Box` mismatch.
///   [`Self::CloneOk`] is reserved for the higher-order kernels whose frontier
///   IS closed plus the pure move/clone/structural kernels.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub enum ElementCapability {
    /// The element is only moved / cloned, OR it is passed to a mapper closure
    /// whose parameter carrier the lowerer aligns to the stored `Arc<dyn Fn>` —
    /// sound for an `Arc<dyn Fn>` (function) element either way. Covers the pure
    /// structural kernels and the higher-order kernels whose function-element
    /// frontier is closed.
    CloneOk,
    /// The element is compared for equality (`==`) — requires `PartialEq`, which
    /// a function carrier does not satisfy. A function-embedding element is
    /// rejected.
    RequiresPartialEq,
    /// The element is ordered (`<`/`sort`/keyed) — requires `PartialOrd`/`Ord`,
    /// which a function carrier does not satisfy. A function-embedding element is
    /// rejected.
    RequiresOrd,
    /// The element is passed into a mapper/comparator closure whose parameter
    /// carrier the lowerer does NOT re-type to the stored `Arc<dyn Fn>` — the
    /// higher-order frontier is open. A function carrier would emit an
    /// `Arc`-vs-`Box` mismatch (`E0308`) or a `Box<dyn Fn>: Clone` failure
    /// (`E0277`), so a function-embedding element is rejected fail-closed rather
    /// than mis-emitted. This is the SSOT for "this map/fold/filter kernel is not
    /// Arc-safe over a function element" — a kernel joins [`Self::CloneOk`] only
    /// once its frontier is actually closed in the lowerer.
    MapperFrontierOpen,
}

impl ElementCapability {
    /// Does this capability forbid a function-carrying element? `true` for the
    /// equality/ordering requirements and the open-frontier mapper family;
    /// `false` only for [`Self::CloneOk`].
    #[must_use]
    pub const fn forbids_function_element(self) -> bool {
        matches!(
            self,
            Self::RequiresPartialEq | Self::RequiresOrd | Self::MapperFrontierOpen
        )
    }
}

#[cfg(test)]
mod tests {
    use super::Capability;

    #[test]
    fn all_lists_every_variant_once() {
        // A guard against `ALL` drifting from the enum: each name is distinct,
        // and the count matches the declared axes.
        let names: Vec<&str> = Capability::ALL.iter().map(|c| c.as_str()).collect();
        assert_eq!(names.len(), 10);
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "ALL has a duplicate");
    }

    #[test]
    fn as_str_is_the_wire_vocabulary() {
        assert_eq!(Capability::Network.as_str(), "network");
        assert_eq!(Capability::Filesystem.as_str(), "filesystem");
        assert_eq!(Capability::Database.as_str(), "database");
        assert_eq!(Capability::Env.as_str(), "env");
        assert_eq!(Capability::Subprocess.as_str(), "subprocess");
        assert_eq!(Capability::Clock.as_str(), "clock");
        assert_eq!(Capability::Random.as_str(), "random");
        assert_eq!(Capability::NativeFfi.as_str(), "native-ffi");
        assert_eq!(Capability::FfiRaw.as_str(), "ffi-raw");
        assert_eq!(Capability::Unsafe.as_str(), "unsafe");
    }

    #[test]
    fn from_str_round_trips_every_variant() {
        // `from_str` is the exact inverse of `as_str` over the whole vocabulary.
        use std::str::FromStr as _;
        for &cap in Capability::ALL {
            assert_eq!(Capability::from_str(cap.as_str()), Ok(cap));
        }
    }

    #[test]
    fn from_str_rejects_an_unknown_name() {
        use std::str::FromStr as _;
        let err = Capability::from_str("filesytem").unwrap_err();
        assert_eq!(err, super::UnknownCapability("filesytem".to_owned()));
    }

    #[test]
    fn low_value_is_exactly_clock_random_unsafe() {
        // Pins the SSOT low-value grouping against drift: exactly the three axes
        // with no OS-isolatable surface. A new capability defaults to high-value
        // (runtime-enforced) unless it is deliberately added here.
        let low: Vec<&str> = Capability::ALL
            .iter()
            .filter(|c| c.is_low_value())
            .map(|c| c.as_str())
            .collect();
        assert_eq!(low, vec!["clock", "random", "unsafe"]);
        assert!(Capability::Clock.is_low_value());
        assert!(Capability::Random.is_low_value());
        assert!(Capability::Unsafe.is_low_value());
        assert!(!Capability::Network.is_low_value());
        assert!(!Capability::NativeFfi.is_low_value());
    }

    #[test]
    fn ordering_is_deterministic_for_a_btreeset() {
        // `program_capabilities` returns a `BTreeSet<Capability>`; the derived
        // `Ord` must give a stable, reproducible report order.
        use std::collections::BTreeSet;
        let set: BTreeSet<Capability> = [Capability::Random, Capability::Network, Capability::Env]
            .into_iter()
            .collect();
        let ordered: Vec<&str> = set.iter().map(|c| c.as_str()).collect();
        assert_eq!(ordered, vec!["network", "env", "random"]);
    }
}
