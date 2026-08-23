//! `Ipe.App` runtime-config front door — the process-wide typed settings a shape
//! app installs at startup.
//!
//! A [`Setting`] is the one concrete carrier the phantom-typed Ipê `Setting
//! shape` erases to (one carrier per position, no `dyn`). The setting-builder
//! kernels ([`ipe_setting_host_bind`] / [`ipe_setting_log_level`] /
//! [`ipe_setting_db_url`] / [`ipe_setting_web_csrf`] /
//! [`ipe_setting_web_session_ttl`]) each produce one; a shape app's entry
//! installs the whole list through [`install_web`] before its server binds.
//!
//! # One precedence
//!
//! Every resolvable value obeys a single order: **env var > setting-in-code >
//! built-in fallback**. Env always wins, so an operator can override any
//! in-code setting without a rebuild; absent both, the fallback is the safe
//! default.
//!
//! # Host bind — fail-closed to loopback
//!
//! [`resolve_host_bind`] is the security-critical resolution: a development
//! build binds `127.0.0.1` (never exposed on the LAN), a production build binds
//! all interfaces, and `IPE_HTTP_BIND` overrides either. Absent any signal the
//! conservative loopback is chosen — the dev console is never reachable off-box
//! by default.

use std::sync::OnceLock;

/// A resolved host-bind mode. The Ipê-source `HostMode` ADT projects onto this
/// closed set; a bare integer cannot stand in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HostMode {
    /// Bind `127.0.0.1` only — never reachable off the local machine.
    Loopback,
    /// Bind `0.0.0.0` — reachable on every interface.
    AllInterfaces,
    /// Defer to the environment (`IPE_HTTP_BIND`, else the build-profile default).
    EnvDriven,
}

/// The runtime-config carrier `ipe_runtime::app_config::Setting` — the single
/// concrete type every phantom `Setting shape` position erases to. `Clone` (a
/// setting may be stored and read at more than one site); never serde (a
/// `DbUrl` carries a [`Secret`](crate::secret::Secret), so a `Setting` in a Web
/// Model is a compile-time rejection, never a session-store leak).
#[derive(Clone)]
pub enum Setting {
    /// `Host.bind` — the requested host-bind mode.
    HostBind(HostMode),
    /// `Log.level` — the requested minimum log severity (`0` debug … `3` error).
    LogLevel(i64),
    /// `Db.url` — the database URL, sealed as a [`Secret`](crate::secret::Secret).
    #[cfg(feature = "secret")]
    DbUrl(crate::secret::Secret),
    /// `Web.csrf` — the CSRF policy tag (`0` strict / `1` disabled).
    WebCsrf(i64),
    /// `Web.sessionTtl` — the session lifetime in seconds.
    WebSessionTtl(i64),
}

/// `Host.bind : HostMode -> Setting a`. The Ipê `HostMode` enum projects onto
/// the closed [`HostMode`] tag: `0` loopback, `1` all interfaces, `2`
/// env-driven. An out-of-range tag falls closed to [`HostMode::Loopback`] (the
/// safe branch), never a panic.
#[must_use]
pub fn ipe_setting_host_bind(mode_tag: i64) -> Setting {
    let mode = match mode_tag {
        1 => HostMode::AllInterfaces,
        2 => HostMode::EnvDriven,
        _ => HostMode::Loopback,
    };
    Setting::HostBind(mode)
}

/// `Log.level : LogLevel -> Setting a`. Carries the severity tag as-is.
#[must_use]
pub fn ipe_setting_log_level(level_tag: i64) -> Setting {
    Setting::LogLevel(level_tag)
}

/// `App.fromEnv : String -> Secret` — the sole env-secret seal. Reads the named
/// environment variable at startup and seals its value into a [`Secret`], so a
/// config credential is never a hard-coded string in source. A missing/empty
/// var seals the empty string (fail-safe: the downstream consumer sees an empty
/// secret rather than a panic); the operator supplies the value at deploy time.
#[cfg(feature = "secret")]
#[must_use]
pub fn ipe_app_from_env(var_name: String) -> crate::secret::Secret {
    let value = crate::system::read_env_var(&var_name).unwrap_or_default();
    crate::secret::secret_from_string(value)
}

/// `Db.url : Secret -> Setting a`. The URL is already a sealed [`Secret`],
/// carried unchanged.
#[cfg(feature = "secret")]
#[must_use]
pub fn ipe_setting_db_url(url: crate::secret::Secret) -> Setting {
    Setting::DbUrl(url)
}

/// `Web.csrf : CsrfMode -> Setting Web`. Carries the CSRF policy tag.
#[must_use]
pub fn ipe_setting_web_csrf(mode_tag: i64) -> Setting {
    Setting::WebCsrf(mode_tag)
}

/// `Web.sessionTtl : Int -> Setting Web`. Carries the session lifetime (seconds).
#[must_use]
pub fn ipe_setting_web_session_ttl(seconds: i64) -> Setting {
    Setting::WebSessionTtl(seconds)
}

/// The resolved, immutable process-wide config, installed once at startup.
#[derive(Default)]
struct ResolvedConfig {
    host_bind: Option<HostMode>,
}

static INSTALLED: OnceLock<ResolvedConfig> = OnceLock::new();

/// Install a Web shape app's settings into the process-wide config. Resolves the
/// in-code settings (env override is applied at read time, so env always wins);
/// a second install is ignored (`OnceLock`), keeping the first app's config
/// authoritative for the process.
pub fn install_web(settings: Vec<Setting>) {
    let mut cfg = ResolvedConfig::default();
    for s in settings {
        if let Setting::HostBind(mode) = s {
            cfg.host_bind = Some(mode);
        }
    }
    // First install wins; a redundant install is a no-op (never a panic).
    let _ = INSTALLED.set(cfg);
}

/// Resolve the host-bind address string, applying the one precedence:
/// `IPE_HTTP_BIND` (env) > the installed `Host.bind` setting > the build-profile
/// fallback. The fallback is loopback in a debug build and all-interfaces in a
/// release build; an `EnvDriven` in-code setting defers to that same fallback.
/// The conservative default absent any signal is loopback, so a dev console is
/// never reachable off-box by accident.
#[must_use]
pub fn resolve_host_bind() -> String {
    // Env override wins unconditionally (a non-blank value).
    if let Ok(raw) = crate::system::read_env_var("IPE_HTTP_BIND") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return trimmed.to_owned();
        }
    }
    let profile_default = || {
        if cfg!(debug_assertions) {
            "127.0.0.1".to_owned()
        } else {
            "0.0.0.0".to_owned()
        }
    };
    match INSTALLED.get().and_then(|c| c.host_bind) {
        Some(HostMode::Loopback) => "127.0.0.1".to_owned(),
        Some(HostMode::AllInterfaces) => "0.0.0.0".to_owned(),
        Some(HostMode::EnvDriven) | None => profile_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_bind_tag_projects_to_the_closed_mode_set() {
        assert!(matches!(
            ipe_setting_host_bind(0),
            Setting::HostBind(HostMode::Loopback)
        ));
        assert!(matches!(
            ipe_setting_host_bind(1),
            Setting::HostBind(HostMode::AllInterfaces)
        ));
        assert!(matches!(
            ipe_setting_host_bind(2),
            Setting::HostBind(HostMode::EnvDriven)
        ));
    }

    #[test]
    fn out_of_range_host_tag_falls_closed_to_loopback() {
        assert!(matches!(
            ipe_setting_host_bind(99),
            Setting::HostBind(HostMode::Loopback)
        ));
    }
}
