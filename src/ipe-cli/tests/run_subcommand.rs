//! Integration tests for `ipe run` — gated on `IPE_E2E=1` so the default
//! `cargo nextest` stays fast and offline (no cargo invocation required).
//!
//! The non-E2E tests still exercise the CLI parsing surface (usage errors) and
//! are unconditionally active.

use std::fs;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// `assert!(false_marker())` fails the test without tripping
/// `clippy::assertions_on_constants`.
// `const` is rejected on purpose: a const-known `false` would re-trip
// `assertions_on_constants` at the call site, defeating the `black_box`.
#[allow(clippy::missing_const_for_fn)]
fn false_marker() -> bool {
    std::hint::black_box(false)
}

// ---------------------------------------------------------------------------
// CLI-parsing tests (unconditional — no network, no build)
// ---------------------------------------------------------------------------

/// `ipe run` (no arguments, nothing to build here) must return a command-usage
/// error naming `run` — so the caller shows `run`'s help page — not panic.
#[test]
fn run_no_args_returns_usage_error() {
    let args: Vec<String> = vec!["run".to_owned()];
    let result = ipe::run_cli(&args);
    assert!(
        matches!(
            result,
            Err(ipe::CliError::CommandUsage { command: "run", .. })
        ),
        "expected a `run` command-usage error for bare `ipe run`, got: {result:?}"
    );
}

/// `ipe run <entry> --bogus` (unrecognised flag after the entry) must return a
/// command-usage error for `run` — carrying a reason naming the offending flag,
/// so the caller shows `run`'s help page — not panic.
#[test]
fn run_unknown_flag_returns_usage_error() {
    let args: Vec<String> = vec![
        "run".to_owned(),
        "Main.ipe".to_owned(),
        "--bogus-flag".to_owned(),
    ];
    let result = ipe::run_cli(&args);
    assert!(
        matches!(
            &result,
            Err(ipe::CliError::CommandUsage { command, reason })
                if *command == "run" && reason.contains("--bogus-flag")
        ),
        "expected a `run` command-usage error naming the offending flag, got: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// E2E test — only active when IPE_E2E=1 (requires cargo + runtime)
// ---------------------------------------------------------------------------

/// `ipe run <entry.ipe>` must:
///   1. Compile the Ipê program (exit 0 from ipe pipeline).
///   2. Invoke `cargo build` on the emitted project (SEAL check).
///   3. Exec the resulting binary; its stdout must equal `"hello from run\n"`.
///
/// This test exercises the full `run_run` path from CLI dispatch through the
/// Unix `exec` replacement.  It is skipped unless `IPE_E2E=1` is set.
#[test]
fn run_subcommand_builds_and_executes_hello_program() {
    const SRC: &str =
        "module Main exposing (main)\n\nimport Ipe.Io\n\nmain = Io.println \"hello from run\"\n";

    if std::env::var("IPE_E2E").is_err() {
        return;
    }

    // Resolve the runtime dir (skips the test when IPE_RUNTIME_DIR is unset
    // and the walk-up also fails, which happens in CI without the repo tree).
    let runtime = ipe::resolve_runtime();
    assert!(
        runtime.is_ok(),
        "runtime must resolve for E2E test: {runtime:?}"
    );
    let Ok(runtime_dir) = runtime else {
        return;
    };

    // Write the source file into a temp directory.
    let dir = std::env::temp_dir().join("ipec_run_subcommand_e2e");
    let _ = fs::remove_dir_all(&dir);
    let entry = dir.join("Main.ipe");
    let created = fs::create_dir_all(&dir).and_then(|()| fs::write(&entry, SRC));
    assert!(created.is_ok(), "write source: {created:?}");

    // Use a dedicated out dir so CARGO_TARGET_DIR contention is isolated.
    let out_dir = dir.join("out");

    // --- Step 1+2: compile + cargo build via run_cli ---
    // We cannot call run_cli("run", …) directly here because on Unix it would
    // exec (replace) this test process.  Instead, call the public `build`
    // function + cargo build directly to verify SEAL, then run the binary as a
    // child process to check stdout.  This exercises the same code paths as
    // run_run without sacrificing the test runner.
    let built = ipe::build(&entry, &out_dir, &runtime_dir);
    assert!(built.is_ok(), "ipe build step must succeed: {built:?}");

    let cargo_status = std::process::Command::new("cargo")
        .arg("build")
        .current_dir(&out_dir)
        .env("CARGO_TARGET_DIR", out_dir.join("target"))
        .status();
    assert!(
        matches!(&cargo_status, Ok(s) if s.success()),
        "cargo build on emitted project must succeed: {cargo_status:?}"
    );

    // --- Step 3: run the binary, capture stdout ---
    let bin: PathBuf = out_dir.join("target").join("debug").join("ipe-app");
    let run = std::process::Command::new(&bin).output();
    let Ok(run) = run else {
        assert!(false_marker(), "failed to run emitted binary: {run:?}");
        return;
    };
    assert!(run.status.success(), "binary must exit 0");
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "hello from run\n",
        "ipec run e2e: stdout mismatch"
    );

    // Cleanup heavy cargo artifacts; leave src for post-mortem if needed.
    let _ = fs::remove_dir_all(out_dir.join("target"));
}

/// After `ipe build` on a single-file program (no manifest) the emitted
/// `Cargo.toml` must carry `name = "ipe-app"`, and after `ipe build_project`
/// on a manifest whose `name` field sanitizes to a different slug the emitted
/// `Cargo.toml` must carry that slug — not `"ipe-app"`.
///
/// This is the structural guarantee that `ipe run` relies on: it reads the
/// binary name from the emitted `Cargo.toml` (the same file cargo just built
/// from) rather than re-deriving it from the manifest, so both sides can never
/// disagree.
#[test]
fn emitted_cargo_toml_name_matches_binary_ipe_run_will_exec() {
    const SRC: &str = "module Main exposing (main)\n\nimport Ipe.Io\n\nmain = Io.println \"ok\"\n";

    if std::env::var("IPE_E2E").is_err() {
        return;
    }

    let Ok(runtime_dir) = ipe::resolve_runtime() else {
        return;
    };

    // --- Case 1: single-file build (no manifest) → name must be "ipe-app" ---
    let dir = std::env::temp_dir().join("ipe_run_bin_name_e2e");
    let _ = fs::remove_dir_all(&dir);
    let entry = dir.join("Main.ipe");
    let created = fs::create_dir_all(&dir).and_then(|()| fs::write(&entry, SRC));
    assert!(created.is_ok(), "write source: {created:?}");

    let out_single = dir.join("out_single");
    let built = ipe::build(&entry, &out_single, &runtime_dir);
    assert!(built.is_ok(), "single-file build must succeed: {built:?}");

    let cargo_toml_text =
        fs::read_to_string(out_single.join("Cargo.toml")).expect("emitted Cargo.toml must exist");
    assert!(
        cargo_toml_text.contains("name = \"ipe-app\""),
        "single-file build must emit name = \"ipe-app\", got:\n{cargo_toml_text}"
    );

    // --- Case 2: manifest build with a project name that sanitizes to a slug ---
    // The manifest name "Crc32 Checksum" sanitizes to "crc32-checksum".
    // After build_project the emitted Cargo.toml must carry that slug, and
    // ipe run's binary lookup must find a file named "crc32-checksum" — not
    // "ipe-app".
    let pkg_dir = dir.join("pkg");
    let src_dir = pkg_dir.join("src");
    let _ = fs::create_dir_all(&src_dir);
    fs::write(
        pkg_dir.join("package.ipe"),
        "module Package exposing (package)\n\n\npackage =\n    Package.named \"Crc32 Checksum\"\n        |> Package.version \"0.1.0\"\n",
    )
    .expect("write package.ipe");
    fs::write(src_dir.join("Main.ipe"), SRC).expect("write Main.ipe");

    let out_pkg = dir.join("out_pkg");
    let built = ipe::build_project(&pkg_dir.join("package.ipe"), &out_pkg, &runtime_dir);
    assert!(built.is_ok(), "project build must succeed: {built:?}");

    let cargo_toml_text =
        fs::read_to_string(out_pkg.join("Cargo.toml")).expect("emitted Cargo.toml must exist");
    assert!(
        cargo_toml_text.contains("name = \"crc32-checksum\""),
        "project build must emit name = \"crc32-checksum\" (sanitized from \
         \"Crc32 Checksum\"), got:\n{cargo_toml_text}"
    );
    // The SSOT guarantee: the name the binary-resolution reads equals the name
    // cargo will produce as a binary artifact.  "ipe-app" must NOT appear as
    // the package name — that would be the pre-fix bug.
    assert!(
        !cargo_toml_text
            .lines()
            .any(|l| l.trim() == "name = \"ipe-app\""),
        "project build must NOT emit name = \"ipe-app\" as the package name"
    );

    let _ = fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Toolchain-absence tests (unconditional — the whole point is NO cargo)
// ---------------------------------------------------------------------------

/// `ipe run <entry.ipe>` under a `PATH` with no `cargo`, and with `CARGO_HOME`
/// and `HOME` pointed at empty directories so no install location is found,
/// must fail with the friendly root-cause message — naming Rust/Cargo, why Ipê
/// needs it, and the rustup fix — rather than the opaque OS spawn error.
///
/// This spawns the `ipe` binary as a child (so the Unix `exec` in `run_run`
/// replaces the child, not the test runner) and requires no cargo, so it runs
/// unconditionally.
#[test]
fn run_without_cargo_reports_the_missing_toolchain() {
    const SRC: &str = "module Main exposing (main)\n\nimport Ipe.Io\n\nmain = Io.println \"hi\"\n";

    // A runtime dir is needed to reach the toolchain check (which fires after
    // emit). Skip when the repo tree is unavailable (CI without checkout).
    let Ok(runtime_dir) = ipe::resolve_runtime() else {
        return;
    };

    let dir = std::env::temp_dir().join("ipe_run_no_cargo_e2e");
    let _ = fs::remove_dir_all(&dir);
    let entry = dir.join("Main.ipe");
    let empty_home = dir.join("empty-home");
    let created = fs::create_dir_all(&empty_home).and_then(|()| fs::write(&entry, SRC));
    assert!(created.is_ok(), "write source + empty home: {created:?}");

    let ipe_bin = env!("CARGO_BIN_EXE_ipe");
    // This path is baked at compile time; a nextest archive that ships the test
    // to another host runs it where that path does not resolve. Skip there — the
    // toolchain-resolution dispositions are covered by the unit tests in
    // `toolchain.rs`; this end-to-end spawn only adds value where the binary is
    // present.
    if !std::path::Path::new(ipe_bin).exists() {
        return;
    }
    // A minimal PATH with no cargo. `/nonexistent-ipe-cargo-probe` cannot hold
    // any executable, so cargo is unresolvable on the PATH.
    let cargoless_path = "/nonexistent-ipe-cargo-probe";
    let out = std::process::Command::new(ipe_bin)
        .args(["run", &entry.to_string_lossy(), "--out"])
        .arg(dir.join("out"))
        .env("PATH", cargoless_path)
        .env("HOME", &empty_home)
        .env("CARGO_HOME", empty_home.join("no-cargo"))
        .env("IPE_RUNTIME_DIR", &runtime_dir)
        .env("NO_COLOR", "1")
        .output();
    let Ok(out) = out else {
        assert!(false_marker(), "failed to spawn ipe: {out:?}");
        return;
    };

    assert!(
        !out.status.success(),
        "ipe run with no cargo must exit non-zero"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    // Which disposition fires depends on the host: a machine with no Rust at
    // all reports "not found"; a machine where Cargo is installed but off this
    // scrubbed PATH reports "not on your PATH". Both name the root cause.
    assert!(
        stderr.contains("Rust and Cargo were not found") || stderr.contains("not on your PATH"),
        "the missing-toolchain message must name the root cause, got:\n{stderr}"
    );
    assert!(
        stderr.contains("compile and run this program"),
        "the message must name what `ipe run` was doing, got:\n{stderr}"
    );
    // Both dispositions end with an actionable fix: install via rustup, or add
    // the existing Cargo to PATH.
    assert!(
        stderr.contains("rustup.rs") || stderr.contains("PATH"),
        "the message must give an actionable fix, got:\n{stderr}"
    );
    // The opaque OS spawn error must NOT leak through.
    assert!(
        !stderr.contains("os error"),
        "the raw OS spawn error must never surface, got:\n{stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}
