//! Runtime-config front door (`Web.appWith` + `Ipe.App`/`Host`/`Log`/`Db`
//! settings). Three proofs pin the security-critical surface:
//!
//!   * THE SEAL — `Web.appWith` carrying a shape-checked `List (Setting Web)`
//!     (a cross-cutting `Host.bind` / `Db.url (App.fromEnv …)` plus the
//!     web-pinned `Web.csrf` / `Web.sessionTtl`) is accepted (exit 0) AND the
//!     emitted crate `cargo build`s under `IPE_E2E=1`;
//!   * a hard-coded `Db.url "postgres://…"` (a `String`, not the `Secret` that
//!     only `App.fromEnv` mints) is REJECTED at ipe time — an in-source
//!     credential is unrepresentable at the boundary;
//!   * a non-`Setting` value in the `List (Setting Web)` slot (a bare `Int`) is
//!     REJECTED — the phantom shape on the settings list is enforced;
//!   * a bare `Int` where a config-tag ADT is expected (`Host.bind 7` /
//!     `Web.csrf 99` / `Log.level 5`) is REJECTED — the closed `HostMode` /
//!     `CsrfMode` / `LogLevel` types make an out-of-range tag a type error, not a
//!     value the runtime falls closed on. `CsrfMode` has no disabling variant.

use std::path::{Path, PathBuf};

use crate::support::repo_root;

fn fixture_entry(root: &Path, golden: &str) -> PathBuf {
    root.join("tests")
        .join("golden")
        .join(golden)
        .join("Main.ipe")
}

/// THE SEAL: the settings-carrying web app is accepted and (under `IPE_E2E=1`)
/// the emitted crate `cargo build`s.
#[test]
fn app_settings_web_seal_builds() {
    const GOLDEN: &str = "app_settings_web_seal";
    let root = repo_root();
    let entry = fixture_entry(&root, GOLDEN);
    let out = std::env::temp_dir().join("ipec_app_settings_web_seal_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = ipe::resolve_runtime() else {
        return; // resolver unavailable — skip
    };
    let built = ipe::build(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "a `Web.appWith` carrying a shape-checked settings list must be accepted, got: {built:?}"
    );

    crate::support::assert_seal_builds(GOLDEN, &out);
}

/// A hard-coded `String` credential passed to `Db.url` (which requires a
/// `Secret`) must be an ipe-time type error — never accepted.
#[test]
fn hard_coded_db_url_secret_is_rejected() {
    const GOLDEN: &str = "app_settings_hardcoded_secret_rejected";
    let root = repo_root();
    let entry = fixture_entry(&root, GOLDEN);
    let out = std::env::temp_dir().join("ipec_app_settings_hardcoded_secret_rejected");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = ipe::resolve_runtime() else {
        return;
    };
    let built = ipe::build(&entry, &out, &runtime);
    assert!(
        built.is_err(),
        "a hard-coded `Db.url \"postgres://…\"` (a `String`, not a `Secret`) MUST be \
         rejected — the only way to a config secret is `App.fromEnv`"
    );
}

/// A bare `Int` in the `List (Setting Web)` settings slot must be an ipe-time
/// type error — the phantom-shaped settings list only admits `Setting Web`.
#[test]
fn non_setting_in_settings_list_is_rejected() {
    const GOLDEN: &str = "app_settings_non_setting_rejected";
    let root = repo_root();
    let entry = fixture_entry(&root, GOLDEN);
    let out = std::env::temp_dir().join("ipec_app_settings_non_setting_rejected");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = ipe::resolve_runtime() else {
        return;
    };
    let built = ipe::build(&entry, &out, &runtime);
    assert!(
        built.is_err(),
        "a bare `Int` in the `List (Setting Web)` settings slot MUST be rejected — \
         the phantom shape on the settings list is enforced"
    );
}

/// `Host.bind 7` — a bare `Int` where the closed `HostMode` ADT is expected —
/// must be an ipe-time type error. An out-of-range host-bind tag is now
/// unrepresentable, not a value the runtime falls closed on.
#[test]
fn bare_int_host_bind_is_rejected() {
    const GOLDEN: &str = "app_settings_bare_int_host_rejected";
    let root = repo_root();
    let entry = fixture_entry(&root, GOLDEN);
    let out = std::env::temp_dir().join("ipec_app_settings_bare_int_host_rejected");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = ipe::resolve_runtime() else {
        return;
    };
    let built = ipe::build(&entry, &out, &runtime);
    assert!(
        built.is_err(),
        "`Host.bind 7` (a bare `Int`, not a `HostMode`) MUST be rejected — the \
         closed `HostMode` ADT makes an out-of-range host-bind tag a type error"
    );
}

/// `Web.csrf 99` — a bare `Int` where the closed `CsrfMode` ADT is expected —
/// must be an ipe-time type error. `CsrfMode` also carries no disabling variant,
/// so a setting cannot express turning CSRF off.
#[test]
fn bare_int_web_csrf_is_rejected() {
    const GOLDEN: &str = "app_settings_bare_int_csrf_rejected";
    let root = repo_root();
    let entry = fixture_entry(&root, GOLDEN);
    let out = std::env::temp_dir().join("ipec_app_settings_bare_int_csrf_rejected");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = ipe::resolve_runtime() else {
        return;
    };
    let built = ipe::build(&entry, &out, &runtime);
    assert!(
        built.is_err(),
        "`Web.csrf 99` (a bare `Int`, not a `CsrfMode`) MUST be rejected — the \
         closed `CsrfMode` ADT makes an out-of-range CSRF tag a type error"
    );
}

/// THE SEAL: `Web.authMaxLifetime <seconds>` is accepted (no N0005) and (under
/// `IPE_E2E=1`) the emitted crate `cargo build`s. Proves the `WebAuthMaxLifetime`
/// kernel is wired through canon/constrain/lower.
#[test]
fn auth_max_lifetime_seal_builds() {
    const GOLDEN: &str = "app_settings_auth_max_lifetime_seal";
    let root = repo_root();
    let entry = fixture_entry(&root, GOLDEN);
    let out = std::env::temp_dir().join("ipec_app_settings_auth_max_lifetime_seal_e2e");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = ipe::resolve_runtime() else {
        return; // resolver unavailable — skip
    };
    let built = ipe::build(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "`Web.authMaxLifetime` must be accepted (no N0005) and emit a buildable crate, \
         got: {built:?}"
    );

    crate::support::assert_seal_builds(GOLDEN, &out);
}

/// `Log.level 5` — a bare `Int` where the closed `LogLevel` ADT is expected —
/// must be an ipe-time type error.
#[test]
fn bare_int_log_level_is_rejected() {
    const GOLDEN: &str = "app_settings_bare_int_loglevel_rejected";
    let root = repo_root();
    let entry = fixture_entry(&root, GOLDEN);
    let out = std::env::temp_dir().join("ipec_app_settings_bare_int_loglevel_rejected");
    let _ = std::fs::remove_dir_all(&out);

    let Ok(runtime) = ipe::resolve_runtime() else {
        return;
    };
    let built = ipe::build(&entry, &out, &runtime);
    assert!(
        built.is_err(),
        "`Log.level 5` (a bare `Int`, not a `LogLevel`) MUST be rejected — the \
         closed `LogLevel` ADT makes an out-of-range severity tag a type error"
    );
}
