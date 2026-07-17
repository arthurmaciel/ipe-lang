//! Sky.Core.Error: the rich, typed `Error` ADT.
//!
//! Ported from the ancestor Go/Haskell design (`sky-stdlib/Sky/Core/Error.sky`
//! in the reference repo): `Error = Error ErrorKind ErrorInfo`, an 11-variant
//! `ErrorKind` classification, message-carrying `ErrorInfo`, and the 5-variant
//! `ErrorDetails` union (`FfiPanic`/`TypeMismatch`/`HttpStatus`/`JsonDecode`/
//! `Custom`) carried optionally on `ErrorInfo.details : Maybe ErrorDetails`.
//!
//! Kind-based classification (`isRetryable`, pattern matching, `toString`)
//! and the `details` enrichment are both fully real and load-bearing today.
//! `Error.withDetails` is the sanctioned way to attach `ErrorDetails` to a
//! live `Error` value — raw Sky-source construction of `ErrorInfo`/
//! `PanicInfo`/`TypeInfo` record literals is NOT supported (those are
//! anonymous structural records at the type level, so a literal lowers to a
//! project-local synthesized struct, not this module's concrete
//! `SkyErrorInfo`/`SkyPanicInfo`/`SkyTypeInfo` — the same limitation
//! `ErrorInfo` itself already had before this pass; see
//! `docs/divergences-from-sky.md`'s `B-ErrorADT` entry).
//!
//! Backed by `builtin_runtime_enum` (mirrors `Order`/`SkyOrder`):
//! `Error`'s sole constructor shares its name with the type
//! (`sky_lower`'s `enum_variants` table), so it emits as the tuple variant
//! `SkyError::Error(kind, info)` via the SAME generic constructor/pattern
//! path `SkyMaybe::Just`/`SkyResult::Ok` already use — no new emitter
//! mechanism needed, just table rows. `ErrorDetails` is registered the same
//! way (`builtin_runtime_enum("ErrorDetails") -> "SkyErrorDetails"`).

use std::fmt;

use crate::sky_runtime::core::SkyMaybe;

/// Sky's `ErrorKind` — 11 nullary variants. Repr(u8) for a compact, sound,
/// exhaustively-matched runtime type (mirrors `SkyOrder`'s convention).
/// Variant order matches canon's registration (`crates/sky_canon/src/env.rs`,
/// "E-12") — do not reorder without updating that table.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum SkyErrorKind {
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
}

impl SkyErrorKind {
    /// Renders the reference design's `"<Kind>: "` prefix (`Error.toString`,
    /// `"<Kind>: <message>"`).
    const fn label(self) -> &'static str {
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
        }
    }
}

/// Sky's `PanicInfo` — `FfiPanic`'s payload: `{ message : String, stack :
/// List String }`.
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub struct SkyPanicInfo {
    pub message: String,
    pub stack: Vec<String>,
}

/// Sky's `TypeInfo` — `TypeMismatch`'s payload: `{ expected : String, actual
/// : String }`.
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub struct SkyTypeInfo {
    pub expected: String,
    pub actual: String,
}

/// Sky's `ErrorDetails` — the 5-variant enrichment union. Constructor names
/// match Sky source verbatim
/// (`sky_backend_rust`'s `builtin_runtime_enum("ErrorDetails")` routes
/// `FfiPanic` / `TypeMismatch` / `HttpStatus` / `JsonDecode` / `Custom`
/// straight to these variants — no synthetic `EnumDef`).
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum SkyErrorDetails {
    FfiPanic(SkyPanicInfo),
    TypeMismatch(SkyTypeInfo),
    HttpStatus(i64),
    JsonDecode(String),
    Custom(String),
}

/// Sky's `ErrorInfo` — `{ message : String, details : Maybe ErrorDetails }`.
///
/// No `#[derive(Eq)]`: `SkyMaybe<T>` (the `details` field's carrier) derives
/// only `PartialEq`, not `Eq` (see `core.rs`'s `SkyMaybe` doc), so `Eq` here
/// would fail to compile. `PartialEq` is unaffected and is what
/// `ir_type_is_derivable`'s Rust-side gate actually requires.
#[derive(Clone, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub struct SkyErrorInfo {
    pub message: String,
    pub details: SkyMaybe<SkyErrorDetails>,
}

/// Sky's `Error` — `Error ErrorKind ErrorInfo`, a single tuple-variant enum
/// (constructor name == type name, matching `sky_lower`'s registration) so
/// the generic `builtin_runtime_enum` constructor/pattern path handles it
/// exactly like `SkyMaybe::Just`/`SkyResult::Ok`.
///
/// No `#[derive(Eq)]` (see `SkyErrorInfo`'s doc — it carries a `SkyMaybe`
/// field, and `SkyMaybe` is `PartialEq`-only).
#[derive(Clone, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub enum SkyError {
    Error(SkyErrorKind, SkyErrorInfo),
}

impl SkyError {
    /// Every message constructor defaults `details = Nothing`, mirroring the
    /// reference design's `mkInfo` smart constructor.
    fn with(kind: SkyErrorKind, message: String) -> Self {
        Self::Error(kind, SkyErrorInfo { message, details: SkyMaybe::Nothing })
    }

    pub fn io(message: String) -> Self {
        Self::with(SkyErrorKind::Io, message)
    }
    pub fn network(message: String) -> Self {
        Self::with(SkyErrorKind::Network, message)
    }
    pub fn ffi(message: String) -> Self {
        Self::with(SkyErrorKind::Ffi, message)
    }
    pub fn decode(message: String) -> Self {
        Self::with(SkyErrorKind::Decode, message)
    }
    pub fn invalid_input(message: String) -> Self {
        Self::with(SkyErrorKind::InvalidInput, message)
    }
    pub fn conflict(message: String) -> Self {
        Self::with(SkyErrorKind::Conflict, message)
    }
    pub fn unavailable(message: String) -> Self {
        Self::with(SkyErrorKind::Unavailable, message)
    }
    pub fn unexpected(message: String) -> Self {
        Self::with(SkyErrorKind::Unexpected, message)
    }
    /// Nullary in the Sky surface — pre-built, fixed message.
    pub fn timeout() -> Self {
        Self::with(SkyErrorKind::Timeout, "operation timed out".to_owned())
    }
    pub fn not_found() -> Self {
        Self::with(SkyErrorKind::NotFound, "not found".to_owned())
    }
    pub fn permission_denied() -> Self {
        Self::with(SkyErrorKind::PermissionDenied, "permission denied".to_owned())
    }

    /// Sky `Error.withMessage : String -> Error -> Error` — replaces the
    /// message, keeps the kind.
    #[must_use]
    pub fn with_message(self, message: String) -> Self {
        let Self::Error(kind, _) = self;
        Self::with(kind, message)
    }

    /// Sky `Error.toString : Error -> String` — `"<Kind>: <message>"`.
    #[must_use]
    pub fn to_sky_string(&self) -> String {
        let Self::Error(kind, info) = self;
        format!("{}: {}", kind.label(), info.message)
    }

    /// Sky `Error.isRetryable : Error -> Bool` — `True` only for the three
    /// kinds a caller can reasonably back off and retry.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        let Self::Error(kind, _) = self;
        matches!(
            kind,
            SkyErrorKind::Timeout | SkyErrorKind::Network | SkyErrorKind::Unavailable
        )
    }

    /// Sky `Error.withDetails : ErrorDetails -> Error -> Error` — keeps kind
    /// and message, sets `details = Just <details>`.
    /// This is the sanctioned way to attach `ErrorDetails` to a live `Error`
    /// value from Sky source (see module doc for why raw record-literal
    /// construction of `ErrorInfo`/`PanicInfo`/`TypeInfo` is not supported).
    #[must_use]
    pub fn with_details(self, details: SkyErrorDetails) -> Self {
        let Self::Error(kind, info) = self;
        Self::Error(kind, SkyErrorInfo { message: info.message, details: SkyMaybe::Just(details) })
    }
}

impl fmt::Display for SkyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_sky_string())
    }
}

// ── Sky.Core.Error kernels ────────────────────────────────
// Each message constructor classifies its own `ErrorKind` at construction,
// rather than sharing one string-identity runtime symbol across all eight.

#[must_use]
pub fn sky_error_unexpected(msg: String) -> SkyError {
    SkyError::unexpected(msg)
}
#[must_use]
pub fn sky_error_invalid_input(msg: String) -> SkyError {
    SkyError::invalid_input(msg)
}
#[must_use]
pub fn sky_error_io(msg: String) -> SkyError {
    SkyError::io(msg)
}
#[must_use]
pub fn sky_error_network(msg: String) -> SkyError {
    SkyError::network(msg)
}
#[must_use]
pub fn sky_error_ffi(msg: String) -> SkyError {
    SkyError::ffi(msg)
}
#[must_use]
pub fn sky_error_decode(msg: String) -> SkyError {
    SkyError::decode(msg)
}
#[must_use]
pub fn sky_error_conflict(msg: String) -> SkyError {
    SkyError::conflict(msg)
}
#[must_use]
pub fn sky_error_unavailable(msg: String) -> SkyError {
    SkyError::unavailable(msg)
}
/// `Error.timeout : Error` — canonical timeout error.
#[must_use]
pub fn sky_error_timeout() -> SkyError {
    SkyError::timeout()
}
/// `Error.notFound : Error` — canonical not-found error.
#[must_use]
pub fn sky_error_not_found() -> SkyError {
    SkyError::not_found()
}
/// `Error.permissionDenied : Error` — canonical permission-denied error.
#[must_use]
pub fn sky_error_permission_denied() -> SkyError {
    SkyError::permission_denied()
}
/// `Error.withMessage : String -> Error -> Error`.
#[must_use]
pub fn sky_error_with_message(msg: String, old: SkyError) -> SkyError {
    old.with_message(msg)
}
/// `Error.isRetryable : Error -> Bool`.
#[must_use]
pub fn sky_error_is_retryable(e: SkyError) -> bool {
    e.is_retryable()
}
/// `Error.withDetails : ErrorDetails -> Error -> Error`.
#[must_use]
pub fn sky_error_with_details(details: SkyErrorDetails, old: SkyError) -> SkyError {
    old.with_details(details)
}

// `Error.toString` routes through the shared Stringify-bounded mechanism
// (any `Show`-obligated type, not an Error-specific kernel — see
// `crates/sky_types/src/constrain.rs`'s `BasicsToString | ErrorToString`
// special case). Without this impl the autoref-specialization fallback would
// render via `#[derive(Debug)]` (`Error(Io, SkyErrorInfo { message: ".." })`)
// instead of the reference design's `"<Kind>: <message>"` format.
impl crate::sky_runtime::stringify::SkyStringify for SkyError {
    fn sky_show(&self) -> String {
        self.to_sky_string()
    }
}

/// Compatibility bridge: kernel call sites across the runtime that produce a
/// bare `String` error keep compiling — `?`/`.into()` on a `String` yields an
/// `Unexpected`-classified `Error` instead of losing type information. Such
/// call sites should migrate to a properly-classified constructor
/// (`SkyError::io`, `::network`, …).
impl From<String> for SkyError {
    fn from(message: String) -> Self {
        Self::unexpected(message)
    }
}

impl From<&str> for SkyError {
    fn from(message: &str) -> Self {
        Self::unexpected(message.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors_carry_kind_and_message() {
        let e = SkyError::io("disk full".to_owned());
        assert_eq!(e.to_sky_string(), "Io: disk full");
        assert!(!e.is_retryable());
    }

    #[test]
    fn nullary_constructors_have_fixed_messages() {
        assert_eq!(SkyError::timeout().to_sky_string(), "Timeout: operation timed out");
        assert_eq!(SkyError::not_found().to_sky_string(), "NotFound: not found");
        assert_eq!(
            SkyError::permission_denied().to_sky_string(),
            "PermissionDenied: permission denied"
        );
    }

    #[test]
    fn retryable_kinds_are_exactly_timeout_network_unavailable() {
        assert!(SkyError::timeout().is_retryable());
        assert!(SkyError::network(String::new()).is_retryable());
        assert!(SkyError::unavailable(String::new()).is_retryable());
        assert!(!SkyError::io(String::new()).is_retryable());
        assert!(!SkyError::unexpected(String::new()).is_retryable());
        assert!(!SkyError::conflict(String::new()).is_retryable());
    }

    #[test]
    fn with_message_replaces_message_keeps_kind() {
        let e = SkyError::network("timeout".to_owned()).with_message("retry later".to_owned());
        assert_eq!(e.to_sky_string(), "Network: retry later");
    }

    #[test]
    fn from_string_classifies_as_unexpected() {
        let e: SkyError = "legacy bare string error".to_owned().into();
        assert_eq!(e.to_sky_string(), "Unexpected: legacy bare string error");
    }

    #[test]
    fn pattern_match_destructures_kind_and_message() {
        let e = SkyError::conflict("duplicate key".to_owned());
        let SkyError::Error(kind, info) = &e;
        assert_eq!(*kind, SkyErrorKind::Conflict);
        assert_eq!(info.message, "duplicate key");
    }

    #[test]
    fn message_constructors_default_details_to_nothing() {
        let e = SkyError::io("disk full".to_owned());
        let SkyError::Error(_, info) = &e;
        assert_eq!(info.details, SkyMaybe::Nothing);
    }

    #[test]
    fn with_details_sets_just_keeps_kind_and_message() {
        let e = SkyError::io("disk full".to_owned())
            .with_details(SkyErrorDetails::HttpStatus(404));
        let SkyError::Error(kind, info) = &e;
        assert_eq!(*kind, SkyErrorKind::Io);
        assert_eq!(info.message, "disk full");
        assert_eq!(info.details, SkyMaybe::Just(SkyErrorDetails::HttpStatus(404)));
    }

    #[test]
    fn error_details_round_trips_all_five_variants() {
        let cases = [
            SkyErrorDetails::FfiPanic(SkyPanicInfo {
                message: "panic!".to_owned(),
                stack: vec!["frame1".to_owned(), "frame2".to_owned()],
            }),
            SkyErrorDetails::TypeMismatch(SkyTypeInfo {
                expected: "Int".to_owned(),
                actual: "String".to_owned(),
            }),
            SkyErrorDetails::HttpStatus(500),
            SkyErrorDetails::JsonDecode("unexpected token".to_owned()),
            SkyErrorDetails::Custom("custom detail".to_owned()),
        ];
        for details in cases {
            let e = SkyError::unexpected("boom".to_owned()).with_details(details.clone());
            let SkyError::Error(_, info) = &e;
            assert_eq!(info.details, SkyMaybe::Just(details));
        }
    }
}
