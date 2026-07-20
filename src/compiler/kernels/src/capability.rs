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
/// The seven axes a sandbox can isolate independently. A kernel maps to at most
/// one; a program's set is the union over its reachable kernels plus
/// [`Capability::NativeFfi`] when it crosses into `Rust.` code.
#[derive(
    Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum Capability {
    /// Outbound or inbound network access (HTTP client/server, WebSocket,
    /// email send).
    Network,
    /// Reading or writing the filesystem (files, directories, a local
    /// database, an `.env` or config file).
    Filesystem,
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
}

impl Capability {
    /// Every capability, in declaration order. The vocabulary is closed; a new
    /// axis is added here and, by the exhaustive match in
    /// [`crate::StdlibKernel::capability`], classified for every kernel.
    pub const ALL: &'static [Self] = &[
        Self::Network,
        Self::Filesystem,
        Self::Env,
        Self::Subprocess,
        Self::Clock,
        Self::Random,
        Self::NativeFfi,
    ];

    /// The stable lowercase wire name, used in the `ipe capabilities` report and
    /// the generated manifest.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::Filesystem => "filesystem",
            Self::Env => "env",
            Self::Subprocess => "subprocess",
            Self::Clock => "clock",
            Self::Random => "random",
            Self::NativeFfi => "native-ffi",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Capability;

    #[test]
    fn all_lists_every_variant_once() {
        // A guard against `ALL` drifting from the enum: each name is distinct,
        // and the count matches the seven declared axes.
        let names: Vec<&str> = Capability::ALL.iter().map(|c| c.as_str()).collect();
        assert_eq!(names.len(), 7);
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "ALL has a duplicate");
    }

    #[test]
    fn as_str_is_the_wire_vocabulary() {
        assert_eq!(Capability::Network.as_str(), "network");
        assert_eq!(Capability::Filesystem.as_str(), "filesystem");
        assert_eq!(Capability::Env.as_str(), "env");
        assert_eq!(Capability::Subprocess.as_str(), "subprocess");
        assert_eq!(Capability::Clock.as_str(), "clock");
        assert_eq!(Capability::Random.as_str(), "random");
        assert_eq!(Capability::NativeFfi.as_str(), "native-ffi");
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
