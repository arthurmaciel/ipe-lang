//! `Ipe.Secret` positive goldens — build + run + assert on
//! stdout directly (no cached-oracle comparison: `Secret` has no Go/Haskell
//! counterpart, same `oracle_divergence` posture as `SqlFragment`'s
//! goldens, but simple enough that a direct stdout assertion is clearer than
//! standing up oracle-cache scaffolding for a feature with no oracle to
//! diverge FROM).
//!
//! Companion files: `crates/ipe/tests/secret_gates.rs` (negative gates —
//! `mySecret ++ "x"` rejected at compile time) and
//! `crates/ipe/tests/model_admissibility.rs`
//! (`live_model_with_secret_field_is_rejected`, the Model-gate IPE-L0120
//! case).
//!
//! Run: `IPE_E2E=1 cargo test golden_secret`

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn golden_dir(root: &Path, name: &str) -> PathBuf {
    root.join("tests").join("golden").join(name)
}

fn e2e_enabled() -> bool {
    std::env::var("IPE_E2E").is_ok()
}

/// Compile `tests/golden/<name>/Main.ipe`, build the emitted Cargo project,
/// run it, and return the captured stdout. Fails the test on any build or
/// runtime error — a broken golden cannot pass silently.
fn compile_build_run(name: &str) -> crate::support::RunOutcome {
    let root = repo_root();
    let entry = golden_dir(&root, name).join("Main.ipe");
    let out = std::env::temp_dir().join(format!("ipec_{name}_e2e"));
    let _ = std::fs::remove_dir_all(&out);

    let runtime = ipe::resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve for E2E");
    let Ok(runtime) = runtime else {
        return crate::support::RunOutcome {
            stdout: String::new(),
            exit_code: None,
        };
    };
    let built = ipe::build(&entry, &out, &runtime);
    assert!(built.is_ok(), "build failed for {name}: {:?}", built.err());

    crate::support::build_and_run_emitted(name, &out)
}

// ── (a) normal safe usage works end-to-end ──────────────────────────────────

/// `Secret.fromString` seals a plaintext `String`; `Secret.reveal` is the
/// single greppable un-parse; `Secret.redacted` is the explicit
/// `"<redacted>"` accessor. Plaintext-grep-guard: the marker string appears
/// in stdout EXACTLY ONCE (the deliberate `reveal` line) — `redacted` never
/// echoes it.
#[test]
fn seal_reveal_round_trips_and_redacted_never_leaks() {
    if !e2e_enabled() {
        return;
    }
    let out = compile_build_run("m_secret_seal_reveal");
    assert_eq!(out.exit_code, Some(0), "got {:?}", out.exit_code);
    let marker = "sk_live_zx9K3_MARKER";
    assert_eq!(
        out.stdout.matches(marker).count(),
        1,
        "the secret marker must appear in stdout EXACTLY ONCE (the deliberate \
         `Secret.reveal` line) — any other count means either `Secret.redacted` \
         leaked it or `reveal` didn't run. Full stdout: {:?}",
        out.stdout
    );
    assert!(
        out.stdout.contains("<redacted>"),
        "Secret.redacted must print the fixed placeholder. Full stdout: {:?}",
        out.stdout
    );
}

// ── (b) every plausible accidental-stringification path is redacted ────────

/// `==` is `Secret`'s ONLY equality (hand-written, constant-time — safe by
/// construction). Exercises match / content-mismatch / length-mismatch.
#[test]
fn equality_is_constant_time_and_structural() {
    if !e2e_enabled() {
        return;
    }
    let out = compile_build_run("m_secret_eq");
    assert_eq!(out.exit_code, Some(0), "got {:?}", out.exit_code);
    assert_eq!(
        out.stdout.trim(),
        "TFF",
        "a==b (match) -> T, a==c (content mismatch) -> F, a==d (length \
         mismatch) -> F"
    );
}

/// A record containing a `Secret` field stays fully derivable
/// (`Clone`/`Debug`/`PartialEq`) — the derive-blast-radius fix. Record
/// `==` recurses into `Secret`'s own equality; `{ r | f = v }` requires
/// `Clone` on every field including the `Secret` one.
#[test]
fn record_containing_secret_stays_clone_debug_eq() {
    if !e2e_enabled() {
        return;
    }
    let out = compile_build_run("m_secret_record");
    assert_eq!(out.exit_code, Some(0), "got {:?}", out.exit_code);
    assert_eq!(
        out.stdout.trim(),
        "TFstaging",
        "cfg1==cfg2 (same fields) -> T, cfg1==cfg3 (label differs) -> F, \
         cfg3.label after the record update -> staging"
    );
}

/// Logging a `Secret` directly (`Log.infoWith "boot" [ aSecret ]`) is safe BY
/// CONSTRUCTION: the attr-list element's Stringify obligation routes through
/// `Secret`'s hand-written `IpeStringify`, which ALWAYS redacts. The marker
/// must NEVER appear anywhere in stdout.
#[test]
fn logging_a_secret_directly_never_leaks() {
    if !e2e_enabled() {
        return;
    }
    let out = compile_build_run("m_secret_log_redact");
    assert_eq!(out.exit_code, Some(0), "got {:?}", out.exit_code);
    let marker = "sk_live_LOGGED_SECRET_MARKER";
    assert!(
        !out.stdout.contains(marker),
        "logging a Secret directly must NEVER leak the plaintext. Full \
         stdout: {:?}",
        out.stdout
    );
    assert!(
        out.stdout.contains("<redacted>"),
        "the log line must contain the redacted placeholder. Full stdout: {:?}",
        out.stdout
    );
}

// ── (c) the one non-Ipê-code serialization-adjacent path: Auth's typed boundary ──

/// `Auth.signToken` / `Auth.verifyToken` are re-typed to take `Secret` (not
/// `String`) at the signing-key position. Full sign -> verify round trip
/// through the `AUTH_WRAPPERS` boundary that reveals the `Secret`
/// immediately before delegating to the runtime.
#[test]
fn auth_sign_verify_round_trip_with_secret_key() {
    if !e2e_enabled() {
        return;
    }
    let out = compile_build_run("m_secret_auth_roundtrip");
    assert_eq!(out.exit_code, Some(0), "got {:?}", out.exit_code);
    assert_eq!(out.stdout.trim(), "alice");
}

// ── Pure compile-only smoke (always runs, no IPE_E2E / no cargo) ───────────

/// All five fixtures above must at least `ipe`-compile cleanly even when
/// `IPE_E2E` is unset (CI without the heavy cargo-build tier still catches a
/// type-check regression).
#[test]
fn all_secret_goldens_compile() {
    let root = repo_root();
    for name in [
        "m_secret_seal_reveal",
        "m_secret_eq",
        "m_secret_record",
        "m_secret_log_redact",
        "m_secret_auth_roundtrip",
    ] {
        let entry = golden_dir(&root, name).join("Main.ipe");
        let out = std::env::temp_dir().join(format!("ipec_{name}_compileonly"));
        let _ = std::fs::remove_dir_all(&out);
        let Ok(runtime) = ipe::resolve_runtime() else {
            return;
        };
        let built = ipe::build(&entry, &out, &runtime);
        assert!(built.is_ok(), "{name} must ipec-compile: {:?}", built.err());
    }
}
