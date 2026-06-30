//! M5b `Sky.Core.Uuid` + `Sky.Core.Jwt` parity gate —
//! UUID generation/parsing and JWT HS256/RS256 encode/decode.
//!
//! Every test compiles a Sky program through `skyc`, builds the emitted Rust
//! project with the shared cargo target, runs the binary, and asserts its stdout
//! matches the cached oracle (`tests/golden/m5b_*/expected_go.txt`).
//! All tests are gated on `SKY_E2E=1`; without it they return early.
//!
//! ## Golden catalogue
//!
//! * `m5b_uuid_format` — `Uuid.v4` length is 36 and version nibble is '4';
//!   `Uuid.v7` length is 36 and version nibble is '7'; both round-trip through
//!   `Uuid.parse`.  Output: `"ok"`.
//!
//! * `m5b_uuid_parse` — `Uuid.parse "not-a-uuid"` → `Nothing`;
//!   `Uuid.parse "<valid-uuid>"` → `Just _`.  Output: `"ok"`.
//!
//! * `m5b_jwt_hs256_roundtrip` — `Jwt.encodeHs256` with a 32-byte key and
//!   fixed claims, then `Jwt.decodeHs256` with the same key succeeds.
//!   Output: `"ok"`.
//!
//! * `m5b_jwt_hs256_tamper` — appending `"x"` to an HS256 token causes
//!   `Jwt.decodeHs256` to reject the tampered input.
//!   Output: `"tamper-detected"`.
//!
//! * `m5b_jwt_rs256_roundtrip` — `Jwt.encodeRs256` with a PKCS#8 RSA-2048
//!   private key and `Jwt.decodeRs256` with the matching SPKI public key
//!   completes the round-trip without error.  Output: `"ok"`.
//!
//! Run:
//!
//! ```text
//! SKY_E2E=1 cargo test golden_m5b
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

// ── UUID format + version nibble ─────────────────────────────────────────────

/// `Uuid.v4` produces a 36-character string with version nibble '4' at index 14;
/// `Uuid.v7` produces a 36-character string with version nibble '7' at index 14;
/// both round-trip through `Uuid.parse`.  Output: `"ok"`.
#[test]
fn uuid_format() {
    assert_runs_and_matches_oracle("m5b_uuid_format");
}

// ── UUID parse ────────────────────────────────────────────────────────────────

/// `Uuid.parse "not-a-uuid"` → `Nothing`; `Uuid.parse "<valid-uuid>"` → `Just _`.
/// Output: `"ok"`.
#[test]
fn uuid_parse() {
    assert_runs_and_matches_oracle("m5b_uuid_parse");
}

// ── JWT HS256 round-trip ──────────────────────────────────────────────────────

/// `Jwt.encodeHs256 secret claims` followed by `Jwt.decodeHs256 secret token`
/// with the same 32-byte key succeeds.  Output: `"ok"`.
/// The HS256 token is deterministic for fixed key + claims, providing Go parity.
#[test]
fn jwt_hs256_roundtrip() {
    assert_runs_and_matches_oracle("m5b_jwt_hs256_roundtrip");
}

// ── JWT HS256 tamper detection ────────────────────────────────────────────────

/// Appending `"x"` to a valid HS256 token causes `Jwt.decodeHs256` to reject it.
/// Output: `"tamper-detected"`.
#[test]
fn jwt_hs256_tamper() {
    assert_runs_and_matches_oracle("m5b_jwt_hs256_tamper");
}

// ── JWT RS256 round-trip ──────────────────────────────────────────────────────

/// `Jwt.encodeRs256 privKeyPem claims` followed by `Jwt.decodeRs256 pubKeyPem token`
/// with a matching RSA-2048 PKCS#8/SPKI key pair succeeds.  Output: `"ok"`.
#[test]
fn jwt_rs256_roundtrip() {
    assert_runs_and_matches_oracle("m5b_jwt_rs256_roundtrip");
}
