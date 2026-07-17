//! `Sky.Http.Middleware.withCsrf` golden.
//!
//! Pins that a `Sky.Http.Server` route wrapped in `Middleware.withCsrf` emits
//! `middleware_with_csrf(...)` and that the emitted crate `cargo build`s (the
//! Seal: `skyc` exit 0 implies `cargo build` exit 0).
//!
//! Compile-only assertions always run; the cargo build is `SKY_E2E=1`-gated
//! with an ISOLATED `CARGO_TARGET_DIR` (a shared dir's fingerprint reuse can
//! mask a rustc failure as a false pass).

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn out_dir() -> PathBuf {
    std::env::temp_dir().join("skyc_m6_middleware_csrf")
}

/// Compile the fixture; `None` (skip) when the runtime cannot be resolved.
fn compile() -> Option<Result<(), skyc::CliError>> {
    let entry = repo_root()
        .join("tests")
        .join("golden")
        .join("middleware_csrf")
        .join("Main.sky");
    let out = out_dir();
    let _ = std::fs::remove_dir_all(&out);
    let Ok(runtime) = skyc::resolve_runtime() else {
        return None;
    };
    Some(skyc::build(&entry, &out, &runtime))
}

/// A `Server.post` route wrapped in `Middleware.withCsrf` must be skyc-0 and
/// emit `middleware_with_csrf(...)` wrapping the handler.
#[test]
fn middleware_with_csrf_emits_wrapped_handler() {
    let Some(result) = compile() else {
        return;
    };
    assert!(
        result.is_ok(),
        "#63: Middleware.withCsrf-wrapped route must be skyc-0, got: {:?}",
        result.err(),
    );
    let main_rs = std::fs::read_to_string(out_dir().join("src").join("main.rs"))
        .expect("emitted main.rs must exist");
    assert!(
        main_rs.contains("middleware_with_csrf("),
        "#63: emitted main.rs must call middleware_with_csrf(...)\n{main_rs}",
    );
}

/// `SKY_E2E` tier: the emitted project must cargo-build (isolated target dir)
/// — proves the seal (skyc exit 0 implies cargo build exit 0) for the new
/// `ServerResponse.cookies` field and the `middleware_with_csrf` kernel.
#[test]
fn middleware_with_csrf_cargo_builds() {
    if std::env::var("SKY_E2E").is_err() {
        return;
    }
    middleware_with_csrf_emits_wrapped_handler();

    let target = std::env::temp_dir().join("r63").join("middleware_csrf");
    let build = std::process::Command::new("cargo")
        .arg("build")
        .env("CARGO_TARGET_DIR", &target)
        .current_dir(out_dir())
        .output()
        .expect("cargo must spawn");
    assert!(
        build.status.success(),
        "#63: Middleware.withCsrf project must cargo-build\n--- cargo stderr ---\n{}",
        String::from_utf8_lossy(&build.stderr),
    );
}
