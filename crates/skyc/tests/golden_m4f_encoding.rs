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
//! * `Encoding.hexEncode` / `Encoding.hexDecode` roundtrip — Go
//!   `hex.EncodeToString` emits lowercase hex:
//!   `"Hi!"` → `"486921"` → `"Hi!"` → `"486921 Hi!"`.
//!   (`m4f_encoding_hex`)
//!
//! * `Encoding.base64Decode` of an invalid input → `Err _` branch taken →
//!   `"invalid"` (`m4f_encoding_invalid`).
//!
//! All four goldens carry `oracle_divergence = false`; Go and Sky-Rust emit
//! identical output for ASCII inputs (the Latin-1 byte convention in the Rust
//! runtime matches Go's string-as-bytes for the ASCII subset tested here).
//!
//! Every test is gated on `SKY_E2E=1`; without it the test returns early. Run:
//!
//! ```text
//! SKY_E2E=1 SKY_RUNTIME_DIR=<path-to-runtime-rust/src/sky_runtime> \
//!     cargo test golden_m4f
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

/// `Encoding.urlEncode "a b&c"` → `"a+b%26c"` (Go QueryEscape: space → `+`);
/// `Encoding.urlDecode "a+b%26c"` → `Ok "a b&c"`.  Output: `"a+b%26c a b&c"`.
#[test]
fn encoding_url_roundtrip() {
    assert_runs_and_matches_oracle("m4f_encoding_url");
}

// ── hexEncode + hexDecode ────────────────────────────────────────────────────

/// `Encoding.hexEncode "Hi!"` → `"486921"` (lowercase hex, Go parity);
/// `Encoding.hexDecode "486921"` → `Ok "Hi!"`.  Output: `"486921 Hi!"`.
#[test]
fn encoding_hex_roundtrip() {
    assert_runs_and_matches_oracle("m4f_encoding_hex");
}

// ── base64Decode of invalid input ────────────────────────────────────────────

/// `Encoding.base64Decode "not-valid-base64!!"` → `Err _`.  Output: `"invalid"`.
#[test]
fn encoding_base64_invalid() {
    assert_runs_and_matches_oracle("m4f_encoding_invalid");
}
