//! Ipe.Error: the rich, typed `Error` ADT.
//!
//! `Error = Error ErrorKind ErrorInfo`, a 12-variant `ErrorKind`
//! classification, message-carrying `ErrorInfo`, and the 5-variant
//! `ErrorDetails` union (`FfiPanic`/`TypeMismatch`/`HttpStatus`/`JsonDecode`/
//! `Custom`) carried optionally on `ErrorInfo.details : Maybe ErrorDetails`.
//!
//! Kind-based classification (`isRetryable`, pattern matching, `toString`)
//! and the `details` enrichment are both fully real and load-bearing today.
//! `Error.withDetails` is the sanctioned way to attach `ErrorDetails` to a
//! live `Error` value — raw Ipê-source construction of `ErrorInfo`/
//! `PanicInfo`/`TypeInfo` record literals is NOT supported (those are
//! anonymous structural records at the type level, so a literal lowers to a
//! project-local synthesized struct, not this module's concrete
//! `IpeErrorInfo`/`IpePanicInfo`/`IpeTypeInfo` — the same limitation
//! `ErrorInfo` itself already had before this pass; see
//! the `B-ErrorADT` sanctioned divergence).
//!
//! Backed by `builtin_runtime_enum` (mirrors `Order`/`IpeOrder`):
//! `Error`'s sole constructor shares its name with the type
//! (`ipe_lower`'s `enum_variants` table), so it emits as the tuple variant
//! `IpeError::Error(kind, info)` via the SAME generic constructor/pattern
//! path `IpeMaybe::Just`/`IpeResult::Ok` already use — no new emitter
//! mechanism needed, just table rows. `ErrorDetails` is registered the same
//! way (`builtin_runtime_enum("ErrorDetails") -> "IpeErrorDetails"`).

use std::fmt;

use crate::core::IpeMaybe;

/// Ipê's `ErrorKind` — 12 nullary variants.
///
/// Repr(u8) for a compact, sound, exhaustively-matched runtime type (mirrors
/// `IpeOrder`'s convention). The order is append-only: each discriminant is
/// stable across serde and the JS port. Canon's `BuiltinUnion` ctor table
/// (`src/compiler/canon/src/builtins.rs`) mirrors it, and
/// `src/compiler/canon/tests/error_kind_agreement.rs` pins the two equal.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[repr(u8)]
pub enum IpeErrorKind {
    Io = 0,
    Network = 1,
    Ffi = 2,
    Decode = 3,
    Timeout = 4,
    NotFound = 5,
    PermissionDenied = 6,
    InvalidInput = 7,
    Conflict = 8,
    Unavailable = 9,
    Unexpected = 10,
    /// A declared ceiling (a size, count or depth bound on one input) turned
    /// the input back; a larger bound would accept the same input.
    LimitExceeded = 11,
}

impl IpeErrorKind {
    /// Every kind, in discriminant order.
    pub const ALL: [Self; 12] = [
        Self::Io,
        Self::Network,
        Self::Ffi,
        Self::Decode,
        Self::Timeout,
        Self::NotFound,
        Self::PermissionDenied,
        Self::InvalidInput,
        Self::Conflict,
        Self::Unavailable,
        Self::Unexpected,
        Self::LimitExceeded,
    ];

    /// Renders the reference design's `"<Kind>: "` prefix (`Error.toString`,
    /// `"<Kind>: <message>"`).
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Io => "Io",
            Self::Network => "Network",
            Self::Ffi => "Ffi",
            Self::Decode => "Decode",
            Self::Timeout => "Timeout",
            Self::NotFound => "NotFound",
            Self::PermissionDenied => "PermissionDenied",
            Self::InvalidInput => "InvalidInput",
            Self::Conflict => "Conflict",
            Self::Unavailable => "Unavailable",
            Self::Unexpected => "Unexpected",
            Self::LimitExceeded => "LimitExceeded",
        }
    }
}

/// Ipê's `PanicInfo` — `FfiPanic`'s payload: `{ message : String, stack :
/// List String }`.
#[derive(Clone, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct IpePanicInfo {
    pub message: String,
    pub stack: Vec<String>,
}

/// Ipê's `TypeInfo` — `TypeMismatch`'s payload: `{ expected : String, actual
/// : String }`.
#[derive(Clone, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct IpeTypeInfo {
    pub expected: String,
    pub actual: String,
}

/// Ipê's `ErrorDetails` — the 5-variant enrichment union. Constructor names
/// match Ipê source verbatim
/// (`ipe_backend_rust`'s `builtin_runtime_enum("ErrorDetails")` routes
/// `FfiPanic` / `TypeMismatch` / `HttpStatus` / `JsonDecode` / `Custom`
/// straight to these variants — no synthetic `EnumDef`).
#[derive(Clone, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum IpeErrorDetails {
    FfiPanic(IpePanicInfo),
    TypeMismatch(IpeTypeInfo),
    HttpStatus(i64),
    JsonDecode(String),
    Custom(String),
}

/// Ipê's `ErrorInfo` — `{ message : String, details : Maybe ErrorDetails }`.
///
/// No `#[derive(Eq)]`: `IpeMaybe<T>` (the `details` field's carrier) derives
/// only `PartialEq`, not `Eq` (see `core.rs`'s `IpeMaybe` doc), so `Eq` here
/// would fail to compile. `PartialEq` is unaffected and is what
/// `ir_type_is_derivable`'s Rust-side gate actually requires.
#[derive(Clone, PartialEq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct IpeErrorInfo {
    pub message: String,
    pub details: IpeMaybe<IpeErrorDetails>,
}

/// Ipê's `Error` — `Error ErrorKind ErrorInfo`, a single tuple-variant enum
/// (constructor name == type name, matching `ipe_lower`'s registration) so
/// the generic `builtin_runtime_enum` constructor/pattern path handles it
/// exactly like `IpeMaybe::Just`/`IpeResult::Ok`.
///
/// No `#[derive(Eq)]` (see `IpeErrorInfo`'s doc — it carries a `IpeMaybe`
/// field, and `IpeMaybe` is `PartialEq`-only).
#[derive(Clone, PartialEq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum IpeError {
    Error(IpeErrorKind, IpeErrorInfo),
}

impl IpeError {
    /// Every message constructor defaults `details = Nothing`, mirroring the
    /// reference design's `mkInfo` smart constructor.
    fn with(kind: IpeErrorKind, message: String) -> Self {
        Self::Error(
            kind,
            IpeErrorInfo {
                message,
                details: IpeMaybe::Nothing,
            },
        )
    }

    #[must_use]
    pub fn io(message: String) -> Self {
        Self::with(IpeErrorKind::Io, message)
    }
    #[must_use]
    pub fn network(message: String) -> Self {
        Self::with(IpeErrorKind::Network, message)
    }
    #[must_use]
    pub fn ffi(message: String) -> Self {
        Self::with(IpeErrorKind::Ffi, message)
    }
    #[must_use]
    pub fn decode(message: String) -> Self {
        Self::with(IpeErrorKind::Decode, message)
    }
    #[must_use]
    pub fn invalid_input(message: String) -> Self {
        Self::with(IpeErrorKind::InvalidInput, message)
    }
    #[must_use]
    pub fn conflict(message: String) -> Self {
        Self::with(IpeErrorKind::Conflict, message)
    }
    #[must_use]
    pub fn unavailable(message: String) -> Self {
        Self::with(IpeErrorKind::Unavailable, message)
    }
    #[must_use]
    pub fn unexpected(message: String) -> Self {
        Self::with(IpeErrorKind::Unexpected, message)
    }
    /// A declared ceiling turned the input back (`ErrorKind.LimitExceeded`).
    #[must_use]
    pub fn limit_exceeded(message: impl Into<String>) -> Self {
        Self::with(IpeErrorKind::LimitExceeded, message.into())
    }
    /// Nullary in the Ipê surface — pre-built, fixed message.
    #[must_use]
    pub fn timeout() -> Self {
        Self::with(IpeErrorKind::Timeout, "operation timed out".to_owned())
    }
    #[must_use]
    pub fn not_found() -> Self {
        Self::with(IpeErrorKind::NotFound, "not found".to_owned())
    }
    #[must_use]
    pub fn permission_denied() -> Self {
        Self::with(
            IpeErrorKind::PermissionDenied,
            "permission denied".to_owned(),
        )
    }

    /// Ipê `Error.withMessage : String -> Error -> Error` — replaces the
    /// message, keeps the kind.
    #[must_use]
    pub fn with_message(self, message: String) -> Self {
        let Self::Error(kind, _) = self;
        Self::with(kind, message)
    }

    /// Ipê `Error.toString : Error -> String` — `"<Kind>: <message>"`.
    #[must_use]
    pub fn to_ipe_string(&self) -> String {
        let Self::Error(kind, info) = self;
        format!("{}: {}", kind.label(), info.message)
    }

    /// Ipê `Error.isRetryable : Error -> Bool` — `True` only for `Timeout`,
    /// `Network` and `Unavailable`.
    ///
    /// Those three are the kinds a caller can reasonably back off and retry.
    /// The other nine fail again on the same input: `LimitExceeded` in
    /// particular is a deterministic refusal of that input by its bound.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        let Self::Error(kind, _) = self;
        matches!(
            kind,
            IpeErrorKind::Timeout | IpeErrorKind::Network | IpeErrorKind::Unavailable
        )
    }

    /// Ipê `Error.withDetails : ErrorDetails -> Error -> Error` — keeps kind
    /// and message, sets `details = Just <details>`.
    /// This is the sanctioned way to attach `ErrorDetails` to a live `Error`
    /// value from Ipê source (see module doc for why raw record-literal
    /// construction of `ErrorInfo`/`PanicInfo`/`TypeInfo` is not supported).
    #[must_use]
    pub fn with_details(self, details: IpeErrorDetails) -> Self {
        let Self::Error(kind, info) = self;
        Self::Error(
            kind,
            IpeErrorInfo {
                message: info.message,
                details: IpeMaybe::Just(details),
            },
        )
    }
}

impl fmt::Display for IpeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_ipe_string())
    }
}

// ── Ipe.Error kernels ────────────────────────────────
// Each message constructor classifies its own `ErrorKind` at construction,
// rather than sharing one string-identity runtime symbol across all nine.

#[must_use]
pub fn ipe_error_unexpected(msg: String) -> IpeError {
    IpeError::unexpected(msg)
}
#[must_use]
pub fn ipe_error_invalid_input(msg: String) -> IpeError {
    IpeError::invalid_input(msg)
}
#[must_use]
pub fn ipe_error_io(msg: String) -> IpeError {
    IpeError::io(msg)
}
#[must_use]
pub fn ipe_error_network(msg: String) -> IpeError {
    IpeError::network(msg)
}
#[must_use]
pub fn ipe_error_ffi(msg: String) -> IpeError {
    IpeError::ffi(msg)
}
#[must_use]
pub fn ipe_error_decode(msg: String) -> IpeError {
    IpeError::decode(msg)
}
#[must_use]
pub fn ipe_error_conflict(msg: String) -> IpeError {
    IpeError::conflict(msg)
}
#[must_use]
pub fn ipe_error_unavailable(msg: String) -> IpeError {
    IpeError::unavailable(msg)
}
/// `Error.limitExceeded : String -> Error` — a declared-ceiling refusal.
#[must_use]
pub fn ipe_error_limit_exceeded(msg: String) -> IpeError {
    IpeError::limit_exceeded(msg)
}
/// `Error.timeout : Error` — canonical timeout error.
#[must_use]
pub fn ipe_error_timeout() -> IpeError {
    IpeError::timeout()
}
/// `Error.notFound : Error` — canonical not-found error.
#[must_use]
pub fn ipe_error_not_found() -> IpeError {
    IpeError::not_found()
}
/// `Error.permissionDenied : Error` — canonical permission-denied error.
#[must_use]
pub fn ipe_error_permission_denied() -> IpeError {
    IpeError::permission_denied()
}
/// `Error.withMessage : String -> Error -> Error`.
#[must_use]
pub fn ipe_error_with_message(msg: String, old: IpeError) -> IpeError {
    old.with_message(msg)
}
/// `Error.isRetryable : Error -> Bool`.
#[must_use]
pub fn ipe_error_is_retryable(e: IpeError) -> bool {
    e.is_retryable()
}
/// `Error.withDetails : ErrorDetails -> Error -> Error`.
#[must_use]
pub fn ipe_error_with_details(details: IpeErrorDetails, old: IpeError) -> IpeError {
    old.with_details(details)
}
/// `Error.kind : Error -> ErrorKind` — the classification carried by an error.
#[must_use]
pub fn ipe_error_kind(e: IpeError) -> IpeErrorKind {
    let IpeError::Error(kind, _) = e;
    kind
}
/// `Error.message : Error -> String` — the human-readable message, without the
/// `"<Kind>: "` prefix `Error.toString` adds.
#[must_use]
pub fn ipe_error_message(e: IpeError) -> String {
    let IpeError::Error(_, info) = e;
    info.message
}
/// `Error.kindName : ErrorKind -> String` — the stable variant name (`"Io"`,
/// `"Network"`, …), the same label `Error.toString` prefixes with.
#[must_use]
pub fn ipe_error_kind_name(kind: IpeErrorKind) -> String {
    kind.label().to_owned()
}

// `Error.toString` routes through the shared Stringify-bounded mechanism
// (any `Show`-obligated type, not an Error-specific kernel — see the
// `Interpolate | ErrorToString` direct-build arm in `ipe_types`' constrain).
// Without this impl the autoref-specialization fallback would render via
// `#[derive(Debug)]` (`Error(Io, IpeErrorInfo { message: ".." })`)
// instead of the reference design's `"<Kind>: <message>"` format.
impl crate::stringify::IpeStringify for IpeError {
    fn ipe_show(&self) -> String {
        self.to_ipe_string()
    }
}

/// Compatibility bridge: kernel call sites across the runtime that produce a
/// bare `String` error keep compiling — `?`/`.into()` on a `String` yields an
/// `Unexpected`-classified `Error` instead of losing type information. Such
/// call sites should migrate to a properly-classified constructor
/// (`IpeError::io`, `::network`, …).
impl From<String> for IpeError {
    fn from(message: String) -> Self {
        Self::unexpected(message)
    }
}

impl From<&str> for IpeError {
    fn from(message: &str) -> Self {
        Self::unexpected(message.to_owned())
    }
}

/// A generic `E: From<String>` error sink (as `tui_app`/`tui_app_ui` take)
/// that can ALSO classify a refusal as `Unavailable` — the retryable kind —
/// instead of folding every string into `Unexpected` through the blanket
/// `From<String>` bridge above. Implemented only for `IpeError`: no call site
/// instantiates those generic functions with any other `E`, so the extra
/// bound costs nothing while keeping the "no terminal" refusal correctly
/// kinded for the one type that ever carries it.
pub trait FromUnavailable {
    fn from_unavailable(message: String) -> Self;
}

impl FromUnavailable for IpeError {
    fn from_unavailable(message: String) -> Self {
        Self::unavailable(message)
    }
}

/// A generic `E: From<String>` error sink that can classify a refusal as
/// `LimitExceeded`.
///
/// The blanket `From<String>` bridge yields `Unexpected`; a declared-ceiling
/// refusal reaching such a sink goes through this trait instead, so it keeps
/// its kind. Implemented for `IpeError`, as `FromUnavailable` is (unit tests
/// add the bare `String` sink below).
pub trait FromLimitExceeded {
    fn from_limit_exceeded(message: String) -> Self;
}

impl FromLimitExceeded for IpeError {
    fn from_limit_exceeded(message: String) -> Self {
        Self::limit_exceeded(message)
    }
}

/// A unit test's bare-`String` error sink keeps only the message.
///
/// Kind assertions in tests use an `IpeError` sink.
#[cfg(test)]
impl FromLimitExceeded for String {
    fn from_limit_exceeded(message: String) -> Self {
        message
    }
}

/// A declared ceiling turned the input back.
///
/// The message names the ceiling and, when one exists, the setting that
/// raises it. The value reaches an error sink only as `LimitExceeded`: it has
/// no conversion into `String`, so it cannot fall into the `From<String>`
/// bridge that yields `Unexpected`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LimitRefusal(String);

impl LimitRefusal {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.0
    }

    /// Prefixes the message with the refusing operation, keeping the kind.
    #[must_use]
    pub fn context(self, operation: &str) -> Self {
        Self(format!("{operation}: {}", self.0))
    }

    /// The refusal in a generic error sink, kinded `LimitExceeded`.
    #[must_use]
    pub fn into_error<E: FromLimitExceeded>(self) -> E {
        E::from_limit_exceeded(self.0)
    }
}

impl From<LimitRefusal> for IpeError {
    fn from(refusal: LimitRefusal) -> Self {
        Self::limit_exceeded(refusal.0)
    }
}

/// A kernel failure whose kind is decided where it is built.
///
/// A bare `String` converts into `Unexpected`, so a `?` on an OS or library
/// error keeps that classification; a ceiling refusal is built as
/// `LimitExceeded` and keeps that kind through `into_error`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KernelFailure {
    /// An unclassified failure (an OS or library error).
    Unexpected(String),
    /// A declared ceiling turned the input back.
    LimitExceeded(LimitRefusal),
}

impl KernelFailure {
    #[must_use]
    pub fn message(&self) -> &str {
        match self {
            Self::Unexpected(message) => message,
            Self::LimitExceeded(refusal) => refusal.message(),
        }
    }

    /// Prefixes the message with the failing operation, keeping the kind.
    #[must_use]
    pub fn context(self, operation: &str) -> Self {
        match self {
            Self::Unexpected(message) => Self::Unexpected(format!("{operation}: {message}")),
            Self::LimitExceeded(refusal) => Self::LimitExceeded(refusal.context(operation)),
        }
    }

    /// The failure in a generic error sink, each variant under its own kind.
    #[must_use]
    pub fn into_error<E: From<String> + FromLimitExceeded>(self) -> E {
        match self {
            Self::Unexpected(message) => E::from(message),
            Self::LimitExceeded(refusal) => refusal.into_error(),
        }
    }
}

impl From<String> for KernelFailure {
    fn from(message: String) -> Self {
        Self::Unexpected(message)
    }
}

impl From<LimitRefusal> for KernelFailure {
    fn from(refusal: LimitRefusal) -> Self {
        Self::LimitExceeded(refusal)
    }
}

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod tests {
    use super::*;

    #[test]
    fn constructors_carry_kind_and_message() {
        let e = IpeError::io("disk full".to_owned());
        assert_eq!(e.to_ipe_string(), "Io: disk full");
        assert!(!e.is_retryable());
    }

    #[test]
    fn nullary_constructors_have_fixed_messages() {
        assert_eq!(
            IpeError::timeout().to_ipe_string(),
            "Timeout: operation timed out"
        );
        assert_eq!(IpeError::not_found().to_ipe_string(), "NotFound: not found");
        assert_eq!(
            IpeError::permission_denied().to_ipe_string(),
            "PermissionDenied: permission denied"
        );
    }

    #[test]
    fn retryable_kinds_are_exactly_timeout_network_unavailable() {
        for kind in IpeErrorKind::ALL {
            let e = IpeError::with(kind, String::new());
            let expected = matches!(
                kind,
                IpeErrorKind::Timeout | IpeErrorKind::Network | IpeErrorKind::Unavailable
            );
            assert_eq!(e.is_retryable(), expected, "{kind:?}");
        }
        assert!(!IpeError::limit_exceeded("x").is_retryable());
    }

    #[test]
    fn kinds_are_append_only_with_stable_discriminants() {
        let pinned: [(IpeErrorKind, u8, &str); 12] = [
            (IpeErrorKind::Io, 0, "Io"),
            (IpeErrorKind::Network, 1, "Network"),
            (IpeErrorKind::Ffi, 2, "Ffi"),
            (IpeErrorKind::Decode, 3, "Decode"),
            (IpeErrorKind::Timeout, 4, "Timeout"),
            (IpeErrorKind::NotFound, 5, "NotFound"),
            (IpeErrorKind::PermissionDenied, 6, "PermissionDenied"),
            (IpeErrorKind::InvalidInput, 7, "InvalidInput"),
            (IpeErrorKind::Conflict, 8, "Conflict"),
            (IpeErrorKind::Unavailable, 9, "Unavailable"),
            (IpeErrorKind::Unexpected, 10, "Unexpected"),
            (IpeErrorKind::LimitExceeded, 11, "LimitExceeded"),
        ];
        assert_eq!(IpeErrorKind::ALL.len(), pinned.len());
        for (index, (kind, discriminant, label)) in pinned.into_iter().enumerate() {
            assert_eq!(IpeErrorKind::ALL.get(index), Some(&kind));
            assert_eq!(kind as u8, discriminant);
            assert_eq!(kind.label(), label);
        }
        let mut labels: Vec<&str> = IpeErrorKind::ALL.iter().map(|k| k.label()).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), IpeErrorKind::ALL.len(), "label is injective");
    }

    #[test]
    fn limit_exceeded_carries_its_kind_and_prefix() {
        let e = ipe_error_limit_exceeded("x".to_owned());
        assert_eq!(ipe_error_kind(e.clone()), IpeErrorKind::LimitExceeded);
        assert_eq!(e.to_ipe_string(), "LimitExceeded: x");
        assert!(!ipe_error_is_retryable(e.clone()));
        assert_eq!(
            <IpeError as FromLimitExceeded>::from_limit_exceeded("x".to_owned()),
            e
        );
        assert_eq!(
            ipe_error_kind_name(IpeErrorKind::LimitExceeded),
            "LimitExceeded"
        );
    }

    #[test]
    fn kernel_failure_keeps_each_kind_through_a_generic_sink() {
        let limit = KernelFailure::from(LimitRefusal::new("over 4 bytes")).context("Op");
        let other = KernelFailure::from("disk gone".to_owned()).context("Op");
        assert_eq!(limit.message(), "Op: over 4 bytes");
        assert_eq!(
            limit.into_error::<IpeError>().to_ipe_string(),
            "LimitExceeded: Op: over 4 bytes"
        );
        assert_eq!(
            other.into_error::<IpeError>().to_ipe_string(),
            "Unexpected: Op: disk gone"
        );
        assert_eq!(
            IpeError::from(LimitRefusal::new("x")),
            IpeError::limit_exceeded("x")
        );
    }

    #[cfg(feature = "serde")]
    #[test]
    fn kinds_round_trip_through_serde_by_variant_name() {
        for kind in IpeErrorKind::ALL {
            let wire = serde_json::to_string(&kind).expect("serialize");
            assert_eq!(wire, format!("\"{}\"", kind.label()));
            let back: IpeErrorKind = serde_json::from_str(&wire).expect("deserialize");
            assert_eq!(back, kind);
        }
    }

    #[test]
    fn with_message_replaces_message_keeps_kind() {
        let e = IpeError::network("timeout".to_owned()).with_message("retry later".to_owned());
        assert_eq!(e.to_ipe_string(), "Network: retry later");
    }

    #[test]
    fn from_string_classifies_as_unexpected() {
        let e: IpeError = "legacy bare string error".to_owned().into();
        assert_eq!(e.to_ipe_string(), "Unexpected: legacy bare string error");
    }

    #[test]
    fn pattern_match_destructures_kind_and_message() {
        let e = IpeError::conflict("duplicate key".to_owned());
        let IpeError::Error(kind, info) = &e;
        assert_eq!(*kind, IpeErrorKind::Conflict);
        assert_eq!(info.message, "duplicate key");
    }

    #[test]
    fn message_constructors_default_details_to_nothing() {
        let e = IpeError::io("disk full".to_owned());
        let IpeError::Error(_, info) = &e;
        assert_eq!(info.details, IpeMaybe::Nothing);
    }

    #[test]
    fn with_details_sets_just_keeps_kind_and_message() {
        let e = IpeError::io("disk full".to_owned()).with_details(IpeErrorDetails::HttpStatus(404));
        let IpeError::Error(kind, info) = &e;
        assert_eq!(*kind, IpeErrorKind::Io);
        assert_eq!(info.message, "disk full");
        assert_eq!(
            info.details,
            IpeMaybe::Just(IpeErrorDetails::HttpStatus(404))
        );
    }

    #[test]
    fn kind_extracts_the_classification() {
        assert_eq!(
            ipe_error_kind(IpeError::io("x".to_owned())),
            IpeErrorKind::Io
        );
        assert_eq!(ipe_error_kind(IpeError::timeout()), IpeErrorKind::Timeout);
    }

    #[test]
    fn message_extracts_the_bare_message() {
        assert_eq!(
            ipe_error_message(IpeError::io("disk full".to_owned())),
            "disk full"
        );
        assert_eq!(ipe_error_message(IpeError::not_found()), "not found");
    }

    #[test]
    fn kind_name_renders_the_stable_label() {
        assert_eq!(ipe_error_kind_name(IpeErrorKind::Io), "Io");
        assert_eq!(
            ipe_error_kind_name(IpeErrorKind::PermissionDenied),
            "PermissionDenied"
        );
        assert_eq!(ipe_error_kind_name(IpeErrorKind::Unexpected), "Unexpected");
    }

    #[test]
    fn error_details_round_trips_all_five_variants() {
        let cases = [
            IpeErrorDetails::FfiPanic(IpePanicInfo {
                message: "panic!".to_owned(),
                stack: vec!["frame1".to_owned(), "frame2".to_owned()],
            }),
            IpeErrorDetails::TypeMismatch(IpeTypeInfo {
                expected: "Int".to_owned(),
                actual: "String".to_owned(),
            }),
            IpeErrorDetails::HttpStatus(500),
            IpeErrorDetails::JsonDecode("unexpected token".to_owned()),
            IpeErrorDetails::Custom("custom detail".to_owned()),
        ];
        for details in cases {
            let e = IpeError::unexpected("boom".to_owned()).with_details(details.clone());
            let IpeError::Error(_, info) = &e;
            assert_eq!(info.details, IpeMaybe::Just(details));
        }
    }
}
