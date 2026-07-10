//! Sky.Core.Error: the rich, typed `Error` ADT (backlog #85/#160).
//!
//! Ported from the ancestor Go/Haskell design (`sky-stdlib/Sky/Core/Error.sky`
//! in the reference repo): `Error = Error ErrorKind ErrorInfo`, an 11-variant
//! `ErrorKind` classification, and message-carrying `ErrorInfo`.
//!
//! **Scope of this pass:** `ErrorInfo` carries only `message` — the reference
//! design's `ErrorDetails` union (`FfiPanic`/`TypeMismatch`/`HttpStatus`/
//! `JsonDecode`/`Custom`, each with its own nested payload record) is filed as
//! an explicit, immediate follow-up (`BACKLOG.md`), not
//! silently dropped. Kind-based classification (`isRetryable`, pattern
//! matching, `toString`) is fully real and load-bearing today.
//!
//! Backed by `builtin_runtime_enum` (mirrors `Order`/`SkyOrder`, #123):
//! `Error`'s sole constructor shares its name with the type
//! (`sky_lower`'s `enum_variants` table), so it emits as the tuple variant
//! `SkyError::Error(kind, info)` via the SAME generic constructor/pattern
//! path `SkyMaybe::Just`/`SkyResult::Ok` already use — no new emitter
//! mechanism needed, just two more `builtin_runtime_enum` table rows.

use std::fmt;

/// Sky's `ErrorKind` — 11 nullary variants. Repr(u8) for a compact, sound,
/// exhaustively-matched runtime type (mirrors `SkyOrder`'s convention).
/// Variant order matches canon's registration (`crates/sky_canon/src/env.rs`,
/// "E-12, #152") — do not reorder without updating that table.
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

/// Sky's `ErrorInfo` — this pass carries only `message`; `details` is filed
/// as an immediate follow-up (see module doc).
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub struct SkyErrorInfo {
    pub message: String,
}

/// Sky's `Error` — `Error ErrorKind ErrorInfo`, a single tuple-variant enum
/// (constructor name == type name, matching `sky_lower`'s registration) so
/// the generic `builtin_runtime_enum` constructor/pattern path handles it
/// exactly like `SkyMaybe::Just`/`SkyResult::Ok`.
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum SkyError {
    Error(SkyErrorKind, SkyErrorInfo),
}

impl SkyError {
    fn with(kind: SkyErrorKind, message: String) -> Self {
        Self::Error(kind, SkyErrorInfo { message })
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
}

impl fmt::Display for SkyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_sky_string())
    }
}

// ── Sky.Core.Error kernels (backlog #85/#160) ────────────────────────────────
// Each message constructor now classifies its own `ErrorKind` at construction
// (previously all eight shared one string-identity runtime symbol, per the
// "minimal Error = String slice, #86" note this supersedes).

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

/// Compatibility bridge: existing kernel call sites across the runtime that
/// still produce a bare `String` error (the pre-#160 shape) keep compiling —
/// `?`/`.into()` on a `String` now yields an `Unexpected`-classified `Error`
/// instead of losing type information. Migrating each such call site to a
/// properly-classified constructor (`SkyError::io`, `::network`, …) is the
/// explicit follow-up this pass files (see module doc + backlog).
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
}
