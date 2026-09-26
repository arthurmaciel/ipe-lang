//! The SSH key `ipe package publish` signs the index commit with: where publish
//! finds it, and the opt-in `ipe login` step that generates one and registers it
//! on the GitHub account as a *signing* key.
//!
//! Lookup order: `IPE_PUBLISH_SIGNING_KEY` wins whenever it is set — an explicit
//! value that names no usable key file fails closed, it never falls back to the
//! stored key. Unset, publish uses the key `ipe login` stored at
//! `<config dir>/signing_key`.
//!
//! Setup is interactive and opt-in only. It generates a dedicated ed25519 key in
//! process (no `ssh-keygen` subprocess), stages both halves in the config dir
//! under fresh names (the private half created `0600`, exclusively, never through
//! a symlink), registers the public half through `POST /user/ssh_signing_keys`
//! with a separate one-shot `write:ssh_signing_key` authorization, and only then
//! links the staged files into their final names. Any failure before that commit
//! point removes the staged files, so a key the account does not know is never
//! left where publish would pick it up.

use std::ffi::OsStr;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{IsTerminal as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use zeroize::Zeroizing;

use crate::CliError;
use crate::login::KeyRegistrationToken;

/// The environment variable naming the SSH signing key's private-key file.
pub const SIGNING_KEY_ENV: &str = "IPE_PUBLISH_SIGNING_KEY";

/// File name of the generated private key inside the ipe config dir.
const PRIVATE_KEY_FILE: &str = "signing_key";
/// File name of the generated public key inside the ipe config dir.
const PUBLIC_KEY_FILE: &str = "signing_key.pub";

/// The OpenSSH key-type name for ed25519.
const KEY_TYPE: &[u8] = b"ssh-ed25519";
/// The comment embedded in the generated key.
const KEY_COMMENT: &str = "ipe-publish";
/// The title the key is registered under on the GitHub account.
const KEY_TITLE: &str = "ipe package publish";

/// The GitHub REST endpoint that adds an SSH *signing* key (`/user/keys` adds
/// authentication keys, which GitHub never uses to mark a commit "Verified").
const SIGNING_KEYS_API: &str = "https://api.github.com/user/ssh_signing_keys";
/// Where the user reviews or deletes registered keys.
const SIGNING_KEYS_SETTINGS: &str = "https://github.com/settings/keys";

/// Upper bound on the length of a GitHub error message echoed to the terminal.
const MAX_ECHOED_MESSAGE_CHARS: usize = 200;

/// Path to an SSH private-key file that publish hands to `git` for signing.
///
/// Holds only a path that named a regular file when it was parsed; the key bytes
/// never enter the process through this type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrivateKeyPath(PathBuf);

impl PrivateKeyPath {
    /// Parse an `IPE_PUBLISH_SIGNING_KEY` value: a non-blank UTF-8 path to an
    /// existing regular file (symlinks followed — the path is the user's choice).
    #[must_use]
    pub fn from_env_value(raw: &OsStr) -> Option<Self> {
        let trimmed = raw.to_str()?.trim();
        if trimmed.is_empty() {
            return None;
        }
        let path = PathBuf::from(trimmed);
        std::fs::metadata(&path)
            .is_ok_and(|m| m.is_file())
            .then_some(Self(path))
    }

    /// The key `ipe login` stored in `config_dir`, when it is a regular file.
    /// A symlink at that name is not accepted: `ipe login` never creates one.
    fn stored(config_dir: &Path) -> Option<Self> {
        let path = config_dir.join(PRIVATE_KEY_FILE);
        std::fs::symlink_metadata(&path)
            .is_ok_and(|m| m.is_file())
            .then_some(Self(path))
    }

    /// The private-key file's path.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

/// Where the key publish will sign with comes from.
#[derive(Debug, PartialEq, Eq)]
pub enum KeyLookup {
    /// `IPE_PUBLISH_SIGNING_KEY` names a usable key; it wins over a stored key.
    Env(PrivateKeyPath),
    /// `IPE_PUBLISH_SIGNING_KEY` is set but names no usable key file. Publish
    /// refuses rather than silently use a different (stored) key.
    EnvUnusable,
    /// The key `ipe login` generated and registered.
    Stored(PrivateKeyPath),
    /// No key is configured.
    Missing,
}

impl KeyLookup {
    /// The key publish signs with, if any.
    #[must_use]
    pub fn usable(self) -> Option<PrivateKeyPath> {
        match self {
            Self::Env(path) | Self::Stored(path) => Some(path),
            Self::EnvUnusable | Self::Missing => None,
        }
    }
}

/// Resolve the signing key from the `IPE_PUBLISH_SIGNING_KEY` value and the ipe
/// config dir. A set, non-blank variable is authoritative; otherwise the stored
/// key is used.
#[must_use]
pub fn lookup(env_value: Option<&OsStr>, config_dir: Option<&Path>) -> KeyLookup {
    let explicit = env_value.filter(|v| !v.to_str().is_some_and(|s| s.trim().is_empty()));
    explicit.map_or_else(
        || {
            config_dir
                .and_then(PrivateKeyPath::stored)
                .map_or(KeyLookup::Missing, KeyLookup::Stored)
        },
        |raw| PrivateKeyPath::from_env_value(raw).map_or(KeyLookup::EnvUnusable, KeyLookup::Env),
    )
}

/// The signing key publish uses in this process's environment.
#[must_use]
pub fn configured() -> Option<PrivateKeyPath> {
    lookup(
        std::env::var_os(SIGNING_KEY_ENV).as_deref(),
        crate::login::config_dir().as_deref(),
    )
    .usable()
}

/// One line for `ipe login --status` naming the key publish would sign with.
pub(crate) fn status_line(env_value: Option<&OsStr>, config_dir: Option<&Path>) -> String {
    match lookup(env_value, config_dir) {
        KeyLookup::Env(path) => format!(
            "signing key: {} (from {SIGNING_KEY_ENV})",
            path.as_path().display()
        ),
        KeyLookup::EnvUnusable => format!(
            "signing key: {SIGNING_KEY_ENV} is set but names no readable key file — publish will refuse"
        ),
        KeyLookup::Stored(path) => format!(
            "signing key: {} (generated by `ipe login`)",
            path.as_path().display()
        ),
        KeyLookup::Missing => {
            "signing key: none — run `ipe login --signing-key` to generate and register one"
                .to_owned()
        }
    }
}

/// An OpenSSH public-key line (`ssh-ed25519 <base64> <comment>`), produced only
/// by [`GeneratedKeyPair`].
pub(crate) struct SshPublicKey(String);

impl SshPublicKey {
    /// The public-key line, as GitHub's `key` field expects it.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// A freshly generated ed25519 keypair in OpenSSH encoding. The private half is
/// wiped from memory on drop.
struct GeneratedKeyPair {
    private_pem: Zeroizing<String>,
    public: SshPublicKey,
}

impl GeneratedKeyPair {
    /// Generate a keypair from the OS CSPRNG. `None` when the CSPRNG fails.
    fn generate() -> Option<Self> {
        let mut seed = Zeroizing::new([0u8; 32]);
        getrandom::fill(seed.as_mut_slice()).ok()?;
        let mut check = [0u8; 4];
        getrandom::fill(&mut check).ok()?;
        Self::from_seed(&seed, u32::from_be_bytes(check))
    }

    /// Encode the keypair for `seed` in the unencrypted `openssh-key-v1` format.
    /// `check` is the format's integrity word, written twice in the private
    /// section.
    fn from_seed(seed: &[u8; 32], check: u32) -> Option<Self> {
        let public_key = ed25519_dalek::SigningKey::from_bytes(seed)
            .verifying_key()
            .to_bytes();

        let mut public_blob = Vec::with_capacity(51);
        put_string(&mut public_blob, KEY_TYPE)?;
        put_string(&mut public_blob, &public_key)?;

        // The OpenSSH ed25519 private scalar field is `seed || public key`.
        let mut secret = Zeroizing::new(Vec::<u8>::with_capacity(64));
        secret.extend_from_slice(seed);
        secret.extend_from_slice(&public_key);

        let mut private_section = Zeroizing::new(Vec::<u8>::with_capacity(160));
        private_section.extend_from_slice(&check.to_be_bytes());
        private_section.extend_from_slice(&check.to_be_bytes());
        put_string(&mut private_section, KEY_TYPE)?;
        put_string(&mut private_section, &public_key)?;
        put_string(&mut private_section, &secret)?;
        put_string(&mut private_section, KEY_COMMENT.as_bytes())?;
        // Pad to the cipher block size (8 for `none`) with 1, 2, 3, …
        let mut pad: u8 = 1;
        while !private_section.len().is_multiple_of(8) {
            private_section.push(pad);
            pad = pad.wrapping_add(1);
        }

        let mut body = Zeroizing::new(Vec::<u8>::with_capacity(256));
        body.extend_from_slice(b"openssh-key-v1\0");
        put_string(&mut body, b"none")?; // cipher
        put_string(&mut body, b"none")?; // kdf
        put_string(&mut body, b"")?; // kdf options
        body.extend_from_slice(&1u32.to_be_bytes()); // key count
        put_string(&mut body, &public_blob)?;
        put_string(&mut body, &private_section)?;

        let encoded = Zeroizing::new(base64::engine::general_purpose::STANDARD.encode(&*body));
        let mut private_pem = Zeroizing::new(String::with_capacity(encoded.len() + 80));
        private_pem.push_str("-----BEGIN OPENSSH PRIVATE KEY-----\n");
        for line in encoded.as_bytes().chunks(70) {
            private_pem.push_str(std::str::from_utf8(line).ok()?);
            private_pem.push('\n');
        }
        private_pem.push_str("-----END OPENSSH PRIVATE KEY-----\n");

        let public = SshPublicKey(format!(
            "ssh-ed25519 {} {KEY_COMMENT}",
            base64::engine::general_purpose::STANDARD.encode(&public_blob)
        ));
        Some(Self {
            private_pem,
            public,
        })
    }
}

/// Append an SSH wire-format `string` (big-endian `u32` length, then bytes).
/// `None` only for a field longer than `u32::MAX`, which no key field is.
fn put_string(out: &mut Vec<u8>, bytes: &[u8]) -> Option<()> {
    let len = u32::try_from(bytes.len()).ok()?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(bytes);
    Some(())
}

/// Why registering the public key on GitHub failed.
#[derive(Debug)]
pub(crate) enum RegistrationError {
    /// The `write:ssh_signing_key` device-flow authorization did not complete.
    Authorization(String),
    /// GitHub answered with a status other than `201 Created`.
    Refused { status: u16, message: String },
    /// The request never got an HTTP answer.
    Transport(String),
}

impl fmt::Display for RegistrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Authorization(reason) => {
                write!(f, "the key-registration authorization failed: {reason}")
            }
            Self::Refused { status, message } => {
                write!(
                    f,
                    "GitHub refused the signing key (HTTP {status}): {message}"
                )
            }
            Self::Transport(reason) => write!(f, "could not reach GitHub: {reason}"),
        }
    }
}

/// Registers a public key as a signing key on the user's GitHub account. The
/// seam the tests fake so no network is touched.
pub(crate) trait SigningKeyRegistrar {
    /// Register `public_key` under `title`. `Ok` only when GitHub confirmed the
    /// key was created.
    fn register(&mut self, public_key: &SshPublicKey, title: &str)
    -> Result<(), RegistrationError>;
}

/// Asks the user a yes/no question; the default answer is no.
pub(crate) trait Consent {
    /// `true` only on an explicit yes.
    fn confirm(&mut self, question: &str) -> bool;
}

/// What the setup step did.
#[derive(Debug, PartialEq, Eq)]
enum SetupOutcome {
    /// A usable key is already configured; nothing was generated.
    AlreadyConfigured(PrivateKeyPath),
    /// `IPE_PUBLISH_SIGNING_KEY` is set but unusable; nothing was generated,
    /// because a stored key would be shadowed by the variable anyway.
    EnvUnusable,
    /// The user declined; nothing was generated.
    Declined,
    /// A key was generated, registered on GitHub, and stored at this path.
    Registered(PrivateKeyPath),
}

impl SetupOutcome {
    fn message(&self) -> String {
        match self {
            Self::AlreadyConfigured(path) => {
                format!("Publish signs with {}.", path.as_path().display())
            }
            Self::EnvUnusable => format!(
                "{SIGNING_KEY_ENV} is set but names no readable key file, so publish will \
                 refuse. Point it at your signing key's private-key file, or unset it and run \
                 `ipe login --signing-key`."
            ),
            Self::Declined => "No signing key generated. `ipe package publish` needs one — \
                               run `ipe login --signing-key` any time, or set \
                               IPE_PUBLISH_SIGNING_KEY to an existing signing key."
                .to_owned(),
            Self::Registered(path) => format!(
                "Signing key registered on your GitHub account and stored at {}. \
                 `ipe package publish` signs with it; review it at {SIGNING_KEYS_SETTINGS}.",
                path.as_path().display()
            ),
        }
    }
}

/// Why the setup step failed. Every variant leaves no staged key behind.
#[derive(Debug)]
enum SetupError {
    /// Neither `XDG_CONFIG_HOME` nor `HOME` is set.
    NoConfigDir,
    /// Something already occupies a key file name.
    Occupied(PathBuf),
    /// The OS CSPRNG failed.
    KeyGeneration,
    /// A filesystem step before registration failed.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// Registration failed; the local key was removed.
    Registration(RegistrationError),
    /// The key IS registered on GitHub but could not be moved into place locally.
    Commit {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl fmt::Display for SetupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoConfigDir => f.write_str(
                "could not determine a config directory for the signing key (set HOME or \
                 XDG_CONFIG_HOME)",
            ),
            Self::Occupied(path) => write!(
                f,
                "{} already exists but is not a usable signing key — move it aside and run \
                 `ipe login --signing-key` again",
                path.display()
            ),
            Self::KeyGeneration => f.write_str(
                "the OS random-number generator failed, so no signing key was generated",
            ),
            Self::Io { path, source } => write!(
                f,
                "could not write {}: {source} — no signing key was registered",
                path.display()
            ),
            Self::Registration(e) => write!(
                f,
                "{e} — no signing key was stored locally; run `ipe login --signing-key` to retry"
            ),
            Self::Commit { path, source } => write!(
                f,
                "the signing key was registered on GitHub, but could not be stored at {}: \
                 {source}. The local copy was removed; delete the key titled \"{KEY_TITLE}\" \
                 at {SIGNING_KEYS_SETTINGS} and run `ipe login --signing-key` again",
                path.display()
            ),
        }
    }
}

/// The final names of the stored keypair.
struct KeyFiles {
    private: PathBuf,
    public: PathBuf,
}

impl KeyFiles {
    fn in_dir(dir: &Path) -> Self {
        Self {
            private: dir.join(PRIVATE_KEY_FILE),
            public: dir.join(PUBLIC_KEY_FILE),
        }
    }

    /// Refuse when anything (file, symlink, directory) already holds either name.
    fn ensure_free(&self) -> Result<(), SetupError> {
        for path in [&self.private, &self.public] {
            let absent = matches!(
                std::fs::symlink_metadata(path),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound
            );
            if !absent {
                return Err(SetupError::Occupied(path.clone()));
            }
        }
        Ok(())
    }
}

/// Both halves written under fresh temporary names. Dropping it removes the
/// temporary names — so any early return before [`Self::commit`] leaves nothing,
/// and after the commit only the final names remain.
struct StagedKeyPair {
    private_tmp: PathBuf,
    public_tmp: PathBuf,
}

impl StagedKeyPair {
    /// Write `pair` beside `files` under unique temporary names: the private
    /// half exclusively created with mode `0600`, the public half `0644`.
    fn write(files: &KeyFiles, pair: &GeneratedKeyPair) -> Result<Self, SetupError> {
        let mut suffix = [0u8; 8];
        getrandom::fill(&mut suffix).map_err(|_| SetupError::KeyGeneration)?;
        let suffix = format!("{}.{}", std::process::id(), hex::encode(suffix));
        let staged = Self {
            private_tmp: temp_name(&files.private, &suffix),
            public_tmp: temp_name(&files.public, &suffix),
        };
        write_new_file(&staged.private_tmp, 0o600, pair.private_pem.as_bytes())?;
        write_new_file(
            &staged.public_tmp,
            0o644,
            format!("{}\n", pair.public.as_str()).as_bytes(),
        )?;
        Ok(staged)
    }

    /// Link both halves into their final names. The private key is linked last:
    /// it is the name publish looks for, so it appears only once the public half
    /// is in place. `hard_link` refuses an existing name, so a file that appeared
    /// meanwhile is never overwritten.
    fn commit(self, files: &KeyFiles) -> Result<PrivateKeyPath, SetupError> {
        std::fs::hard_link(&self.public_tmp, &files.public).map_err(|source| {
            SetupError::Commit {
                path: files.public.clone(),
                source,
            }
        })?;
        if let Err(source) = std::fs::hard_link(&self.private_tmp, &files.private) {
            let _ = std::fs::remove_file(&files.public);
            return Err(SetupError::Commit {
                path: files.private.clone(),
                source,
            });
        }
        Ok(PrivateKeyPath(files.private.clone()))
    }
}

impl Drop for StagedKeyPair {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.private_tmp);
        let _ = std::fs::remove_file(&self.public_tmp);
    }
}

/// `.<name>.<suffix>.tmp` beside `path`.
fn temp_name(path: &Path, suffix: &str) -> PathBuf {
    let name = path
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or(PRIVATE_KEY_FILE);
    path.with_file_name(format!(".{name}.{suffix}.tmp"))
}

/// Create `path` exclusively (an existing name or symlink is refused, never
/// followed) with `mode` set at creation, then write and sync `contents`. A
/// partially written file is removed.
fn write_new_file(path: &Path, mode: u32, contents: &[u8]) -> Result<(), SetupError> {
    let io_error = |source| SetupError::Io {
        path: path.to_path_buf(),
        source,
    };
    let mut file = exclusive_create(path, mode).map_err(io_error)?;
    let written = file.write_all(contents).and_then(|()| file.sync_all());
    drop(file);
    written.map_err(|source| {
        let _ = std::fs::remove_file(path);
        io_error(source)
    })
}

#[cfg(unix)]
fn exclusive_create(path: &Path, mode: u32) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt as _;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)
}

#[cfg(not(unix))]
fn exclusive_create(path: &Path, _mode: u32) -> std::io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

/// Create the config dir (owner-only when newly created on Unix).
fn create_config_dir(dir: &Path) -> Result<(), SetupError> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(dir).map_err(|source| SetupError::Io {
        path: dir.to_path_buf(),
        source,
    })
}

/// The consent question: what will be generated, where it is stored, and the
/// extra scope the one-shot registration authorization asks for.
fn consent_question(files: &KeyFiles) -> String {
    format!(
        "No commit-signing key is configured. `ipe package publish` signs the index\n\
         commit with an SSH key registered on your GitHub account as a signing key.\n\
         \n\
         ipe can generate a dedicated ed25519 key (no passphrase, mode 0600) at\n\
         \x20 {}\n\
         and register its public half as a signing key on your account. That needs a\n\
         second, one-time GitHub authorization with the `write:ssh_signing_key` scope;\n\
         its token is used for this single request and never stored.\n\
         \n\
         Generate and register a signing key now?",
        files.private.display()
    )
}

/// The setup step: skip when a key is configured, otherwise ask, generate,
/// stage, register, and commit — removing the staged key on any failure.
fn set_up<C: Consent, R: SigningKeyRegistrar>(
    env_value: Option<&OsStr>,
    config_dir: Option<&Path>,
    consent: &mut C,
    registrar: &mut R,
) -> Result<SetupOutcome, SetupError> {
    let dir = match lookup(env_value, config_dir) {
        KeyLookup::Env(path) | KeyLookup::Stored(path) => {
            return Ok(SetupOutcome::AlreadyConfigured(path));
        }
        KeyLookup::EnvUnusable => return Ok(SetupOutcome::EnvUnusable),
        KeyLookup::Missing => config_dir.ok_or(SetupError::NoConfigDir)?,
    };
    let files = KeyFiles::in_dir(dir);
    files.ensure_free()?;
    if !consent.confirm(&consent_question(&files)) {
        return Ok(SetupOutcome::Declined);
    }
    let pair = GeneratedKeyPair::generate().ok_or(SetupError::KeyGeneration)?;
    create_config_dir(dir)?;
    let staged = StagedKeyPair::write(&files, &pair)?;
    registrar
        .register(&pair.public, KEY_TITLE)
        .map_err(SetupError::Registration)?;
    staged.commit(&files).map(SetupOutcome::Registered)
}

/// Reads the answer from the terminal.
struct TerminalConsent;

impl Consent for TerminalConsent {
    fn confirm(&mut self, question: &str) -> bool {
        print!("{}", crate::style::gutter(&format!("\n{question} [y/N] ")));
        let _ = std::io::stdout().flush();
        crate::read_yes_no_default(false)
    }
}

/// Registers through the GitHub REST API after a one-shot device-flow grant.
struct GithubRegistrar;

impl SigningKeyRegistrar for GithubRegistrar {
    fn register(
        &mut self,
        public_key: &SshPublicKey,
        title: &str,
    ) -> Result<(), RegistrationError> {
        let token = crate::login::authorize_signing_key_registration()
            .map_err(|e| RegistrationError::Authorization(e.to_string()))?;
        post_signing_key(&token, public_key, title)
    }
}

/// `POST /user/ssh_signing_keys`. Only `201 Created` counts as registered. The
/// token travels in-process in the `Authorization` header — never on an argv —
/// and is dropped when this returns.
fn post_signing_key(
    token: &KeyRegistrationToken,
    public_key: &SshPublicKey,
    title: &str,
) -> Result<(), RegistrationError> {
    let body = serde_json::json!({ "title": title, "key": public_key.as_str() }).to_string();
    let authorization = Zeroizing::new(format!("Bearer {}", token.as_str()));
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(30))
        .build();
    let result = agent
        .post(SIGNING_KEYS_API)
        .set("User-Agent", "ipe-cli")
        .set("Accept", "application/vnd.github+json")
        .set("X-GitHub-Api-Version", "2022-11-28")
        .set("Content-Type", "application/json")
        .set("Authorization", &authorization)
        .send_string(&body);
    match result {
        Ok(response) if response.status() == 201 => Ok(()),
        Ok(response) => Err(RegistrationError::Refused {
            status: response.status(),
            message: github_message(response),
        }),
        Err(ureq::Error::Status(status, response)) => Err(RegistrationError::Refused {
            status,
            message: github_message(response),
        }),
        Err(ureq::Error::Transport(transport)) => {
            Err(RegistrationError::Transport(transport.to_string()))
        }
    }
}

/// GitHub's `message` field, stripped of control characters and capped, so a
/// hostile response cannot drive the terminal.
fn github_message(response: ureq::Response) -> String {
    let message = response
        .into_string()
        .ok()
        .and_then(|body| serde_json::from_str::<serde_json::Value>(&body).ok())
        .and_then(|json| {
            json.get("message")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_default();
    let cleaned: String = message
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_ECHOED_MESSAGE_CHARS)
        .collect();
    if cleaned.is_empty() {
        "no message".to_owned()
    } else {
        cleaned
    }
}

fn is_interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

/// Run the setup against the real terminal and GitHub, printing the outcome.
fn run_interactive(env_value: Option<&OsStr>, config_dir: Option<&Path>) -> Result<(), CliError> {
    let outcome = set_up(
        env_value,
        config_dir,
        &mut TerminalConsent,
        &mut GithubRegistrar,
    )
    .map_err(|e| CliError::Resolve(format!("ipe login: {e}")))?;
    print!(
        "{}",
        crate::style::frame(&crate::style::gutter(&outcome.message()))
    );
    Ok(())
}

/// After `ipe login` stored a token: offer signing-key setup when none is
/// configured. Without a terminal nothing is asked — only a hint is printed.
///
/// # Errors
/// [`CliError::Resolve`] when the user opted in and setup failed; no partial key
/// is left behind.
pub(crate) fn offer_after_login() -> Result<(), CliError> {
    let env_value = std::env::var_os(SIGNING_KEY_ENV);
    let config_dir = crate::login::config_dir();
    if is_interactive() {
        return run_interactive(env_value.as_deref(), config_dir.as_deref());
    }
    if lookup(env_value.as_deref(), config_dir.as_deref()) == KeyLookup::Missing {
        print!(
            "{}",
            crate::style::frame(&crate::style::gutter(
                "No commit-signing key is configured; `ipe package publish` needs one. Run \
                 `ipe login --signing-key` in a terminal to generate and register it."
            ))
        );
    }
    Ok(())
}

/// `ipe login --signing-key`: the setup step on its own.
///
/// # Errors
/// [`CliError::Resolve`] without an interactive terminal, or when setup failed;
/// no partial key is left behind.
pub(crate) fn run_setup_command() -> Result<(), CliError> {
    if !is_interactive() {
        return Err(CliError::Resolve(
            "ipe login: `--signing-key` asks for consent and a GitHub authorization, so it needs \
             an interactive terminal"
                .to_owned(),
        ));
    }
    run_interactive(
        std::env::var_os(SIGNING_KEY_ENV).as_deref(),
        crate::login::config_dir().as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 8032 §7.1 test 1 secret seed.
    const RFC8032_SEED: [u8; 32] = [
        0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec, 0x2c,
        0xc4, 0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03, 0x1c, 0xae,
        0x7f, 0x60,
    ];

    fn test_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ipe-signing-key-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create test dir");
        dir
    }

    fn dir_entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .expect("readdir")
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    struct Answer {
        yes: bool,
        asked: usize,
    }

    impl Consent for Answer {
        fn confirm(&mut self, _question: &str) -> bool {
            self.asked += 1;
            self.yes
        }
    }

    struct FakeRegistrar {
        refuse: bool,
        calls: usize,
        seen_key: Option<String>,
        staged_private_mode: Option<u32>,
        dir: PathBuf,
    }

    impl FakeRegistrar {
        fn new(dir: &Path, refuse: bool) -> Self {
            Self {
                refuse,
                calls: 0,
                seen_key: None,
                staged_private_mode: None,
                dir: dir.to_path_buf(),
            }
        }
    }

    impl SigningKeyRegistrar for FakeRegistrar {
        fn register(
            &mut self,
            public_key: &SshPublicKey,
            _title: &str,
        ) -> Result<(), RegistrationError> {
            self.calls += 1;
            self.seen_key = Some(public_key.as_str().to_owned());
            // Observe the staged private key's mode at the moment of registration.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                self.staged_private_mode = std::fs::read_dir(&self.dir)
                    .expect("readdir")
                    .filter_map(Result::ok)
                    .find(|e| {
                        let name = e.file_name().to_string_lossy().into_owned();
                        name.starts_with(".signing_key.")
                            && !name.starts_with(".signing_key.pub.")
                            && name.ends_with(".tmp")
                    })
                    .map(|e| e.metadata().expect("metadata").permissions().mode() & 0o777);
            }
            if self.refuse {
                Err(RegistrationError::Refused {
                    status: 403,
                    message: "Resource not accessible by integration".to_owned(),
                })
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn encodes_the_rfc8032_key_in_openssh_format() {
        let pair = GeneratedKeyPair::from_seed(&RFC8032_SEED, 0x0102_0304).expect("encodes");
        assert_eq!(
            pair.public.as_str(),
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAINdamAGCsQq31Uv+08lkBzoO4XLz2qYjJa8CGmj3B1Ea ipe-publish"
        );
        assert_eq!(
            pair.private_pem.as_str(),
            "-----BEGIN OPENSSH PRIVATE KEY-----\n\
             b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\n\
             QyNTUxOQAAACDXWpgBgrEKt9VL/tPJZAc6DuFy89qmIyWvAhpo9wdRGgAAAJABAgMEAQID\n\
             BAAAAAtzc2gtZWQyNTUxOQAAACDXWpgBgrEKt9VL/tPJZAc6DuFy89qmIyWvAhpo9wdRGg\n\
             AAAECdYbGd7/1aYLqESvSS7CzEREnFaXsyaRlwO6wDHK5/YNdamAGCsQq31Uv+08lkBzoO\n\
             4XLz2qYjJa8CGmj3B1EaAAAAC2lwZS1wdWJsaXNoAQI=\n\
             -----END OPENSSH PRIVATE KEY-----\n"
        );
    }

    #[test]
    fn generated_keys_are_distinct() {
        let a = GeneratedKeyPair::generate().expect("csprng");
        let b = GeneratedKeyPair::generate().expect("csprng");
        assert_ne!(a.public.as_str(), b.public.as_str());
        assert!(
            a.public
                .as_str()
                .starts_with("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5")
        );
    }

    #[test]
    fn declined_prompt_generates_nothing() {
        let dir = test_dir("declined");
        let mut consent = Answer {
            yes: false,
            asked: 0,
        };
        let mut registrar = FakeRegistrar::new(&dir, false);
        let outcome =
            set_up(None, Some(dir.as_path()), &mut consent, &mut registrar).expect("declined");
        assert_eq!(outcome, SetupOutcome::Declined);
        assert_eq!(consent.asked, 1);
        assert_eq!(registrar.calls, 0, "nothing is registered after a decline");
        assert!(
            dir_entries(&dir).is_empty(),
            "no file is written after a decline"
        );
        assert_eq!(lookup(None, Some(dir.as_path())), KeyLookup::Missing);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn api_error_rolls_back_the_local_key() {
        let dir = test_dir("rollback");
        let mut consent = Answer {
            yes: true,
            asked: 0,
        };
        let mut registrar = FakeRegistrar::new(&dir, true);
        let result = set_up(None, Some(dir.as_path()), &mut consent, &mut registrar);
        assert!(matches!(
            result,
            Err(SetupError::Registration(RegistrationError::Refused {
                status: 403,
                ..
            }))
        ));
        assert_eq!(registrar.calls, 1);
        assert!(
            dir_entries(&dir).is_empty(),
            "a failed registration leaves no key file: {:?}",
            dir_entries(&dir)
        );
        assert_eq!(
            lookup(None, Some(dir.as_path())),
            KeyLookup::Missing,
            "publish finds no key after a failed registration"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn registered_key_is_stored_owner_only_and_found_by_publish() {
        let dir = test_dir("registered");
        let mut consent = Answer {
            yes: true,
            asked: 0,
        };
        let mut registrar = FakeRegistrar::new(&dir, false);
        let outcome =
            set_up(None, Some(dir.as_path()), &mut consent, &mut registrar).expect("registered");
        let private = dir.join(PRIVATE_KEY_FILE);
        assert_eq!(
            outcome,
            SetupOutcome::Registered(PrivateKeyPath(private.clone()))
        );
        assert_eq!(
            dir_entries(&dir),
            vec![PRIVATE_KEY_FILE.to_owned(), PUBLIC_KEY_FILE.to_owned()],
            "only the final names remain"
        );
        let stored_pub = std::fs::read_to_string(dir.join(PUBLIC_KEY_FILE)).expect("pub");
        assert_eq!(
            Some(stored_pub.trim_end().to_owned()),
            registrar.seen_key,
            "the stored public key is the one registered"
        );
        let pem = std::fs::read_to_string(&private).expect("private");
        assert!(pem.starts_with("-----BEGIN OPENSSH PRIVATE KEY-----\n"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&private)
                .expect("meta")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "private key must be 0600, got {mode:04o}");
            assert_eq!(
                registrar.staged_private_mode,
                Some(0o600),
                "the staged private key is 0600 from creation"
            );
        }
        assert_eq!(
            lookup(None, Some(dir.as_path())),
            KeyLookup::Stored(PrivateKeyPath(private))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_configured_key_is_not_replaced() {
        let dir = test_dir("configured");
        std::fs::write(dir.join(PRIVATE_KEY_FILE), b"existing").expect("plant");
        let mut consent = Answer {
            yes: true,
            asked: 0,
        };
        let mut registrar = FakeRegistrar::new(&dir, false);
        let outcome =
            set_up(None, Some(dir.as_path()), &mut consent, &mut registrar).expect("skip");
        assert!(matches!(outcome, SetupOutcome::AlreadyConfigured(_)));
        assert_eq!(consent.asked, 0, "no prompt when a key is configured");
        assert_eq!(registrar.calls, 0);
        assert_eq!(
            std::fs::read(dir.join(PRIVATE_KEY_FILE)).expect("read"),
            b"existing"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_at_the_key_name_is_refused_before_registering() {
        let dir = test_dir("symlink");
        let target = dir.join("elsewhere");
        std::fs::write(&target, b"not ours").expect("plant target");
        std::os::unix::fs::symlink(&target, dir.join(PRIVATE_KEY_FILE)).expect("plant symlink");
        assert_eq!(
            lookup(None, Some(dir.as_path())),
            KeyLookup::Missing,
            "a symlinked stored key is not used"
        );
        let mut consent = Answer {
            yes: true,
            asked: 0,
        };
        let mut registrar = FakeRegistrar::new(&dir, false);
        let result = set_up(None, Some(dir.as_path()), &mut consent, &mut registrar);
        assert!(matches!(result, Err(SetupError::Occupied(_))));
        assert_eq!(consent.asked, 0);
        assert_eq!(
            registrar.calls, 0,
            "nothing registered when a name is occupied"
        );
        assert_eq!(std::fs::read(&target).expect("read"), b"not ours");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn env_override_wins_over_the_stored_key() {
        let dir = test_dir("env-wins");
        let stored = dir.join(PRIVATE_KEY_FILE);
        std::fs::write(&stored, b"stored").expect("plant stored");
        let explicit = dir.join("explicit_key");
        std::fs::write(&explicit, b"explicit").expect("plant explicit");

        assert_eq!(
            lookup(Some(explicit.as_os_str()), Some(dir.as_path())),
            KeyLookup::Env(PrivateKeyPath(explicit)),
            "a usable env key wins"
        );
        assert_eq!(
            lookup(
                Some(OsStr::new("/nonexistent/ipe/key")),
                Some(dir.as_path())
            ),
            KeyLookup::EnvUnusable,
            "an unusable env key refuses; it never falls back to the stored key"
        );
        assert_eq!(
            lookup(Some(OsStr::new("  ")), Some(dir.as_path())),
            KeyLookup::Stored(PrivateKeyPath(stored.clone())),
            "a blank env value counts as unset"
        );
        assert_eq!(
            lookup(None, Some(dir.as_path())),
            KeyLookup::Stored(PrivateKeyPath(stored))
        );
        assert_eq!(lookup(None, None), KeyLookup::Missing);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_set_env_var_suppresses_the_offer() {
        let dir = test_dir("env-set");
        let mut consent = Answer {
            yes: true,
            asked: 0,
        };
        let mut registrar = FakeRegistrar::new(&dir, false);
        let outcome = set_up(
            Some(OsStr::new("/nonexistent/ipe/key")),
            Some(dir.as_path()),
            &mut consent,
            &mut registrar,
        )
        .expect("skip");
        assert_eq!(outcome, SetupOutcome::EnvUnusable);
        assert_eq!(consent.asked, 0);
        assert_eq!(registrar.calls, 0);
        assert!(dir_entries(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn registration_errors_never_carry_key_material() {
        let pair = GeneratedKeyPair::from_seed(&RFC8032_SEED, 1).expect("encodes");
        let rendered = SetupError::Registration(RegistrationError::Refused {
            status: 422,
            message: "key is already in use".to_owned(),
        })
        .to_string();
        assert!(!rendered.contains("PRIVATE KEY"));
        assert!(!rendered.contains(pair.private_pem.lines().nth(1).expect("body line")));
    }
}
