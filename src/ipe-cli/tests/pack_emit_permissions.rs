#![forbid(unsafe_code)]
//! End-to-end `ipe pack --emit-permissions`: the desktop/mobile packager's
//! capability → OS-permission derivation, invoked over the CLI. The
//! `geo-clipboard` example accepts `geolocation`, so the iOS derivation must
//! surface the location usage-description key; an app that accepts nothing emits
//! no usage key. Asserts the real subcommand runs and derives from the app's
//! declared `accepts` set.

// A failed `expect` in test setup IS the failure signal the harness reports.
#![allow(clippy::expect_used)]

mod support;

use std::process::Command;

/// Run `ipe pack --emit-permissions <platform> <path>`, returning
/// `(success, stdout, stderr)`.
fn run_emit_permissions(platform: &str, path: &str) -> (bool, String, String) {
    let out = Command::new(support::ipe_bin())
        .arg("pack")
        .arg("--emit-permissions")
        .arg(platform)
        .arg(path)
        .current_dir(support::repo_root())
        .output()
        .expect("run ipe pack --emit-permissions");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn geo_clipboard_derives_the_ios_location_permission() {
    let (ok, stdout, stderr) = run_emit_permissions("ios", "examples/shapes/web/geo-clipboard");
    assert!(
        ok,
        "`ipe pack --emit-permissions ios` must succeed on the geo-clipboard example:\n{stderr}"
    );
    assert!(
        stdout.contains("NSLocation"),
        "geo-clipboard accepts `geolocation`, so the iOS derivation must surface a \
         location usage-description key; got stdout:\n{stdout}\nstderr:\n{stderr}"
    );
}
