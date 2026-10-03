//! The one way the CLI opens a URL in the user's browser.
//!
//! A URL reaches the platform opener only as a [`BrowserUrl`], parsed once for
//! its origin, and the opener is started as a plain argv through the runtime's
//! hardened spawner: no shell ever re-parses the URL.

use std::ffi::OsStr;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use ipe_runtime_rust::system::{SpawnRefusal, spawn_hardened};

/// How long [`open_url`] waits for the opener to exit before leaving it running.
const OPENER_GRACE: Duration = Duration::from_secs(5);

/// How often [`open_url`] checks whether the opener has exited.
const OPENER_POLL: Duration = Duration::from_millis(50);

/// The one authority a [`BrowserOrigin::GitHub`] URL may name.
const GITHUB_AUTHORITY: &str = "github.com";

/// The hosts a [`BrowserOrigin::Loopback`] URL may name.
const LOOPBACK_HOSTS: [&str; 2] = ["127.0.0.1", "localhost"];

/// A URL proven safe to hand to the platform opener for its origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserUrl(String);

/// Where a [`BrowserUrl`] may point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserOrigin {
    /// `https://github.com`, with no userinfo and no port.
    GitHub,
    /// `http://127.0.0.1:<port>` or `http://localhost:<port>`, the port in `1..=65535`.
    Loopback,
}

/// Why a raw string is not a [`BrowserUrl`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserUrlRefusal {
    /// The URL does not start with its origin's scheme.
    Scheme,
    /// The URL names a host its origin does not admit.
    Host,
    /// The URL's port is missing, malformed, out of range, or not admitted.
    Port,
    /// The URL carries userinfo that could mask its host.
    Userinfo,
    /// The URL holds this byte where it is not admitted.
    Byte(u8),
    /// The URL is empty.
    Empty,
}

impl std::fmt::Display for BrowserUrlRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Scheme => f.write_str("the URL has the wrong scheme"),
            Self::Host => f.write_str("the URL names a host that is not admitted"),
            Self::Port => f.write_str("the URL's port is not admitted"),
            Self::Userinfo => f.write_str("the URL carries userinfo"),
            Self::Byte(byte) => write!(f, "the URL holds the byte {byte:#04x}"),
            Self::Empty => f.write_str("the URL is empty"),
        }
    }
}

/// Whether `byte` may appear anywhere in a [`BrowserUrl`].
///
/// Excluded: space, controls, `"`, `^`, `|`, `<`, `>`, `\`, `` ` ``, `{`, `}`,
/// `,` (explorer splits on commas), and every non-ASCII byte.
const fn is_url_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'-' | b'.'
                | b'_'
                | b'~'
                | b':'
                | b'/'
                | b'?'
                | b'#'
                | b'['
                | b']'
                | b'@'
                | b'!'
                | b'$'
                | b'&'
                | b'\''
                | b'('
                | b')'
                | b'*'
                | b'+'
                | b';'
                | b'='
                | b'%'
        )
}

/// Whether `byte` may appear in a [`BrowserUrl`]'s path.
///
/// The path admits no shell-significant delimiter (`&`, `'`, `(`, `)`, `;`,
/// `!`, `$`, `*`, `[`, `]`): those belong to a query or a fragment.
const fn is_path_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'-' | b'.' | b'_' | b'~' | b':' | b'/' | b'@' | b'+' | b'=' | b'%'
        )
}

/// The first byte of `raw` not admitted by `admitted`, or a `%` not followed by two hex digits.
fn first_refused_byte(raw: &[u8], admitted: fn(u8) -> bool) -> Option<u8> {
    let mut bytes = raw.iter().copied().enumerate();
    while let Some((at, byte)) = bytes.next() {
        if !admitted(byte) {
            return Some(byte);
        }
        if byte == b'%' {
            let hex = raw.get(at + 1..at + 3);
            if !hex.is_some_and(|pair| pair.iter().all(u8::is_ascii_hexdigit)) {
                return Some(byte);
            }
            bytes.nth(1);
        }
    }
    None
}

/// Whether `port` is decimal digits naming a port in `1..=65535`.
fn is_loopback_port(port: &str) -> bool {
    !port.is_empty()
        && port.len() <= 5
        && port.bytes().all(|b| b.is_ascii_digit())
        && port.parse::<u32>().is_ok_and(|n| (1..=65535).contains(&n))
}

impl BrowserUrl {
    /// Parse `raw` as a URL of `origin`.
    ///
    /// # Errors
    /// The [`BrowserUrlRefusal`] naming the first part of `raw` not admitted.
    pub fn parse(raw: &str, origin: BrowserOrigin) -> Result<Self, BrowserUrlRefusal> {
        if raw.is_empty() {
            return Err(BrowserUrlRefusal::Empty);
        }
        let scheme = match origin {
            BrowserOrigin::GitHub => "https://",
            BrowserOrigin::Loopback => "http://",
        };
        let rest = raw.strip_prefix(scheme).ok_or(BrowserUrlRefusal::Scheme)?;
        if let Some(byte) = first_refused_byte(raw.as_bytes(), is_url_byte) {
            return Err(BrowserUrlRefusal::Byte(byte));
        }
        let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let (authority, after) = rest.split_at(authority_end);
        if authority.contains('@') {
            return Err(BrowserUrlRefusal::Userinfo);
        }
        match origin {
            BrowserOrigin::GitHub => {
                if authority != GITHUB_AUTHORITY {
                    return Err(if authority.starts_with("github.com:") {
                        BrowserUrlRefusal::Port
                    } else {
                        BrowserUrlRefusal::Host
                    });
                }
            }
            BrowserOrigin::Loopback => {
                let (host, port) = authority.rsplit_once(':').ok_or(BrowserUrlRefusal::Port)?;
                if !LOOPBACK_HOSTS.contains(&host) {
                    return Err(BrowserUrlRefusal::Host);
                }
                if !is_loopback_port(port) {
                    return Err(BrowserUrlRefusal::Port);
                }
            }
        }
        let path_end = after.find(['?', '#']).unwrap_or(after.len());
        let (path, _) = after.split_at(path_end);
        if let Some(byte) = first_refused_byte(path.as_bytes(), is_path_byte) {
            return Err(BrowserUrlRefusal::Byte(byte));
        }
        Ok(Self(raw.to_owned()))
    }

    /// The URL text.
    pub const fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

/// What starting the platform opener on a URL did.
#[derive(Debug)]
pub enum OpenOutcome {
    /// The opener exited successfully, or was still running at the grace and left to run.
    Opened,
    /// The opener exited unsuccessfully.
    OpenerFailed(ExitStatus),
    /// The platform has no opener installed.
    OpenerMissing,
    /// The hardened spawner refused or failed to start the opener.
    Spawn(SpawnRefusal),
    /// Checking whether the opener had exited failed.
    Wait(std::io::ErrorKind),
}

impl std::fmt::Display for OpenOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Opened => f.write_str("opened the browser"),
            Self::OpenerFailed(status) => write!(f, "the browser opener exited with {status}"),
            Self::OpenerMissing => f.write_str("no browser opener is installed"),
            Self::Spawn(refusal) => write!(f, "the browser opener could not start: {refusal}"),
            Self::Wait(kind) => write!(f, "waiting on the browser opener failed: {kind}"),
        }
    }
}

/// The platform whose opener [`open_url`] runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Platform {
    /// macOS, opened through `open`.
    MacOs,
    /// Windows, opened through `explorer.exe`.
    Windows,
    /// Every other platform, opened through `xdg-open`.
    Other,
}

impl Platform {
    /// The platform this CLI was built for.
    const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Other
        }
    }
}

/// The opener program and its one argument for `url` on `platform`.
///
/// No platform runs a shell: Windows starts `explorer.exe` with the URL as its
/// single argument, never `cmd /C start`.
const fn opener_argv(platform: Platform, url: &BrowserUrl) -> (&'static str, [&str; 1]) {
    let program = match platform {
        Platform::MacOs => "open",
        Platform::Windows => "explorer.exe",
        Platform::Other => "xdg-open",
    };
    (program, [url.as_str()])
}

/// Open `url` in the user's browser through the platform opener.
///
/// Every caller prints the URL when the outcome is not [`OpenOutcome::Opened`].
pub fn open_url(url: &BrowserUrl) -> OpenOutcome {
    let (program, args) = opener_argv(Platform::current(), url);
    open_with(OsStr::new(program), &args, OPENER_GRACE)
}

/// Start `program` on `args` with null stdio and wait up to `grace` for it to exit.
///
/// An opener still running at `grace` counts as opened and is left running: a
/// dropped `Child` is not killed, and on Linux the parent-death floor reaches
/// only the opener, never the browser it started.
fn open_with(program: &OsStr, args: &[&str; 1], grace: Duration) -> OpenOutcome {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = match spawn_hardened(command) {
        Ok(child) => child,
        Err(SpawnRefusal::Spawn(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            return OpenOutcome::OpenerMissing;
        }
        Err(refusal) => return OpenOutcome::Spawn(refusal),
    };
    let started = Instant::now();
    let deadline = started.checked_add(grace).unwrap_or(started);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return OpenOutcome::Opened,
            Ok(Some(status)) => return OpenOutcome::OpenerFailed(status),
            Ok(None) if Instant::now() >= deadline => return OpenOutcome::Opened,
            Ok(None) => std::thread::sleep(OPENER_POLL),
            Err(e) => return OpenOutcome::Wait(e.kind()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `raw` parsed as a GitHub URL.
    fn github(raw: &str) -> Result<BrowserUrl, BrowserUrlRefusal> {
        BrowserUrl::parse(raw, BrowserOrigin::GitHub)
    }

    /// `raw` parsed as a loopback URL.
    fn loopback(raw: &str) -> Result<BrowserUrl, BrowserUrlRefusal> {
        BrowserUrl::parse(raw, BrowserOrigin::Loopback)
    }

    #[test]
    fn a_github_device_url_is_accepted_verbatim() {
        let url = github("https://github.com/login/device").expect("accepted");
        assert_eq!(url.as_str(), "https://github.com/login/device");
    }

    #[test]
    fn a_github_compare_url_with_a_query_is_accepted() {
        let raw = "https://github.com/o/ipe-registry/compare/main...octocat:publish/foo-1.2.3\
                   ?quick_pull=1&title=Publish%20foo%201.2.3";
        assert!(github(raw).is_ok());
    }

    #[test]
    fn a_loopback_url_is_accepted() {
        assert!(loopback("http://127.0.0.1:8080/").is_ok());
        assert!(loopback("http://localhost:65535").is_ok());
    }

    #[test]
    fn an_empty_url_is_refused() {
        assert_eq!(github(""), Err(BrowserUrlRefusal::Empty));
        assert_eq!(loopback(""), Err(BrowserUrlRefusal::Empty));
    }

    #[test]
    fn a_wrong_scheme_is_refused() {
        assert_eq!(
            github("http://github.com/x"),
            Err(BrowserUrlRefusal::Scheme)
        );
        assert_eq!(github("file:///etc/passwd"), Err(BrowserUrlRefusal::Scheme));
        assert_eq!(
            github("javascript:alert(1)"),
            Err(BrowserUrlRefusal::Scheme)
        );
        assert_eq!(
            loopback("https://127.0.0.1:80/"),
            Err(BrowserUrlRefusal::Scheme)
        );
    }

    #[test]
    fn another_host_is_refused() {
        for raw in [
            "https://evil.example.com/login/device",
            "https://github.com.evil.com/x",
            "https://notgithub.com/x",
        ] {
            assert_eq!(github(raw), Err(BrowserUrlRefusal::Host), "{raw}");
        }
        assert_eq!(
            loopback("http://10.0.0.1:80/"),
            Err(BrowserUrlRefusal::Host)
        );
    }

    #[test]
    fn github_userinfo_is_refused() {
        assert_eq!(
            github("https://github.com@evil.example.com/x"),
            Err(BrowserUrlRefusal::Userinfo)
        );
        assert_eq!(
            loopback("http://user@127.0.0.1:80/"),
            Err(BrowserUrlRefusal::Userinfo)
        );
    }

    #[test]
    fn a_github_port_is_refused() {
        assert_eq!(
            github("https://github.com:8443/login/device"),
            Err(BrowserUrlRefusal::Port)
        );
    }

    #[test]
    fn a_loopback_port_with_a_plus_is_refused() {
        assert_eq!(
            loopback("http://127.0.0.1:+80/"),
            Err(BrowserUrlRefusal::Port)
        );
    }

    #[test]
    fn a_loopback_port_out_of_range_or_missing_is_refused() {
        for raw in [
            "http://127.0.0.1:0/",
            "http://127.0.0.1:65536/",
            "http://127.0.0.1:000080/",
            "http://127.0.0.1:/",
            "http://127.0.0.1/",
        ] {
            assert_eq!(loopback(raw), Err(BrowserUrlRefusal::Port), "{raw}");
        }
    }

    #[test]
    fn a_verification_uri_with_a_shell_metacharacter_is_refused() {
        assert_eq!(
            github("https://github.com/login/device&calc"),
            Err(BrowserUrlRefusal::Byte(b'&'))
        );
        for byte in *b"'();!$*[]" {
            let raw = format!("https://github.com/login/device{}x", char::from(byte));
            assert_eq!(github(&raw), Err(BrowserUrlRefusal::Byte(byte)), "{raw}");
        }
    }

    #[test]
    fn every_refused_byte_class_is_refused() {
        for byte in [
            b' ', b'\t', b'\n', 0x1b, 0x7f, b'"', b'^', b'|', b'<', b'>', b'\\', b'`', b'{', b'}',
            b',',
        ] {
            let raw = format!("https://github.com/x?q={}", char::from(byte));
            assert_eq!(github(&raw), Err(BrowserUrlRefusal::Byte(byte)), "{raw:?}");
        }
        assert_eq!(
            github("https://github.com/caf\u{e9}"),
            Err(BrowserUrlRefusal::Byte(0xc3))
        );
    }

    #[test]
    fn a_percent_without_two_hex_digits_is_refused() {
        for raw in [
            "https://github.com/x%2",
            "https://github.com/x%zz",
            "https://github.com/x?q=%",
        ] {
            assert_eq!(github(raw), Err(BrowserUrlRefusal::Byte(b'%')), "{raw}");
        }
        assert!(github("https://github.com/x%2Fy").is_ok());
    }

    #[test]
    fn the_windows_opener_never_runs_a_shell() {
        let url = github("https://github.com/login/device").expect("accepted");
        let (program, args) = opener_argv(Platform::Windows, &url);
        assert_eq!(program, "explorer.exe");
        assert_eq!(args, [url.as_str()]);
    }

    #[test]
    fn the_unix_openers_take_the_url_as_one_argument() {
        let url = github("https://github.com/login/device").expect("accepted");
        assert_eq!(opener_argv(Platform::MacOs, &url), ("open", [url.as_str()]));
        assert_eq!(
            opener_argv(Platform::Other, &url),
            ("xdg-open", [url.as_str()])
        );
    }

    /// An executable `sh` stub under a fresh scratch dir for `tag`, running `body`.
    #[cfg(unix)]
    fn stub(tag: &str, body: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt as _;
        let base =
            ipe_test_temp::temp_root().join(format!("ipe-browser-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("scratch base");
        let opener = base.join("opener");
        std::fs::write(&opener, format!("#!/bin/sh\n{body}\n")).expect("write stub");
        std::fs::set_permissions(&opener, std::fs::Permissions::from_mode(0o755))
            .expect("stub executable");
        (base, opener)
    }

    #[cfg(unix)]
    #[test]
    fn an_opener_still_running_at_the_grace_is_opened_and_not_killed() {
        let (base, opener) = stub("grace", "sleep 2; touch \"$(dirname \"$0\")/finished\"");
        let started = Instant::now();
        let outcome = open_with(
            opener.as_os_str(),
            &["http://127.0.0.1:1/"],
            Duration::from_millis(500),
        );
        assert!(matches!(outcome, OpenOutcome::Opened), "{outcome}");
        assert!(started.elapsed() < Duration::from_secs(2));
        let finished = base.join("finished");
        let waited = Instant::now();
        while !finished.exists() && waited.elapsed() < Duration::from_secs(20) {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(finished.exists(), "the opener ran on past the grace");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn a_failing_opener_reports_failure() {
        let (base, opener) = stub("fail", "exit 3");
        let outcome = open_with(opener.as_os_str(), &["http://127.0.0.1:1/"], OPENER_GRACE);
        assert!(
            matches!(outcome, OpenOutcome::OpenerFailed(status) if status.code() == Some(3)),
            "{outcome}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_missing_opener_is_reported_missing() {
        let outcome = open_with(
            OsStr::new("ipe-no-such-browser-opener"),
            &["http://127.0.0.1:1/"],
            OPENER_GRACE,
        );
        assert!(matches!(outcome, OpenOutcome::OpenerMissing), "{outcome}");
    }
}
