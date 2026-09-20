//! Single source of truth for the runtime-shape field-NAME sets.
//!
//! Several Ipê record shapes fold to a nominal runtime struct rather than a
//! backend-synthesised `Rec…` struct: `HttpRequest`, `ServerResponse`,
//! `CacheCfg`, `CsvDoc`, `WsClientCfg`, the four `Ipe.Email` records, and the
//! two `Ipe.Process` config records. Two crates decide this shape:
//!
//! - `ipe_lower` folds a record whose NAMES *and* TYPES match one of these
//!   shapes into the opaque `IrType` variant, so the shape never reaches the
//!   struct registry.
//! - `ipe_backend_rust` reconstructs the runtime struct name from a NAME-only
//!   fallback (it has no access to the lowerer's `Ty`), consulted only when no
//!   synthesised struct is registered for the literal's field-name set.
//!
//! The shared fact is the sorted field-NAME set. It lives here so both crates
//! read ONE definition: a rename here reaches every consumer at once, and the
//! lower-side name+type-pair tables bind their NAME column to these consts with
//! a build-time [`names_match`] assertion, so a drift is a `cargo build`
//! failure rather than an `ipe`-exit-0-then-`cargo`-fail SEAL breach.
//!
//! Every set is sorted alphabetically by name (Rust names struct-literal
//! fields, so write order is free; the fallback compares against a sorted key).

/// `HttpRequest` runtime struct field names (`ipe_runtime::http_client`).
pub const HTTP_REQUEST_FIELDS: &[&str] =
    &["body", "headers", "method", "redirects", "timeout", "url"];

/// `Ipe.Process.runWith` config field names (`ipe_runtime::system::ProcessRunWithCfg`).
pub const PROCESS_RUN_WITH_CFG_FIELDS: &[&str] = &["args", "command", "cwd", "env"];

/// `Ipe.Process.runInPty` config field names (`ipe_runtime::system::ProcessRunInPtyCfg`).
pub const PROCESS_RUN_IN_PTY_CFG_FIELDS: &[&str] =
    &["args", "cols", "command", "cwd", "env", "rows"];

/// `Ipe.Cache.CacheCfg` field names (`ipe_runtime::cache::CacheCfg`).
pub const CACHE_CFG_FIELDS: &[&str] = &["maxBytes", "maxEntries", "ttlMs"];

/// `Ipe.Csv.Csv` field names (`ipe_runtime::csv::CsvDoc`).
pub const CSV_DOC_FIELDS: &[&str] = &["header", "rows"];

/// `Ipe.WebSocket.WebSocketCfg` field names (`ipe_runtime::ws_client::WsClientCfg`).
pub const WEBSOCKET_CFG_FIELDS: &[&str] = &["headers", "pingInterval", "timeout", "url"];

/// `Ipe.Http.Server.Response` field names (`ipe_runtime::server::ServerResponse`).
/// The runtime struct carries one EXTRA runtime-only field, `cookies:
/// Vec<String>` (multi-`Set-Cookie` support), which the Ipê record alias does
/// not expose — the backend defaults it to `Vec::new()`.
pub const SERVER_RESPONSE_FIELDS: &[&str] = &["body", "contentType", "headers", "status"];

/// `Ipe.Email` `EmailMessage` field names (`ipe_runtime::email::EmailMessage`).
pub const EMAIL_MESSAGE_FIELDS: &[&str] = &[
    "attachments",
    "bcc",
    "cc",
    "from",
    "htmlBody",
    "replyTo",
    "subject",
    "textBody",
    "to",
];

/// `Ipe.Email` `Attachment` field names (`ipe_runtime::email::EmailAttachment`).
pub const EMAIL_ATTACHMENT_FIELDS: &[&str] = &["content", "filename", "mimeType"];

/// `Ipe.Email` `SesConfig` field names (`ipe_runtime::email::SesConfig`).
pub const EMAIL_SES_FIELDS: &[&str] = &["key", "region", "secret"];

/// `Ipe.Email` `SmtpConfig` field names (`ipe_runtime::email::SmtpConfig`).
pub const EMAIL_SMTP_FIELDS: &[&str] = &["host", "pass", "port", "user"];

/// Are two `&str` slices element-for-element equal? A `const fn` byte compare
/// so a consumer that keeps its own copy of a name column can bind it to the
/// shared const with `const _: () = assert!(names_match(TABLE, X));` — a name
/// drift then fails `cargo build` (const-eval) rather than a downstream cargo
/// error. Element access is by slice-pattern destructuring (`[head, tail @
/// ..]`), never `[_]` indexing, so no index can panic and the workspace
/// `indexing_slicing` restriction holds; `slice::get` is not yet a stable
/// `const fn`, so the walk recurses on the tail rather than indexing.
#[must_use]
pub const fn names_match(a: &[&str], b: &[&str]) -> bool {
    match (a, b) {
        ([], []) => true,
        ([sa, a_rest @ ..], [sb, b_rest @ ..]) => str_eq(sa, sb) && names_match(a_rest, b_rest),
        _ => false,
    }
}

/// Byte-for-byte `&str` equality in a `const` context, by slice-pattern
/// recursion (no `[_]` indexing, and `slice::get` is not yet const-stable).
const fn str_eq(a: &str, b: &str) -> bool {
    match (a.as_bytes(), b.as_bytes()) {
        ([], []) => true,
        ([ba, a_rest @ ..], [bb, b_rest @ ..]) => *ba == *bb && bytes_eq(a_rest, b_rest),
        _ => false,
    }
}

/// Tail of [`str_eq`]: byte-slice equality by slice-pattern recursion.
const fn bytes_eq(a: &[u8], b: &[u8]) -> bool {
    match (a, b) {
        ([], []) => true,
        ([ba, a_rest @ ..], [bb, b_rest @ ..]) => *ba == *bb && bytes_eq(a_rest, b_rest),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_match_is_reflexive() {
        assert!(names_match(CSV_DOC_FIELDS, CSV_DOC_FIELDS));
        assert!(names_match(HTTP_REQUEST_FIELDS, HTTP_REQUEST_FIELDS));
    }

    #[test]
    fn names_match_rejects_length_mismatch() {
        assert!(!names_match(CSV_DOC_FIELDS, HTTP_REQUEST_FIELDS));
    }

    #[test]
    fn names_match_rejects_element_mismatch() {
        assert!(!names_match(&["header", "rows"], &["header", "cols"]));
    }
}
