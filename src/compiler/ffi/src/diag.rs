//! Typed diagnostics for the FFI generator — the `IPE-F####` block.
//!
//! Every fallible public function in this crate returns
//! `Result<T, Diagnostic>`; there is no `Result<_, String>` on any public
//! surface. Each variant maps to exactly one taxonomy [`Code`] and carries a
//! closed defect enum, so a caller can match on the failure class without
//! string inspection.

use std::fmt;

use ipe_diagnostics::{
    Code, Diagnostic as SharedDiag, FfiError, IPE_F4400, IPE_F4401, IPE_F4402, IPE_F4411,
    IPE_F4412, IPE_F4414, IPE_F4415,
};

/// One FFI-generator diagnostic: the failure class plus enough context to
/// name the offending binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Diagnostic {
    /// `IPE-F4400` — a foreign-call AST failed one of the structural checks
    /// that make `render_call` total; it is refused before any emission.
    CallUnrenderable {
        /// The wrapper-reference name of the function whose call was refused.
        function: String,
        /// Which structural check failed.
        defect: CallDefect,
    },
    /// `IPE-F4401` — inspector wire data carried a value outside its closed
    /// legal set (unknown enum string, illegal identifier, malformed JSON).
    WireMalformed {
        /// Where in the wire document the defect was met (crate or function).
        context: String,
        /// Which wire rule was broken.
        defect: WireDefect,
    },
    /// `IPE-F4402` — a function's shape flags are contradictory (two of the
    /// mutually-exclusive accessor flags set at once). The one binding is
    /// dropped; the rest of the package survives.
    ShapeContradiction {
        /// The function whose flags contradict.
        function: String,
        /// The flag names that were simultaneously set.
        flags: Vec<&'static str>,
    },
    /// `IPE-F4400` — a reached generic FFI call site (or the generic binding
    /// itself) cannot be soundly bound: the instantiation falls outside the
    /// closed bindable set, a trait bound is unsatisfied or unmodellable, or
    /// a multi-call closure captures a non-Clone value. Raised at the call
    /// site by the instance gate — never a deferred cargo failure.
    GenericNotBindable {
        /// The qualified Ipê callee (`Rust.Box1.make`).
        callee: String,
        /// Which bindability rule was broken.
        defect: GenericBindDefect,
    },
    /// `IPE-F4411` — an untrusted crate source (git URL, pin, or crate name)
    /// was rejected at the driver gate, before reaching any command or the
    /// network.
    SourceRejected {
        /// The offending input, verbatim.
        source: String,
        /// Which gate rule was broken.
        defect: SourceDefect,
    },
    /// `IPE-F4412` — an FFI cache artifact could not be read, written, or
    /// removed.
    ArtifactIo {
        /// The artifact path.
        path: String,
        /// The rendered OS error.
        detail: String,
    },
    /// `IPE-F4414` — an author-asserted foreign call (`Rust.Ffi.call`) was
    /// refused at validation, before any shim was generated.
    AssertedRefused {
        /// The asserted Rust path, verbatim.
        path: String,
        /// Which rule refused it.
        defect: AssertedDefect,
    },
    /// `IPE-F4415` — the inspector's build failed because a required system
    /// library is not installed on the host. Parsed from the inspector's
    /// captured stderr at the `pkg-config`-not-found signature boundary, so the
    /// CLI can surface a targeted install hint instead of a raw cargo dump.
    SystemLibraryNotFound {
        /// The `pkg-config` library name that was not found (e.g. `wayland-client`).
        system_lib: String,
        /// The Rust crate whose `build.rs` required the library (e.g. `wayland-sys`).
        crate_name: String,
        /// A short OS-aware install hint, or a generic `-dev`/`.pc` fallback.
        install_hint: String,
    },
}

impl Diagnostic {
    /// The stable taxonomy code for this diagnostic.
    #[must_use]
    pub const fn code(&self) -> Code {
        match self {
            Self::CallUnrenderable { .. } | Self::GenericNotBindable { .. } => IPE_F4400,
            Self::WireMalformed { .. } => IPE_F4401,
            Self::ShapeContradiction { .. } => IPE_F4402,
            Self::SourceRejected { .. } => IPE_F4411,
            Self::ArtifactIo { .. } => IPE_F4412,
            Self::AssertedRefused { .. } => IPE_F4414,
            Self::SystemLibraryNotFound { .. } => IPE_F4415,
        }
    }
}

impl From<Diagnostic> for SharedDiag {
    fn from(d: Diagnostic) -> Self {
        Self::Ffi {
            msg: FfiError::from(d),
        }
    }
}

impl From<Diagnostic> for FfiError {
    fn from(d: Diagnostic) -> Self {
        match d {
            Diagnostic::CallUnrenderable { function, defect } => Self::CallUnrenderable {
                function,
                detail: defect.to_string(),
            },
            Diagnostic::GenericNotBindable { callee, defect } => Self::GenericNotBindable {
                callee,
                detail: defect.to_string(),
            },
            Diagnostic::WireMalformed { context, defect } => Self::WireMalformed {
                context,
                detail: defect.to_string(),
            },
            Diagnostic::ShapeContradiction { function, flags } => Self::ShapeContradiction {
                function,
                flags: flags.iter().map(ToString::to_string).collect(),
            },
            Diagnostic::SourceRejected { source, defect } => Self::SourceRejected {
                source,
                detail: defect.to_string(),
            },
            Diagnostic::ArtifactIo { path, detail } => Self::ArtifactIo { path, detail },
            Diagnostic::AssertedRefused { path, defect } => Self::AssertedRefused {
                path,
                detail: defect.to_string(),
            },
            Diagnostic::SystemLibraryNotFound {
                system_lib,
                crate_name,
                install_hint,
            } => Self::SystemLibraryNotFound {
                system_lib,
                crate_name,
                install_hint,
            },
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let shared: SharedDiag = self.clone().into();
        f.write_str(&ipe_diagnostics::render(&shared, "", ""))
    }
}

impl std::error::Error for Diagnostic {}

/// The closed set of asserted-call validation refusals (`IPE-F4414`). Every
/// rule runs at build preparation, before any shim or interface entry is
/// generated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssertedDefect {
    /// The path's crate segment names no installed FFI crate — the escape
    /// hatch can never bypass the `ipe rust add` admission pipeline.
    TargetCrateNotInstalled {
        /// The crate segment of the asserted path.
        crate_ident: String,
    },
    /// The asserted signature is not a function arrow.
    NotAFunction,
    /// A signature component falls outside the closed carrier set.
    CarrierOutsideClosedSet {
        /// The offending type, as written.
        ty: String,
    },
    /// An opaque nominal in the signature is not declared by the target
    /// crate's interface — an assertion cannot conjure a type mapping or
    /// forge another crate's handle.
    OpaqueNotDeclared {
        /// The nominal, as written.
        name: String,
    },
    /// The result is not exactly `Result Error <T>` — the one shape whose
    /// error channel the panic boundary folds into.
    ResultShape {
        /// The result type, as written.
        ty: String,
    },
    /// A `()` parameter beside other parameters (it is legal only as the sole
    /// parameter of a zero-argument target).
    UnitParamNotSole,
    /// The target is inspected and its signature cannot carry an asserted
    /// call (non-pure effect, no return value, or a receiver).
    InspectedShapeUnsupported {
        /// The unsupported aspect.
        reason: String,
    },
    /// The target is inspected and the assertion does not match it under the
    /// exact-carrier rule (identity, never a clamp or widening).
    InspectedMismatch {
        /// The exact Rust signature the inspection records.
        expected: String,
    },
    /// Two definitions assert different signatures for one path.
    ConflictingAssertions {
        /// The two rendered signatures.
        first: String,
        second: String,
    },
}

impl fmt::Display for AssertedDefect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TargetCrateNotInstalled { crate_ident } => write!(
                f,
                "crate `{crate_ident}` is not an installed FFI dependency — run \
                 `ipe rust add {crate_ident}` first"
            ),
            Self::NotAFunction => write!(
                f,
                "the asserted signature must be a function type ending in \
                 `Result Error <T>`"
            ),
            Self::CarrierOutsideClosedSet { ty } => write!(
                f,
                "`{ty}` is outside the asserted-call carrier set (Int, Float, Bool, \
                 Char, String, Bytes, or an opaque type the target crate declares)"
            ),
            Self::OpaqueNotDeclared { name } => write!(
                f,
                "`{name}` is not a type the target crate's interface declares — an \
                 assertion cannot introduce a new foreign type"
            ),
            Self::ResultShape { ty } => write!(
                f,
                "the result must be exactly `Result Error <T>` (got `{ty}`) — the \
                 error channel is where a foreign panic or failure lands"
            ),
            Self::UnitParamNotSole => write!(
                f,
                "`()` is legal only as the single parameter of a zero-argument target"
            ),
            Self::InspectedShapeUnsupported { reason } => write!(
                f,
                "the inspected target cannot carry an asserted call ({reason}) — use \
                 the inspected import (`import Rust.<Crate>`) instead"
            ),
            Self::InspectedMismatch { expected } => write!(
                f,
                "the assertion does not match the inspected signature `{expected}` \
                 under the exact-carrier rule (no clamp, no widening) — fix the \
                 assertion or use the inspected import"
            ),
            Self::ConflictingAssertions { first, second } => write!(
                f,
                "two definitions assert different signatures for this path \
                 (`{first}` vs `{second}`) — make them identical or share one \
                 asserted definition"
            ),
        }
    }
}

/// The closed set of crate-source gate rejections (`IPE-F4411`). Every rule
/// runs BEFORE the input can reach a command line or the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceDefect {
    /// A crate name outside `[A-Za-z0-9_-]+`.
    CrateNameIllegal,
    /// A git URL whose scheme is not `https://`.
    SchemeNotHttps,
    /// A git URL with no host component.
    HostMissing,
    /// A git host with characters outside `[A-Za-z0-9._-]`.
    HostCharsetIllegal {
        /// The offending host.
        host: String,
    },
    /// A git host not on the allowlist.
    HostNotAllowlisted {
        /// The offending host.
        host: String,
        /// The hosts that would be accepted.
        allowed: Vec<String>,
    },
    /// More than one of rev/branch/tag supplied — git honours only one.
    MultiplePins {
        /// The pin kinds that were simultaneously supplied.
        present: Vec<&'static str>,
    },
    /// A pin value that is empty, option-shaped (`-…`), or carries
    /// whitespace/control characters.
    PinIllegal {
        /// The offending pin value.
        got: String,
    },
    /// A crate version requirement outside the semver charset
    /// `[0-9A-Za-z.*=<>~^,+ -]` — the same TOML-value position the crate name
    /// occupies, so the same injection gate applies.
    VersionReqIllegal {
        /// The offending version requirement.
        got: String,
    },
    /// A `[rust.wrapper]` path that is absolute, empty, or escapes the package
    /// root (a `..` component or a leading `/`). Only a package-jailed relative
    /// path may name a local wrapper crate.
    WrapperPathEscapes {
        /// The offending path.
        got: String,
    },
    /// A `[rust.wrapper]` path carrying a character outside the safe set
    /// `[A-Za-z0-9._/-]` — every path segment must be a plain relative
    /// directory name so nothing breaks out of an argv or a TOML value.
    WrapperPathCharsetIllegal {
        /// The offending path.
        got: String,
    },
    /// A `[rust.wrapper]` with no `expose` list — a wrapper that binds nothing
    /// is a no-op declaration and almost certainly an authoring mistake.
    WrapperExposeEmpty,
    /// A `[rust.wrapper] capabilities` entry outside the closed capability
    /// vocabulary. A typo'd capability must be a loud rejection at decode, never
    /// a raw string the install-time reconcile silently fails to compare.
    WrapperCapabilityUnknown {
        /// The offending capability name.
        got: String,
    },
}

impl fmt::Display for SourceDefect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CrateNameIllegal => {
                f.write_str("crate name must be non-empty and match [A-Za-z0-9_-]+")
            }
            Self::SchemeNotHttps => f.write_str("git source must use the https:// scheme"),
            Self::HostMissing => f.write_str("git URL has no host component"),
            Self::HostCharsetIllegal { host } => {
                write!(f, "git host {host:?} has characters outside [A-Za-z0-9._-]")
            }
            Self::HostNotAllowlisted { host, allowed } => write!(
                f,
                "git host {host:?} is not on the allowlist ({}); set IPE_FFI_GIT_HOSTS to extend it",
                allowed.join(", ")
            ),
            Self::MultiplePins { present } => write!(
                f,
                "rev/branch/tag are mutually exclusive, but {} were supplied",
                present.join(" + ")
            ),
            Self::PinIllegal { got } => write!(
                f,
                "pin value {got:?} is empty, option-shaped, or carries whitespace"
            ),
            Self::VersionReqIllegal { got } => write!(
                f,
                "version requirement {got:?} must be non-empty semver text \
                 ([0-9A-Za-z.*=<>~^,+ -])"
            ),
            Self::WrapperPathEscapes { got } => write!(
                f,
                "wrapper crate path {got:?} must be a non-empty relative path inside \
                 the package (no leading `/`, no `..` component)"
            ),
            Self::WrapperPathCharsetIllegal { got } => write!(
                f,
                "wrapper crate path {got:?} has characters outside [A-Za-z0-9._/-]"
            ),
            Self::WrapperExposeEmpty => f.write_str(
                "[rust.wrapper] declares no `expose` symbols — a wrapper that binds \
                 nothing has no effect",
            ),
            Self::WrapperCapabilityUnknown { got } => write!(
                f,
                "[rust.wrapper] declares unknown capability {got:?} (expected one of: \
                 network, filesystem, database, env, subprocess, clock, random, native-ffi)"
            ),
        }
    }
}

/// Why a concrete instantiation type falls outside the closed bindable set.
///
/// A residual type variable means monomorphisation did not specialise the
/// call — the sound answer is this rejection, never a boxed/`any` fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClosedSetViolation {
    /// A bare type variable survived monomorphisation.
    UnresolvedTypeVariable(String),
    /// A named constructor outside the closed set (opaque foreign types are
    /// conservatively rejected pending derive-scan metadata).
    NonClosedConstructor(String),
    /// A record type.
    RecordType,
    /// A tuple type.
    TupleType,
    /// A function type.
    FunctionType,
    /// A type alias (unexpanded).
    TypeAlias(String),
}

impl fmt::Display for ClosedSetViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnresolvedTypeVariable(n) => write!(f, "unresolved type variable `{n}`"),
            Self::NonClosedConstructor(n) => write!(f, "non-closed type constructor `{n}`"),
            Self::RecordType => write!(f, "record type"),
            Self::TupleType => write!(f, "tuple type"),
            Self::FunctionType => write!(f, "function type"),
            Self::TypeAlias(n) => write!(f, "type alias `{n}`"),
        }
    }
}

/// The closed set of generic-FFI bindability defects (`IPE-F4400`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GenericBindDefect {
    /// The instantiation type is outside the closed Ipê↔Rust bindable set.
    OutsideClosedSet {
        /// The type-param name.
        param: String,
        /// The rendered instantiation type.
        ty: String,
        /// Why the type is outside the set.
        violation: ClosedSetViolation,
    },
    /// The bound is modellable but the concrete Rust type lacks the trait.
    BoundUnsatisfied {
        /// The type-param name.
        param: String,
        /// The rendered Ipê instantiation type.
        ty: String,
        /// The closed Rust type it maps to.
        rust_ty: String,
        /// The unsatisfied trait bound.
        bound: String,
    },
    /// The declared bound is outside the modellable `MODELLABLE_5` table —
    /// the backend cannot prove it holds for an arbitrary bindable type, so
    /// it refuses to emit an unsound wrapper (names the BOUND, not the type).
    UnmodellableBound {
        /// The type-param name.
        param: String,
        /// The unmodellable trait bound.
        bound: String,
    },
    /// An Ipê lambda captures a non-Clone value into a multi-call
    /// (`Fn`/`FnMut`) closure slot, whose owned-clone bridge re-clones every
    /// capture per call.
    CaptureNotClone {
        /// The captured variable name.
        capture: String,
        /// The rendered Ipê type of the capture.
        ty: String,
    },
}

impl fmt::Display for GenericBindDefect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutsideClosedSet {
                param,
                ty,
                violation,
            } => write!(
                f,
                "type parameter `{param}` is instantiated at `{ty}` ({violation}), outside the \
                 Ipê↔Rust bindable set; use a primitive (Int / Float / Bool / Char / String), a \
                 `List` or `Maybe` of one, or bind a non-generic FFI wrapper for `{ty}`"
            ),
            Self::BoundUnsatisfied {
                param,
                ty,
                rust_ty,
                bound,
            } => write!(
                f,
                "type parameter `{param}` is instantiated at `{ty}` (Rust `{rust_ty}`), but the \
                 binding requires the Rust trait bound `{bound}` and `{rust_ty}` does not \
                 implement `{bound}`"
            ),
            Self::UnmodellableBound { param, bound } => write!(
                f,
                "the binding declares the Rust trait bound `{bound}` on type parameter `{param}`, \
                 but the backend can only model the bounds {{Hash, Eq, Ord, Clone, Default}}; it \
                 will not emit an unsound generic wrapper — drop the `{bound}` bound or bind a \
                 non-generic wrapper at the concrete type(s) you need"
            ),
            Self::CaptureNotClone { capture, ty } => write!(
                f,
                "capture `{capture}` of type `{ty}` is passed into a multi-call closure that Rust \
                 requires to be `Fn + Clone`, but `{ty}` is not provably Clone; use a Clone-able \
                 captured value, capture nothing, or pass it to a single-call FnOnce slot"
            ),
        }
    }
}

/// The closed set of structural defects a foreign-call AST can carry.
///
/// A negative argument index has no variant: wire indices decode as `usize`,
/// so a negative value is rejected at the serde layer as [`WireDefect::Json`]
/// before a call AST exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallDefect {
    /// A `{param: i}` type reference is outside the declared param count.
    ParamRefOutOfRange {
        /// The out-of-range index.
        index: usize,
        /// How many type params the enclosing generic block declares.
        n_params: usize,
    },
    /// Call kind `method` but no `receiver` present.
    ReceiverMissingForMethod,
    /// Call kind `function` but a `receiver` is present.
    ReceiverForbiddenForFunction,
    /// A value-arg index feeds two slots (a use-after-move in rendered Rust).
    ArgIndexDuplicated {
        /// The index referenced more than once.
        index: usize,
    },
    /// Value-arg indices are not contiguous from 0.
    ArgIndexGap {
        /// The smallest never-referenced index below the arity.
        missing: usize,
    },
    /// `argTypes` length disagrees with the call's value-arg count.
    ArgTypeArityMismatch {
        /// The number of `argTypes` entries present.
        arg_types_len: usize,
        /// The value-arg count the call references.
        arity: usize,
    },
    /// A closure type appears somewhere other than a direct argument slot
    /// (nested in a container, the return, a type-argument, or a method
    /// turbofish) — unrenderable as valid Rust.
    ClosureNestedOrNonDirect,
    /// An `iterAdapters` index does not reference a real value-arg slot.
    IterAdapterOutOfRange {
        /// The out-of-range adapter index.
        index: usize,
        /// The call's value-arg count.
        arity: usize,
    },
    /// An `iterAdapters` index targets a non-`Vec` argument type
    /// (`.into_iter()` is sound only on a `Vec` arg).
    IterAdapterTargetNotVec {
        /// The adapter index whose slot is not a `Vec`.
        index: usize,
    },
    /// A type reference carried a string outside the closed renderable type
    /// grammar — it would render verbatim as unsound (or injection-bearing)
    /// Rust, so the whole call is refused at decode.
    TypeUnrenderable {
        /// The offending type string.
        got: String,
    },
}

impl fmt::Display for CallDefect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ParamRefOutOfRange { index, n_params } => write!(
                f,
                "type-param ref {{param:{index}}} is out of range (declared {n_params} param(s))"
            ),
            Self::ReceiverMissingForMethod => {
                write!(
                    f,
                    "call kind \"method\" requires a `receiver`, but none is present"
                )
            }
            Self::ReceiverForbiddenForFunction => {
                write!(f, "call kind \"function\" must not carry a `receiver`")
            }
            Self::ArgIndexDuplicated { index } => {
                write!(f, "value-arg {{arg:{index}}} is referenced more than once")
            }
            Self::ArgIndexGap { missing } => write!(
                f,
                "value-arg index {missing} is never referenced (arg indices must be contiguous from 0)"
            ),
            Self::ArgTypeArityMismatch {
                arg_types_len,
                arity,
            } => write!(
                f,
                "argTypes has {arg_types_len} entry(ies) but the call references {arity} value-arg(s)"
            ),
            Self::ClosureNestedOrNonDirect => write!(
                f,
                "a closure type may only appear as a direct wrapper argument, not nested inside a container, return, type-argument, or method turbofish"
            ),
            Self::IterAdapterOutOfRange { index, arity } => write!(
                f,
                "iterAdapters index {index} is out of range (the call references {arity} value-arg(s))"
            ),
            Self::IterAdapterTargetNotVec { index } => write!(
                f,
                "iterAdapters index {index} targets a non-Vec argType (`.into_iter()` is sound only on a Vec arg)"
            ),
            Self::TypeUnrenderable { got } => write!(
                f,
                "type reference {got:?} is outside the closed renderable type grammar (paths, generics, borrows, tuples, arrays only)"
            ),
        }
    }
}

/// The closed set of wire-level defects the validating decoders reject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireDefect {
    /// A call `kind` string outside `method` / `function`.
    UnknownCallKind {
        /// The value met on the wire.
        got: String,
    },
    /// A receiver `by` string outside `ref` / `refmut` / `value`.
    UnknownByKind {
        /// The value met on the wire.
        got: String,
    },
    /// A closure `kind` string outside `Fn` / `FnMut` / `FnOnce`.
    UnknownClosureKind {
        /// The value met on the wire.
        got: String,
    },
    /// An `effect` string outside `pure` / `fallible` / `effectful`.
    UnknownEffect {
        /// The value met on the wire.
        got: String,
    },
    /// A `TypeRef` object with zero or more than one discriminator key.
    TypeRefDiscriminator {
        /// The discriminator keys that were present.
        present: Vec<&'static str>,
    },
    /// A name that must be a legal Rust identifier is not.
    InvalidIdent {
        /// The offending name.
        got: String,
    },
    /// A module path segment that is not a legal Rust identifier path.
    InvalidModulePath {
        /// The offending path.
        got: String,
    },
    /// A type expression that is outside the closed FFI-emitter grammar (a
    /// byte, unbalanced bracket, or `;` that could open a statement in the
    /// rendered wrapper).
    InvalidType {
        /// The offending type string.
        got: String,
    },
    /// An enum-arm pattern that is not a `RustIdent` head with an optional
    /// `(..)` / `{..}` suffix.
    InvalidPattern {
        /// The offending pattern string.
        got: String,
    },
    /// A field selector that is neither a `RustIdent` nor a decimal index.
    InvalidSelector {
        /// The offending selector string.
        got: String,
    },
    /// The `pkg` path carries a control character (a bare newline could
    /// otherwise close the `//` comment it is emitted into and splice
    /// compilable Rust source into the generated bindings file).
    InvalidPkgPath {
        /// The offending path.
        got: String,
    },
    /// A resolved crate version carries a character outside the semver charset
    /// `[0-9A-Za-z.*=<>~^,+ -]`. The version is spliced into a TOML value
    /// position of the emitted `Cargo.toml` (`<name> = "=<version>"`); a value
    /// carrying a quote/brace/bracket/newline could break out of the string
    /// and inject arbitrary manifest content.
    InvalidVersion {
        /// The offending version string.
        got: String,
    },
    /// A Cargo feature name carries a character outside the feature charset
    /// `[A-Za-z0-9_+./?:-]`. Each feature is spliced into a `features = [ … ]`
    /// TOML array position of the emitted `Cargo.toml`; a value carrying a
    /// quote/brace/bracket/newline could break out of the array and inject
    /// arbitrary manifest content.
    InvalidFeature {
        /// The offending feature string.
        got: String,
    },
    /// A `[rust.define.closure]` signature does not parse into the closed
    /// [`crate::carrier::ClosureSig`] shape: a parameter or return component
    /// outside the carrier set, a bound outside `{Send, Sync, 'static}`, a
    /// return that is neither a total scalar carrier nor `Result`/`Option`, or
    /// unconsumed trailing text. Refused at decode — no fragment reaches the
    /// emitted adapter as a raw string.
    InvalidClosureSig {
        /// The offending signature string.
        got: String,
        /// Which structural rule was broken.
        reason: String,
    },
    /// A `[rust.define.struct]`/`[rust.define.enum]` whose field/payload
    /// reference graph forms a cycle back to itself (directly, or mutually
    /// through other define types). A recursive nominal type has no `Box` in
    /// the closed carrier set to break it, so emitting it would be an
    /// infinitely-sized Rust type (`error[E0072]`). Refused at decode — the
    /// package author must break the cycle (e.g. indirect through a handle the
    /// FFI can name), never emit-and-cargo-fail.
    RecursiveDefineType {
        /// The define type refused (a member of the cycle).
        name: String,
        /// The cycle, as the chain of define-type names it closes over.
        cycle: Vec<String>,
    },
    /// The document is not the JSON shape the wire contract declares
    /// (carries the rendered serde error as detail).
    Json {
        /// The serde decode error, rendered.
        detail: String,
    },
}

impl fmt::Display for WireDefect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownCallKind { got } => write!(
                f,
                "unknown call kind {got:?} (expected \"method\" or \"function\")"
            ),
            Self::UnknownByKind { got } => write!(
                f,
                "unknown receiver `by` kind {got:?} (expected \"ref\", \"refmut\", or \"value\")"
            ),
            Self::UnknownClosureKind { got } => write!(
                f,
                "unknown closure kind {got:?} (expected \"Fn\", \"FnMut\", or \"FnOnce\")"
            ),
            Self::UnknownEffect { got } => write!(
                f,
                "unknown effect {got:?} (expected \"pure\", \"fallible\", or \"effectful\")"
            ),
            Self::TypeRefDiscriminator { present } => {
                if present.is_empty() {
                    write!(
                        f,
                        "TypeRef must have exactly one of `param`, `prim`, `ctor`, `closure`, `serdeValue`, or `serdeValueRef`"
                    )
                } else {
                    write!(
                        f,
                        "TypeRef carries more than one discriminator: {}",
                        present.join(", ")
                    )
                }
            }
            Self::InvalidIdent { got } => {
                write!(f, "{got:?} is not a legal Rust identifier")
            }
            Self::InvalidModulePath { got } => {
                write!(f, "{got:?} is not a legal Rust identifier path")
            }
            Self::InvalidType { got } => {
                write!(
                    f,
                    "{got:?} is outside the closed FFI type grammar (paths, generics, \
                     borrows, tuples, arrays only — no statement tokens)"
                )
            }
            Self::InvalidPattern { got } => {
                write!(
                    f,
                    "{got:?} is not a legal enum-arm pattern (a variant identifier with an \
                     optional (..) or {{..}} suffix)"
                )
            }
            Self::InvalidSelector { got } => {
                write!(
                    f,
                    "{got:?} is not a legal field selector (a field identifier or a decimal \
                     tuple index)"
                )
            }
            Self::InvalidPkgPath { got } => {
                write!(
                    f,
                    "{got:?} is not a legal package path (it carries a control character)"
                )
            }
            Self::InvalidVersion { got } => {
                write!(
                    f,
                    "{got:?} is not a legal crate version (it must match the semver charset \
                     [0-9A-Za-z.*=<>~^,+ -])"
                )
            }
            Self::InvalidFeature { got } => {
                write!(
                    f,
                    "{got:?} is not a legal cargo feature name (it must match the charset \
                     [A-Za-z0-9_+./?:-])"
                )
            }
            Self::InvalidClosureSig { got, reason } => {
                write!(
                    f,
                    "{got:?} is not a legal define.closure signature: {reason}"
                )
            }
            Self::RecursiveDefineType { name, cycle } => {
                write!(
                    f,
                    "define type {name:?} is recursive ({}) — a nominal FFI type cannot \
                     reference itself (no boxed indirection is available in the closed carrier \
                     set); break the cycle by indirecting through a crate handle the FFI can name",
                    cycle.join(" -> ")
                )
            }
            Self::Json { detail } => write!(f, "{detail}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_variant_maps_to_its_taxonomy_code() {
        let call = Diagnostic::CallUnrenderable {
            function: "parse".into(),
            defect: CallDefect::ReceiverMissingForMethod,
        };
        assert_eq!(call.code().as_str(), "IPE-F4400");

        let wire = Diagnostic::WireMalformed {
            context: "crate `semver`".into(),
            defect: WireDefect::UnknownEffect {
                got: "spooky".into(),
            },
        };
        assert_eq!(wire.code().as_str(), "IPE-F4401");

        let shape = Diagnostic::ShapeContradiction {
            function: "major_from_version".into(),
            flags: vec!["isField", "isEnumCtor"],
        };
        assert_eq!(shape.code().as_str(), "IPE-F4402");
    }

    #[test]
    fn display_carries_code_context_and_defect() {
        let d = Diagnostic::CallUnrenderable {
            function: "left".into(),
            defect: CallDefect::ParamRefOutOfRange {
                index: 3,
                n_params: 2,
            },
        };
        let s = d.to_string();
        assert!(s.contains("IPE-F4400"), "{s}");
        assert!(s.contains("`left`"), "{s}");
        assert!(s.contains("{param:3}"), "{s}");
        assert!(s.contains("2 param(s)"), "{s}");
    }
}
