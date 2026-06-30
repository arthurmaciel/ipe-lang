//! M4g `Sky.Core.Json.Encode` parity gate — JSON encoder kernels with
//! byte-for-byte Go parity.
//!
//! These golden tests exercise the `JsonEnc` kernel family end-to-end:
//!
//! * `JsonEnc.object` + `JsonEnc.string` + `JsonEnc.int` + `JsonEnc.list` +
//!   `JsonEnc.encode 0` — a nested `{name, age, tags:[…]}` object at compact
//!   (indent=0) output.  Go: `json.Marshal(map[string]any{})` sorts keys
//!   alphabetically → `"age" < "name" < "tags"`.
//!   (`m4g_json_enc_object_compact`)
//!
//! * Same object at `encode 2` — 2-space pretty-print matching Go's
//!   `json.MarshalIndent(val, "", "  ")`.
//!   (`m4g_json_enc_object_pretty`)
//!
//! * `JsonEnc.float 1.5` + `JsonEnc.bool True` + `JsonEnc.null` — scalar
//!   primitives.  Output: `"1.5 true null"`.
//!   (`m4g_json_enc_float`)
//!
//! * `JsonEnc.string "say \"hi\""` — a string whose value contains `"` chars
//!   that JSON must escape as `\"`.  Pins byte-for-byte agreement between
//!   `serde_json` and Go's `encoding/json`.
//!   (`m4g_json_enc_escape`)
//!
//! Every test is gated on `SKY_E2E=1`; without it the test returns early.  Run:
//!
//! ```text
//! SKY_E2E=1 SKY_RUNTIME_DIR=<path-to-runtime-rust/src/sky_runtime> \
//!     cargo test golden_m4g
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

// ── compact object (indent 0) ────────────────────────────────────────────────

/// `JsonEnc.object [{name,age,tags}]` encoded at `indent=0`.
/// Keys sorted: `"age" < "name" < "tags"`.  Output: compact JSON line.
#[test]
fn json_enc_object_compact() {
    assert_runs_and_matches_oracle("m4g_json_enc_object_compact");
}

// ── pretty object (indent 2) ─────────────────────────────────────────────────

/// Same object encoded at `indent=2`.  Mirrors Go's `json.MarshalIndent`
/// 2-space indentation.
#[test]
fn json_enc_object_pretty() {
    assert_runs_and_matches_oracle("m4g_json_enc_object_pretty");
}

// ── float + bool + null scalars ──────────────────────────────────────────────

/// `JsonEnc.float 1.5` → `"1.5"`, `JsonEnc.bool True` → `"true"`,
/// `JsonEnc.null` → `"null"`.  Output: `"1.5 true null"`.
#[test]
fn json_enc_float_bool_null() {
    assert_runs_and_matches_oracle("m4g_json_enc_float");
}

// ── string escaping ──────────────────────────────────────────────────────────

/// `JsonEnc.string "say \"hi\""` → `"say \"hi\""`.  Pins `"` → `\"` JSON
/// escaping parity between `serde_json` and Go's `encoding/json`.
#[test]
fn json_enc_string_escape() {
    assert_runs_and_matches_oracle("m4g_json_enc_escape");
}
