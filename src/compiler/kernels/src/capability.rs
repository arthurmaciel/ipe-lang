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
    /// Resolved by SP4 sandbox to filesystem or network per the `package.ipe` driver.
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
    /// Shipping a browser custom-element widget: the program's reachable code
    /// binds `CustomElement.node` over a `CustomElement.fromFile "<path>"` handle, so it serves
    /// author-written JavaScript that runs in the page with full DOM authority.
    /// Like [`Self::NativeFfi`], this is a disclosure of a declared-trust surface
    /// the server-side sandbox cannot see through — the browser JS is not an OS
    /// resource axis an OS jail confines. Its presence discloses that the package
    /// ships client JS whose behaviour is the package author's declared trust; the
    /// served bytes are SRI-pinned and CSP-constrained, but never sandboxed. It is
    /// deliberately NOT low-value: a shipped-JS surface must always surface to the
    /// consumer, exactly as [`Self::NativeFfi`] does.
    CustomElement,
    /// Using a JS port: the program's reachable code binds `Js.send` (outbound
    /// Ipê→JS) or `Js.subscribe` (inbound JS→Ipê), so it exchanges typed values
    /// with page JavaScript over the raw transport. Like [`Self::CustomElement`],
    /// this is a disclosure of a declared-trust surface the server-side sandbox
    /// cannot see through: the far side is attacker-controlled browser JS, gated
    /// only by the fail-closed seal decoder (inbound) and the seal type (outbound).
    /// Its presence discloses that the package exchanges data with page JS whose
    /// behaviour is the package author's declared trust. Deliberately NOT
    /// low-value: a JS-exchange surface must always surface to the consumer.
    ///
    /// The port carries a [`WebCapability`] sub-axis naming the specific Web API
    /// the far side reaches ([`WebCapability::Clipboard`], …), or
    /// [`WebCapability::Raw`] for an uncharacterised hand-rolled port. A bare
    /// `js-port` with no web axis is unrepresentable — every port discloses a
    /// concrete web capability, so one coarse grant can never re-authorise the
    /// whole browser surface.
    JsPort(WebCapability),
}

/// The closed per-Web-API axis a [`Capability::JsPort`] discloses.
///
/// A browser port reaches exactly one Web API; this names which. It is a
/// separate axis from the OS-jail capabilities because a web capability runs in
/// the client page, never in the server process — the whole `JsPort(_)` family
/// is uniformly the "no server-jail surface" partition. The vocabulary is
/// closed and compiler-owned: a new browser capability is one variant here plus
/// its [`Self::as_str`] arm and its `Ipe.Browser.<Api>` module→axis table row.
///
/// [`Self::Raw`] is the explicit, grantable axis for a port whose reached Web API
/// the compiler cannot characterise (a hand-rolled `Js.send`/`Js.subscribe` with
/// author-written JS). Granting `js-port:raw` admits ONLY uncharacterised ports;
/// it does not grant any characterised axis. This is the coarse-axis-as-bypass
/// designed out: the coarsest grantable web axis reaches nothing specific.
#[derive(
    Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum WebCapability {
    /// `navigator.geolocation` — the device's location.
    Geolocation,
    /// `navigator.clipboard` — reading or writing the system clipboard.
    Clipboard,
    /// The Notification API — showing a system notification.
    Notification,
    /// `localStorage` / `sessionStorage` / `IndexedDB` — client-side persistence.
    Storage,
    /// `navigator.vibrate` — the vibration actuator.
    Vibration,
    /// `navigator.share` — the platform share sheet.
    Share,
    /// `navigator.getBattery` — battery status.
    Battery,
    /// `navigator.connection` — network-information hints.
    NetworkInfo,
    /// `<input type="file">` via the File API — opening a native file picker and
    /// reading the chosen file as a `data:` URL.
    File,
    /// `<input type="file" capture="environment" accept="image/*">` — directing
    /// the OS to open the device camera (mobile) or fall back to an image file
    /// picker (desktop). The result is a single captured image as a `data:` URL.
    Camera,
    /// A port with no characterised Web API: a hand-rolled `Js.send`/`Js.subscribe`
    /// reaching author-written JS. The reachability floor no port can slip below —
    /// an uncharacterised port discloses `js-port:raw`, never nothing.
    Raw,
}

impl WebCapability {
    /// Every web axis, in declaration order — the closed vocabulary. Feeds the
    /// flattened [`Capability::ALL`] and the round-trip drift guards.
    pub const ALL: &'static [Self] = &[
        Self::Geolocation,
        Self::Clipboard,
        Self::Notification,
        Self::Storage,
        Self::Vibration,
        Self::Share,
        Self::Battery,
        Self::NetworkInfo,
        Self::File,
        Self::Camera,
        Self::Raw,
    ];

    /// The stable lowercase wire suffix, the half after `js-port:` in the wire
    /// name (`js-port:clipboard` → `"clipboard"`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Geolocation => "geolocation",
            Self::Clipboard => "clipboard",
            Self::Notification => "notification",
            Self::Storage => "storage",
            Self::Vibration => "vibration",
            Self::Share => "share",
            Self::Battery => "battery",
            Self::NetworkInfo => "network-info",
            Self::File => "file",
            Self::Camera => "camera",
            Self::Raw => "raw",
        }
    }

    /// The web axis disclosed by importing a reserved `Ipe.Browser.<Api>` stdlib
    /// module, matched on the module's dotted path segments — the compiler-owned,
    /// closed SSOT for "which browser module discloses which Web API." A new
    /// browser module is one row here plus its `WebCapability` variant.
    ///
    /// Keyed on the `["Ipe", "Browser", <Api>, ..]` PREFIX, alias-immune exactly
    /// like the `Ipe.<M>.Unsafe` import rule: the canonical import path is what
    /// matches, never a local alias, and the reserved namespace cannot be forged by
    /// a user file. Matching the prefix (not the exact 3-segment path) closes the
    /// low-level submodule hole: importing `Ipe.Browser.Geolocation.Internals`
    /// discloses the same `js-port:geolocation` axis as the top-level module, so the
    /// full option surface cannot be reached undisclosed.
    #[must_use]
    pub fn for_browser_module(segments: &[&str]) -> Option<Self> {
        match segments {
            ["Ipe", "Browser", "Geolocation", ..] => Some(Self::Geolocation),
            ["Ipe", "Browser", "Clipboard", ..] => Some(Self::Clipboard),
            ["Ipe", "Browser", "Notification", ..] => Some(Self::Notification),
            ["Ipe", "Browser", "Storage", ..] => Some(Self::Storage),
            ["Ipe", "Browser", "Vibration", ..] => Some(Self::Vibration),
            ["Ipe", "Browser", "Share", ..] => Some(Self::Share),
            ["Ipe", "Browser", "Battery", ..] => Some(Self::Battery),
            ["Ipe", "Browser", "NetworkInfo", ..] => Some(Self::NetworkInfo),
            ["Ipe", "Browser", "FilePicker", ..] => Some(Self::File),
            ["Ipe", "Browser", "Camera", ..] => Some(Self::Camera),
            _ => None,
        }
    }

    /// One-line description of what granting this web capability permits.
    ///
    /// Used by [`Capability::grants`] for the `JsPort(_)` case.
    #[must_use]
    pub const fn grants(self) -> &'static str {
        match self {
            Self::Geolocation => "Accessing navigator.geolocation — the device's location.",
            Self::Clipboard => "Reading or writing the system clipboard via navigator.clipboard.",
            Self::Notification => "Showing a system notification via the Notification API.",
            Self::Storage => {
                "Client-side persistence via localStorage / sessionStorage / IndexedDB."
            }
            Self::Vibration => "Activating the vibration actuator via navigator.vibrate.",
            Self::Share => "Invoking the platform share sheet via navigator.share.",
            Self::Battery => "Reading battery status via navigator.getBattery.",
            Self::NetworkInfo => "Reading network-information hints via navigator.connection.",
            Self::File => {
                "Opening a native file picker and reading the chosen file via the File API \
                 (FileReader.readAsDataURL)."
            }
            Self::Camera => {
                "Capturing a photo via the device camera (mobile) or an image file picker \
                 (desktop) using a <input capture> element and the File API."
            }
            Self::Raw => {
                "Exchanging data with hand-rolled page JS over an uncharacterised port \
                 (Js.send / Js.subscribe with author-written JS)."
            }
        }
    }

    /// Parse a web-axis wire suffix, the inverse of [`Self::as_str`]. An
    /// unrecognised suffix is [`UnknownCapability`] carrying the full offending
    /// `js-port:<suffix>` token — fail-closed, never a silent drop.
    fn from_suffix(full: &str, suffix: &str) -> Result<Self, UnknownCapability> {
        match suffix {
            "geolocation" => Ok(Self::Geolocation),
            "clipboard" => Ok(Self::Clipboard),
            "notification" => Ok(Self::Notification),
            "storage" => Ok(Self::Storage),
            "vibration" => Ok(Self::Vibration),
            "share" => Ok(Self::Share),
            "battery" => Ok(Self::Battery),
            "network-info" => Ok(Self::NetworkInfo),
            "file" => Ok(Self::File),
            "camera" => Ok(Self::Camera),
            "raw" => Ok(Self::Raw),
            _ => Err(UnknownCapability(full.to_owned())),
        }
    }
}

impl Capability {
    /// The trust-boundary partition this capability belongs to.
    ///
    /// Three partitions cover the full vocabulary:
    ///
    /// - `"OS resource"` — axes an OS-level sandbox (seccomp, pledge, etc.)
    ///   can confine independently: network, filesystem, database, env,
    ///   subprocess. Each maps to a concrete system-call family the jail
    ///   controls.
    /// - `"Non-determinism"` — axes that are real OS effects but carry no
    ///   exfiltration risk on their own: clock, random. Sandboxes may or may
    ///   not confine these; they are never the primary isolation lever.
    /// - `"Native crossing"` — axes that disclose an opaque trust boundary
    ///   the compiler cannot see through: native FFI, raw FFI assertion,
    ///   Ipê-level unsafe escape, shipped browser JS, JS port data exchange.
    ///   No OS jail confines them; their disclosure is the enforcement
    ///   mechanism — the consumer decides whether to grant.
    #[must_use]
    pub const fn boundary_class(self) -> &'static str {
        match self {
            Self::Network | Self::Filesystem | Self::Database | Self::Env | Self::Subprocess => {
                "OS resource"
            }
            Self::Clock | Self::Random => "Non-determinism",
            Self::NativeFfi
            | Self::FfiRaw
            | Self::Unsafe
            | Self::CustomElement
            | Self::JsPort(_) => "Native crossing",
        }
    }

    /// One-line description of what granting this capability permits.
    ///
    /// Sourced from the variant's doc-comment; condensed for table display.
    #[must_use]
    pub const fn grants(self) -> &'static str {
        match self {
            Self::Network => {
                "Outbound or inbound network access (HTTP client/server, WebSocket, email send)."
            }
            Self::Filesystem => {
                "Reading or writing the filesystem (files, directories, config files)."
            }
            Self::Database => "Structured database access (SQL queries, migrations, row decoders).",
            Self::Env => "Reading or writing process environment variables and argv.",
            Self::Subprocess => "Spawning or controlling a child process.",
            Self::Clock => "Reading wall-clock or monotonic time, or sleeping / firing on a timer.",
            Self::Random => "Drawing non-deterministic randomness (RNG, random tokens, UUIDs).",
            Self::NativeFfi => {
                "Crossing into native Rust. code — the program's true capability set cannot be \
                 inferred from Ipê alone."
            }
            Self::FfiRaw => {
                "Crossing into native Rust. code through an author-asserted signature rather than \
                 an inspected binding (always accompanies NativeFfi)."
            }
            Self::Unsafe => {
                "Importing an Ipe.<M>.Unsafe submodule — the program contains a value the \
                 compiler could not prove safe."
            }
            Self::CustomElement => {
                "Shipping browser custom-element JS that runs in the page with full DOM \
                 authority; the served bytes are SRI-pinned and CSP-constrained."
            }
            Self::JsPort(w) => w.grants(),
        }
    }

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
        Self::CustomElement,
        // The `JsPort(_)` sub-axis flattens to one entry per `WebCapability`, so
        // `ALL` stays the closed, enumerable list every drift guard iterates.
        Self::JsPort(WebCapability::Geolocation),
        Self::JsPort(WebCapability::Clipboard),
        Self::JsPort(WebCapability::Notification),
        Self::JsPort(WebCapability::Storage),
        Self::JsPort(WebCapability::Vibration),
        Self::JsPort(WebCapability::Share),
        Self::JsPort(WebCapability::Battery),
        Self::JsPort(WebCapability::NetworkInfo),
        Self::JsPort(WebCapability::File),
        Self::JsPort(WebCapability::Camera),
        Self::JsPort(WebCapability::Raw),
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
            Self::CustomElement => "custom-element",
            // The dotted `js-port:<axis>` wire name, one static literal per web
            // axis — bare `js-port` is not among them (it is unrepresentable).
            Self::JsPort(w) => match w {
                WebCapability::Geolocation => "js-port:geolocation",
                WebCapability::Clipboard => "js-port:clipboard",
                WebCapability::Notification => "js-port:notification",
                WebCapability::Storage => "js-port:storage",
                WebCapability::Vibration => "js-port:vibration",
                WebCapability::Share => "js-port:share",
                WebCapability::Battery => "js-port:battery",
                WebCapability::NetworkInfo => "js-port:network-info",
                WebCapability::File => "js-port:file",
                WebCapability::Camera => "js-port:camera",
                WebCapability::Raw => "js-port:raw",
            },
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
            "custom-element" => Ok(Self::CustomElement),
            // A web port is spelled `js-port:<axis>`; the suffix parses against the
            // closed `WebCapability` vocabulary. A bare `js-port` (no suffix) has
            // no arm here, so it falls through to `UnknownCapability` — the coarse
            // grant-everything token a manifest cannot spell.
            other => other.strip_prefix("js-port:").map_or_else(
                || Err(UnknownCapability(other.to_owned())),
                |suffix| WebCapability::from_suffix(other, suffix).map(Self::JsPort),
            ),
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
             database, env, subprocess, clock, random, native-ffi, ffi-raw, unsafe, \
             custom-element, js-port:<axis> where <axis> is one of geolocation, \
             clipboard, notification, storage, vibration, share, battery, \
             network-info, file, camera, raw)",
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
    use super::{Capability, WebCapability};

    #[test]
    fn all_lists_every_variant_once() {
        // A guard against `ALL` drifting from the enum: each name is distinct,
        // and the count matches the declared axes (11 flat + one `JsPort` per
        // `WebCapability`, so the sub-axis is enumerated, not wildcarded away).
        let names: Vec<&str> = Capability::ALL.iter().map(|c| c.as_str()).collect();
        assert_eq!(names.len(), 11 + WebCapability::ALL.len());
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "ALL has a duplicate");
    }

    #[test]
    fn all_enumerates_every_web_axis_under_js_port() {
        // MUST-FIX #3 drift guard: every `WebCapability` appears as a `JsPort(w)`
        // member of `ALL` — no web axis can be silently dropped from the closed set.
        for &w in WebCapability::ALL {
            assert!(
                Capability::ALL.contains(&Capability::JsPort(w)),
                "ALL is missing js-port sub-axis {w:?}"
            );
        }
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
        assert_eq!(Capability::CustomElement.as_str(), "custom-element");
        assert_eq!(
            Capability::JsPort(WebCapability::Clipboard).as_str(),
            "js-port:clipboard"
        );
        assert_eq!(
            Capability::JsPort(WebCapability::Raw).as_str(),
            "js-port:raw"
        );
    }

    #[test]
    fn from_str_round_trips_every_variant() {
        // `from_str` is the exact inverse of `as_str` over the whole vocabulary,
        // including every `js-port:<axis>` sub-axis (MUST-FIX #2 round-trip).
        use std::str::FromStr as _;
        for &cap in Capability::ALL {
            assert_eq!(Capability::from_str(cap.as_str()), Ok(cap));
        }
        for &w in WebCapability::ALL {
            let cap = Capability::JsPort(w);
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
    fn from_str_hard_rejects_bare_js_port() {
        // MUST-FIX #2: a bare `js-port` (no `:<axis>` suffix) is NOT a member — a
        // manifest cannot spell the coarse grant-everything token.
        use std::str::FromStr as _;
        let err = Capability::from_str("js-port").unwrap_err();
        assert_eq!(err, super::UnknownCapability("js-port".to_owned()));
    }

    #[test]
    fn from_str_rejects_an_unknown_web_axis_suffix() {
        // An unrecognised `js-port:<axis>` suffix fails closed, carrying the full
        // offending token, exactly as any other typo does.
        use std::str::FromStr as _;
        let err = Capability::from_str("js-port:unknown-axis").unwrap_err();
        assert_eq!(
            err,
            super::UnknownCapability("js-port:unknown-axis".to_owned())
        );
    }

    #[test]
    fn from_str_accepts_file_and_camera_web_axes() {
        use std::str::FromStr as _;
        assert_eq!(
            Capability::from_str("js-port:file"),
            Ok(Capability::JsPort(WebCapability::File))
        );
        assert_eq!(
            Capability::from_str("js-port:camera"),
            Ok(Capability::JsPort(WebCapability::Camera))
        );
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
        // `custom-element` is a disclosure of shipped browser JS, a declared-trust
        // surface analogous to `native-ffi`: it must always surface, never be
        // grouped with the clock/random/unsafe noise the low-value flag marks.
        assert!(!Capability::CustomElement.is_low_value());
        // `js-port` is the same class of declared-trust JS-exchange disclosure: it
        // must always surface to the consumer, never be grouped as low-value. Every
        // web sub-axis (including `:raw`) shares that posture.
        for &w in WebCapability::ALL {
            assert!(!Capability::JsPort(w).is_low_value());
        }
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
