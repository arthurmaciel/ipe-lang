//! M5b `Sky.Core.Http` gate —
//! `Http.parseQuery` (pure), `defaultRequest` + `with*` builder chain (pure),
//! and `HttpResponse` record literal / field access (pure struct path).
//!
//! Every test compiles a Sky program through `skyc`, builds the emitted Rust
//! project with the shared cargo target, runs the binary, and checks its stdout
//! against the cached oracle (`tests/golden/m5b_http_*/oracle.meta` +
//! `expected_go.txt`). All tests are gated on `SKY_E2E=1`; without it they
//! return early.
//!
//! ## Oracle provenance
//!
//! All three goldens are TRUE Go-parity goldens (`oracle_divergence = false`):
//! the Go reference compiler compiled the identical `Main.sky` and produced the
//! captured `expected_go.txt`.  The test asserts byte-identity of stdout with
//! the Go output.
//!
//! ## Golden catalogue
//!
//! * `m5b_http_parse_query` — `Http.parseQuery "a=1&b=two%20words&a=ignored&c"`
//!   first-key-wins, percent-decodes, empty-value for key-only, sorted via
//!   `Dict.toList`. Output: three `key=value` lines.
//!
//! * `m5b_http_builders` — `defaultRequest |> withMethod "POST" |> withTimeout
//!   5000 |> withHeader "A" "1" |> withHeader "B" "2" |> withBody "hello"`.
//!   Prints `.method`, `.url`, `.timeout`, `.body`, and `.headers` (comma-joined).
//!   Confirms pure builder chain, record-update syntax, and that `withHeader`
//!   prepends (latest-added first) matching the Go reference.
//!
//! * `m5b_http_response_fields` — constructs an `HttpResponse` literal
//!   `{ status = 200, body = "ok", headers = Dict.fromList […] }`, reads back
//!   `.status`, `.body`, and `Dict.get "X-Test"`. Proves the
//!   `{body, headers, status}` fieldset synthesises correctly.
//!
//! Run:
//!
//! ```text
//! SKY_E2E=1 cargo test golden_m5b_http
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

/// Compile `tests/golden/<name>/Main.sky` through skyc, build the emitted Rust
/// project with the shared cargo target, run the binary, and return the golden
/// directory plus the run outcome. Gated on `SKY_E2E=1`.
fn build_run(name: &str) -> (PathBuf, support::RunOutcome) {
    let root = repo_root();
    let dir = golden_dir(&root, name);
    let entry = dir.join("Main.sky");
    let out = std::env::temp_dir().join(format!("skyc_{name}_e2e"));
    let _ = std::fs::remove_dir_all(&out);

    let runtime = skyc::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve for E2E");
    let Ok(runtime) = runtime else {
        return (
            dir,
            support::RunOutcome {
                stdout: String::new(),
                exit_code: None,
            },
        );
    };
    let built = skyc::build(&entry, &out, &runtime);
    assert!(built.is_ok(), "build failed for {name}: {:?}", built.err());

    let outcome = support::build_and_run_emitted(name, &out);
    (dir, outcome)
}

/// Compile/build/run the golden and assert its stdout matches the cached Go
/// oracle. Gated on `SKY_E2E=1`.
fn assert_runs_and_matches_oracle(name: &str) {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }
    let (dir, outcome) = build_run(name);
    support::assert_go_parity(name, &dir, &outcome.stdout);
    assert_eq!(outcome.exit_code, Some(0), "{name}: must exit 0");
}

// ── Http.parseQuery ───────────────────────────────────────────────────────────

/// `Http.parseQuery "a=1&b=two%20words&a=ignored&c"` — first-key-wins,
/// percent-decodes spaces, empty-value for bare key.  Output: 3 `key=value`
/// lines (sorted by `Dict.toList`).  Go-parity oracle.
#[test]
fn http_parse_query() {
    assert_runs_and_matches_oracle("m5b_http_parse_query");
}

// ── Http builder chain ────────────────────────────────────────────────────────

/// `defaultRequest |> withMethod |> withTimeout |> withHeader "A" |> withHeader
/// "B" |> withBody` chain prints 5 lines.  `withHeader` prepends (B before A).
/// Go-parity oracle.
#[test]
fn http_builders() {
    assert_runs_and_matches_oracle("m5b_http_builders");
}

// ── HttpResponse record literal + field access ────────────────────────────────

/// Construct `{ status = 200, body = "ok", headers = Dict.fromList […] }`
/// and read back fields.  Proves the `{body, headers, status}` fieldset
/// synthesises and that `Dict.get` inside `.headers` resolves.  Go-parity
/// oracle.
#[test]
fn http_response_fields() {
    assert_runs_and_matches_oracle("m5b_http_response_fields");
}
