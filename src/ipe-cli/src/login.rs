//! `ipe login` — GitHub device-code OAuth for a publish token.
//!
//! Obtains a GitHub access token via the [device authorization grant] and stores
//! it locally so `ipe package publish`'s headless path can open the index PR
//! without a manually-exported `GITHUB_TOKEN`. Device flow needs only the public
//! `client_id` (no client secret) and no redirect/callback, so it fits a CLI: the
//! user is shown a short code to enter at a GitHub URL while `ipe` polls for the
//! token.
//!
//! The token is written to `$XDG_CONFIG_HOME/ipe/token` (or `~/.config/ipe/token`)
//! with `0600` permissions, never into the project tree.
//!
//! [device authorization grant]: https://docs.github.com/apps/oauth-apps/building-oauth-apps/authorizing-oauth-apps#device-flow

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use crate::CliError;

/// The Ipê CLI's GitHub OAuth App client id. Public by design — the device flow
/// authenticates with the client id alone (no secret), so embedding it is safe.
const CLIENT_ID: &str = "Ov23liBpCFLSoxJvSTwO";

/// The scope requested: enough to fork the public index repo and open the
/// publish pull request, nothing more.
const SCOPE: &str = "public_repo";

const DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// `ipe login [--status | --logout]` — obtain and store a GitHub publish token.
///
/// With no flag, runs the device flow and stores the token. `--status` reports
/// whether a token is stored; `--logout` removes it.
///
/// # Errors
/// [`CliError::UsageOwned`] on an unknown flag; [`CliError::Resolve`] when the
/// OAuth request fails, the user does not authorize in time, or the token cannot
/// be stored.
pub fn run_login(rest: &[String]) -> Result<(), CliError> {
    match rest.first().map(String::as_str) {
        None => run_device_flow(),
        Some("--status") if rest.len() == 1 => {
            if let Some(path) = existing_token_path() {
                print!(
                    "{}",
                    crate::style::frame(&crate::style::gutter(&format!(
                        "logged in — token stored at {}",
                        path.display()
                    )))
                );
            } else {
                print!(
                    "{}",
                    crate::style::frame(&crate::style::gutter(
                        "not logged in — run `ipe login` to authorize"
                    ))
                );
            }
            Ok(())
        }
        Some("--logout") if rest.len() == 1 => logout(),
        Some(other) => Err(CliError::UsageOwned(format!(
            "login: unexpected argument `{other}` (usage: ipe login [--status | --logout])"
        ))),
    }
}

/// The stored publish token, if the user has run `ipe login`. `None` when no
/// token file exists or it cannot be read. Consumed by the publish headless path.
#[must_use]
pub fn stored_token() -> Option<String> {
    let raw = std::fs::read_to_string(token_path()?).ok()?;
    let token = raw.trim().to_owned();
    if token.is_empty() { None } else { Some(token) }
}

/// Run the full device flow: request a code, prompt the user, poll for the token,
/// store it.
fn run_device_flow() -> Result<(), CliError> {
    let device = request_device_code()?;

    print!(
        "{}",
        crate::style::frame(&crate::style::gutter(&format!(
            "To authorize ipe, visit:\n  {}\nand enter the code:  {}",
            device.verification_uri, device.user_code
        )))
    );
    if open_in_browser(&device.verification_uri) {
        eprintln!("{}", crate::style::gutter("(opened your browser)"));
    }
    eprintln!("{}", crate::style::gutter("Waiting for authorization …"));

    let token = poll_for_token(&device)?;
    let path = store_token(&token)?;
    print!(
        "{}",
        crate::style::frame(&crate::style::gutter(&format!(
            "Logged in. Token stored at {}",
            path.display()
        )))
    );
    Ok(())
}

/// The device-code grant's first response.
struct DeviceGrant {
    device_code: String,
    user_code: String,
    verification_uri: String,
    interval: u64,
    expires_in: u64,
}

/// POST `login/device/code` and parse the device-code grant.
fn request_device_code() -> Result<DeviceGrant, CliError> {
    let json = post_form(
        DEVICE_CODE_URL,
        &[("client_id", CLIENT_ID), ("scope", SCOPE)],
    )?;
    let device_code = str_field(&json, "device_code")?;
    let user_code = str_field(&json, "user_code")?;
    let verification_uri = str_field(&json, "verification_uri")?;
    // GitHub returns these as JSON numbers; default to safe values if absent.
    let interval = json
        .get("interval")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(5);
    let expires_in = json
        .get("expires_in")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(900);
    Ok(DeviceGrant {
        device_code,
        user_code,
        verification_uri,
        interval,
        expires_in,
    })
}

/// Poll `login/oauth/access_token` until the user authorizes, the code expires,
/// or GitHub reports a terminal error.
fn poll_for_token(device: &DeviceGrant) -> Result<String, CliError> {
    let deadline = Instant::now() + Duration::from_secs(device.expires_in);
    let mut interval = device.interval.max(1);
    loop {
        std::thread::sleep(Duration::from_secs(interval));
        if Instant::now() >= deadline {
            return Err(login_error(
                "the authorization code expired before you approved it — run `ipe login` again",
            ));
        }
        let json = post_form(
            TOKEN_URL,
            &[
                ("client_id", CLIENT_ID),
                ("device_code", &device.device_code),
                ("grant_type", GRANT_TYPE),
            ],
        )?;
        if let Some(token) = json.get("access_token").and_then(serde_json::Value::as_str) {
            return Ok(token.to_owned());
        }
        match json.get("error").and_then(serde_json::Value::as_str) {
            // Not authorized yet — keep waiting at the current cadence.
            Some("authorization_pending") => {}
            // GitHub asks us to back off; it also raises the required interval.
            Some("slow_down") => {
                interval = json
                    .get("interval")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(interval + 5)
                    .max(interval + 5);
            }
            Some("access_denied") => {
                return Err(login_error("authorization was denied on GitHub"));
            }
            Some("expired_token") => {
                return Err(login_error(
                    "the authorization code expired — run `ipe login` again",
                ));
            }
            Some(other) => return Err(login_error(&format!("GitHub reported `{other}`"))),
            None => {
                return Err(login_error(
                    "GitHub's response had neither a token nor a recognised status",
                ));
            }
        }
    }
}

/// POST a form-encoded body and parse the JSON response. Shells out to `curl`
/// (the crate carries no HTTP client, mirroring the `git`-based resolver); every
/// field value here is URL-safe, so no extra encoding is needed.
fn post_form(url: &str, fields: &[(&str, &str)]) -> Result<serde_json::Value, CliError> {
    let body = fields
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    let output = Command::new("curl")
        .args(["--silent", "--show-error", "--fail", "-X", "POST"])
        .args(["-H", "Accept: application/json"])
        .args(["-d", &body, url])
        .output()
        .map_err(|e| {
            login_error(&format!(
                "could not run `curl` (needed for the GitHub OAuth request): {e}"
            ))
        })?;
    if !output.status.success() {
        return Err(login_error(&format!(
            "the OAuth request to GitHub failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|e| login_error(&format!("could not parse GitHub's response as JSON: {e}")))
}

/// Extract a required string field, erroring if it is absent.
fn str_field(json: &serde_json::Value, key: &str) -> Result<String, CliError> {
    json.get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| login_error(&format!("GitHub's response was missing `{key}`")))
}

/// The token file path (`$XDG_CONFIG_HOME/ipe/token`, else `~/.config/ipe/token`).
/// `None` only when neither `XDG_CONFIG_HOME` nor `HOME` is set.
fn token_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("ipe").join("token"))
}

/// The token path if a token is actually stored there.
fn existing_token_path() -> Option<PathBuf> {
    token_path().filter(|p| p.is_file())
}

/// Write the token with owner-only permissions, creating the config dir.
fn store_token(token: &str) -> Result<PathBuf, CliError> {
    let path = token_path().ok_or_else(|| {
        login_error("could not determine a config directory (set HOME or XDG_CONFIG_HOME)")
    })?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| login_error(&format!("could not create {}: {e}", parent.display())))?;
    }
    std::fs::write(&path, format!("{token}\n"))
        .map_err(|e| login_error(&format!("could not write {}: {e}", path.display())))?;
    restrict_permissions(&path)?;
    Ok(path)
}

/// Remove the stored token.
fn logout() -> Result<(), CliError> {
    let Some(path) = token_path().filter(|p| p.exists()) else {
        print!(
            "{}",
            crate::style::frame(&crate::style::gutter("not logged in — nothing to remove"))
        );
        return Ok(());
    };
    std::fs::remove_file(&path)
        .map_err(|e| login_error(&format!("could not remove {}: {e}", path.display())))?;
    print!(
        "{}",
        crate::style::frame(&crate::style::gutter(&format!(
            "logged out — removed {}",
            path.display()
        )))
    );
    Ok(())
}

/// Restrict the token file to owner read/write (`0600`) on Unix. A no-op
/// elsewhere; the containing user profile directory is the protection there.
#[cfg(unix)]
fn restrict_permissions(path: &std::path::Path) -> Result<(), CliError> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|e| {
        login_error(&format!(
            "could not set permissions on {}: {e}",
            path.display()
        ))
    })
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &std::path::Path) -> Result<(), CliError> {
    Ok(())
}

/// Best-effort browser open (same contract as publish's opener).
fn open_in_browser(url: &str) -> bool {
    let mut command = if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        c.arg(url);
        c
    } else if cfg!(target_os = "windows") {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", "", url]);
        c
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(url);
        c
    };
    command.status().is_ok_and(|s| s.success())
}

/// Build a login error.
fn login_error(message: &str) -> CliError {
    CliError::Resolve(format!("ipe login: {message}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_device_code_response() {
        let json: serde_json::Value = serde_json::from_str(
            r#"{"device_code":"abc","user_code":"WXYZ-1234","verification_uri":"https://github.com/login/device","expires_in":899,"interval":5}"#,
        )
        .unwrap();
        assert_eq!(str_field(&json, "device_code").unwrap(), "abc");
        assert_eq!(str_field(&json, "user_code").unwrap(), "WXYZ-1234");
        assert_eq!(
            json.get("interval").and_then(serde_json::Value::as_u64),
            Some(5)
        );
    }

    #[test]
    fn a_missing_field_is_a_typed_error() {
        let json: serde_json::Value = serde_json::from_str(r#"{"user_code":"X"}"#).unwrap();
        assert!(str_field(&json, "device_code").is_err());
    }

    #[test]
    fn unexpected_login_argument_is_a_usage_error() {
        let result = run_login(&["--bogus".to_owned()]);
        assert!(matches!(result, Err(CliError::UsageOwned(_))));
    }
}
