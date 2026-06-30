//! M4f `Sky.Core.Encoding` parity gate — base64 / URL / hex encode-decode
//! kernels with byte-for-byte Go parity.
//!
//! These golden tests exercise the `Sky.Core.Encoding` kernel family end-to-end:
//!
//! * `Encoding.base64Encode` / `Encoding.base64Decode` roundtrip using Go
//!   `base64.StdEncoding` (standard alphabet with `=` padding, NOT URL-safe):
//!   `"hello"` → `"aGVsbG8="` → `"hello"` → `"aGVsbG8= hello"`.
//!   (`m4f_encoding_base64`)
//!
//! * `Encoding.urlEncode` / `Encoding.urlDecode` roundtrip using Go
//!   `url.QueryEscape` semantics (space → `+`, `&` → `%26`):
//!   `"a b&c"` → `"a+b%26c"` → `"a b&c"` → `"a+b%26c a b&c"`.
//!   (`m4f_encoding_url`)
//!
//! * `Encoding.urlEncode` over the ASCII unreserved set: Go `url.QueryEscape`
//!   leaves `A-Za-z0-9` plus the four marks `-` `_` `.` `~` verbatim while
//!   escaping `+` → `%2B` and `@` → `%40`. Pins parity over both shapes.
//!   (`m4f_encoding_url_unreserved`)
//!
//! * `Encoding.hexEncode` / `Encoding.hexDecode` roundtrip — Go
//!   `hex.EncodeToString` emits lowercase hex:
//!   `"Hi!"` → `"486921"` → `"Hi!"` → `"486921 Hi!"`.
//!   (`m4f_encoding_hex`)
//!
//! * `Encoding.base64Decode` of an invalid input → `Err _` branch taken →
//!   `"invalid"` (`m4f_encoding_invalid`).
//!
//! * `Encoding.base64Encode` / `hexEncode` over non-ASCII text → recorded
//!   `divergence:` (`m4f_encoding_nonascii_divergence`).
//!
//! ## Oracle-divergence flags (per golden)
//!
//! The pure-ASCII ENCODE roundtrips (`m4f_encoding_base64`, `m4f_encoding_url`,
//! `m4f_encoding_url_unreserved`, `m4f_encoding_hex`) carry `oracle_divergence
//! = false`: for ASCII the Rust runtime's Latin-1 byte convention coincides with
//! Go's string-as-bytes, so output is byte-identical to the Go reference.
//!
//! `m4f_encoding_nonascii_divergence` carries `oracle_divergence = true`
//! (`divergence:` kind): for codepoints ≥ 0x80 the Latin-1 char-as-byte model
//! differs from Go's UTF-8 string bytes. See `docs/architecture/divergence-policy.md`.
//!
//! `m4f_encoding_invalid` carries `oracle_divergence = true` (Go-failure kind):
//! the Go backend PANICS with `CoerceFailure` (`rt.ResultCoerce` →
//! `coerceInner`) when narrowing the `Result` of a decode kernel inside a
//! top-level `case … of Ok _ … Err _ …`, so it cannot produce a reference for
//! this shape. skyc handles it correctly (`"invalid"`), which `refresh-oracle`
//! caches. This is an UPSTREAM (Go-backend) bug, not a Sky-Rust one. TRACKED:
//! re-verify this golden against the live Go oracle once the upstream
//! CoerceFailure-on-decode-Result narrowing is fixed (`refresh-oracle
//! m4f_encoding_invalid`) — if Go then succeeds, the flag should flip back to
//! `false`. The base64/url/hex DECODE oracles decode from a let-bound `case`
//! whose `Err _` arm yields a String literal (a different routing shape that Go
//! handles), so they stay parity-clean — re-verified against the live Go oracle.
//!
//! Every test is gated on `SKY_E2E=1`; without it the test returns early. Run:
//!
//! ```text
//! SKY_E2E=1 cargo test golden_m4f
//! ```

use std::path::{Path, PathBuf};

mod support;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn golden_dir(root: &Path, name: &str) -> PathBuf {
    root.join("tests").join("golden").join(name)
}

/// Compile `tests/golden/<name>/Main.sky`, build the emitted Cargo project,
/// run it, and assert its stdout matches the cached oracle.  Gated on
/// `SKY_E2E=1`.
fn assert_runs_and_matches_oracle(name: &str) {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }

    let root = repo_root();
    let dir = golden_dir(&root, name);
    let entry = dir.join("Main.sky");
    let out = std::env::temp_dir().join(format!("skyc_{name}_e2e"));
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve for E2E");
    let Ok(runtime) = runtime else { return };
    let built = skyc::build(&entry, &out, &runtime);
    assert!(built.is_ok(), "build failed for {name}: {:?}", built.err());

    let outcome = support::build_and_run_emitted(name, &out);
    support::assert_go_parity(name, &dir, &outcome.stdout);
    assert_eq!(outcome.exit_code, Some(0), "exit 0, matching the oracle");
}

// ── base64Encode + base64Decode ──────────────────────────────────────────────

/// `Encoding.base64Encode "hello"` → `"aGVsbG8="`; `Encoding.base64Decode
/// "aGVsbG8="` → `Ok "hello"`.  Output: `"aGVsbG8= hello"`.
#[test]
fn encoding_base64_roundtrip() {
    assert_runs_and_matches_oracle("m4f_encoding_base64");
}

// ── urlEncode + urlDecode ────────────────────────────────────────────────────

/// `Encoding.urlEncode "a b&c"` → `"a+b%26c"` (Go `QueryEscape`: space → `+`);
/// `Encoding.urlDecode "a+b%26c"` → `Ok "a b&c"`.  Output: `"a+b%26c a b&c"`.
#[test]
fn encoding_url_roundtrip() {
    assert_runs_and_matches_oracle("m4f_encoding_url");
}

/// `Encoding.urlEncode "a-b_c.d~e"` → `"a-b_c.d~e"` (unreserved set verbatim);
/// `Encoding.urlEncode "user+name@example.com"` → `"user%2Bname%40example.com"`
/// (`+` → `%2B`, `@` → `%40`).  Locks the Go `QueryEscape` unreserved set.
#[test]
fn encoding_url_unreserved() {
    assert_runs_and_matches_oracle("m4f_encoding_url_unreserved");
}

// ── hexEncode + hexDecode ────────────────────────────────────────────────────

/// `Encoding.hexEncode "Hi!"` → `"486921"` (lowercase hex, Go parity);
/// `Encoding.hexDecode "486921"` → `Ok "Hi!"`.  Output: `"486921 Hi!"`.
#[test]
fn encoding_hex_roundtrip() {
    assert_runs_and_matches_oracle("m4f_encoding_hex");
}

// ── non-ASCII byte-model divergence ──────────────────────────────────────────

/// `Encoding.base64Encode "café"` / `hexEncode "café"` over non-ASCII text.
/// Recorded `divergence:` — the Rust runtime's Latin-1 char-as-byte model
/// differs from Go's UTF-8 string bytes for codepoints ≥ 0x80.  Expected holds
/// the Sky-Rust output (`café Y2Fm6Q== 636166e9`); see divergence-policy.md.
#[test]
fn encoding_nonascii_divergence() {
    assert_runs_and_matches_oracle("m4f_encoding_nonascii_divergence");
}

// ── base64Decode of invalid input ────────────────────────────────────────────

/// `Encoding.base64Decode "not-valid-base64!!"` → `Err _`.  Output: `"invalid"`.
#[test]
fn encoding_base64_invalid() {
    assert_runs_and_matches_oracle("m4f_encoding_invalid");
}
