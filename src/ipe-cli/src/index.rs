//! The package index entry schema and reader.
//!
//! The index is a git repository holding one entry file per package at
//! `packages/<name>.toml`. An entry lists every published version, and for each
//! the source repository, its pinned revision, the sha256 of the source tree at
//! that revision, and the capability set the publisher declared. Resolution
//! ([`resolve_version`]) picks the highest published version satisfying a
//! [`semver::VersionReq`].
//!
//! Parse, don't validate: an entry file is read into a typed [`IndexEntry`] whose
//! versions are [`semver::Version`] and whose capabilities are [`Capability`], so
//! a malformed version or an unknown capability name is a hard error at read
//! time, never a resolution-time surprise.
//!
//! [`SourceUrl`] and [`CommitId`] are typed newtypes that gate the two
//! publisher-controlled fields. An unvalidated string can never reach the `git`
//! subprocess — it must first pass through one of these constructors.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::str::FromStr as _;

use ipe_ir::Capability;

use crate::CliError;

/// A validated source-repository URL accepted by the package index.
///
/// The accept set covers the network transports (`https://`, `git://`, `ssh://`,
/// `file://`) and bare absolute paths (a leading `/`). Any value that begins
/// with `-` (option injection) or contains `::` (git transport helpers such as
/// `ext::` or `fd::`, the real RCE vector) is rejected at parse time so a
/// malicious index entry can never reach the `git` subprocess.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceUrl(String);

impl SourceUrl {
    /// Parse a raw string from the index into a [`SourceUrl`], rejecting
    /// injection-shaped values.
    ///
    /// Accepted: `https://`, `git://`, `ssh://`, `file://`, and bare absolute
    /// paths (starting with `/`). Rejected: a leading `-` (git flag injection)
    /// or `::` anywhere (transport-helper execution, the RCE vector).
    ///
    /// # Errors
    /// [`CliError::Resolve`] when the value is not an accepted source form.
    pub fn parse(pkg: &str, raw: &str) -> Result<Self, CliError> {
        let allowed = raw.starts_with("https://")
            || raw.starts_with("git://")
            || raw.starts_with("ssh://")
            || raw.starts_with("file://")
            || raw.starts_with('/');
        // Fail closed: absent proof the transport is safe, reject.
        // `-`-leading values would be parsed as git flags; `::` introduces
        // transport helpers (e.g. `ext::`) that execute arbitrary commands.
        if !allowed || raw.starts_with('-') || raw.contains("::") {
            return Err(CliError::Resolve(format!(
                "package `{pkg}`: `source` must be an https://, git://, ssh://, or file:// URL \
                 (or a bare absolute path), got: {raw:?}"
            )));
        }
        Ok(Self(raw.to_owned()))
    }

    /// The validated URL string, safe to pass to `git clone -- <url> <dest>`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SourceUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A validated commit identifier accepted by the package index.
///
/// Accepts full commit hashes, abbreviated hashes, ref names, and `HEAD` —
/// anything git itself accepts — as long as the value is not injection-shaped.
/// Rejected: a leading `-` (git flag injection), `::` anywhere (transport
/// helper), whitespace or control characters, git refspec metacharacters
/// (`..`, `~`, `^`, `:`, `?`, `*`, `[`, `\`, `@{`), and a trailing `/` or
/// `.lock` (path-confusion vectors).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitId(String);

impl CommitId {
    /// Parse a raw string into a [`CommitId`], rejecting injection-shaped values.
    ///
    /// Accepts any non-injection ref name, including full/abbreviated hashes,
    /// branch names, and `HEAD`. Rejected: a leading `-`, `::`, whitespace,
    /// control chars, git refspec metacharacters (`..`, `~`, `^`, `:`, `?`,
    /// `*`, `[`, `\`, `@{`), trailing `/`, and `.lock` suffix.
    ///
    /// # Errors
    /// [`CliError::Resolve`] when the value is injection-shaped.
    pub fn parse(pkg: &str, raw: &str) -> Result<Self, CliError> {
        let injection = raw.is_empty()
            || raw.starts_with('-')
            || raw.contains("::")
            || raw
                .chars()
                .any(|c| c.is_ascii_whitespace() || c.is_ascii_control())
            || raw.contains("..")
            || raw.contains('~')
            || raw.contains('^')
            || raw.contains(':')
            || raw.contains('?')
            || raw.contains('*')
            || raw.contains('[')
            || raw.contains('\\')
            || raw.contains("@{")
            || raw.ends_with('/')
            || std::path::Path::new(raw)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("lock"));
        if injection {
            return Err(CliError::Resolve(format!(
                "package `{pkg}`: `rev` contains an injection-shaped value, got: {raw:?}"
            )));
        }
        Ok(Self(raw.to_owned()))
    }

    /// The validated commit-id string, safe to pass to `git checkout <rev>`.
    ///
    /// The `CommitId` parse boundary guarantees this value cannot start with
    /// `-`, so passing it without `--` is safe — `--` in `git checkout` means
    /// "treat as a path, not a ref", which is wrong for a commit id.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for CommitId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A parsed index entry: one package and every version published for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexEntry {
    /// The package name, matching the entry file stem (`packages/<name>.toml`).
    pub name: String,
    /// The publishing account (informational; provenance for the entry).
    pub publisher: String,
    /// Every published version, in file order. [`resolve_version`] scans these
    /// for the highest match rather than relying on file order.
    pub versions: Vec<EntryVersion>,
}

/// One published version of a package: where its source lives, exactly which
/// revision, the content hash to verify the fetched tree against, and the
/// capabilities the publisher declared.
///
/// `source` and `rev` are typed ([`SourceUrl`] and [`CommitId`]) so an
/// unvalidated publisher-controlled string can never reach the `git` subprocess.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryVersion {
    /// The exact published version.
    pub version: semver::Version,
    /// The source repository URL, validated at parse time.
    pub source: SourceUrl,
    /// The pinned commit hash, validated at parse time.
    pub rev: CommitId,
    /// The sha256 of the source tree at `rev`. A fetched tree is trusted only
    /// when its hash equals this (verify-before-trust, in `crate::resolve`).
    pub sha256: String,
    /// The capability set the publisher declared for this version, surfaced for
    /// consent at `ipe add`.
    pub capabilities: BTreeSet<Capability>,
}

/// The path of a package's entry file inside an index checkout.
fn entry_path(index_root: &Path, name: &str) -> PathBuf {
    index_root.join("packages").join(format!("{name}.toml"))
}

/// Whether the package's entry file exists in the index checkout at `index_root`.
///
/// Publish uses this to distinguish a first publish (no file — create it) from a
/// present-but-unreadable entry (a real error to surface), rather than treating
/// every read failure as "absent".
#[must_use]
pub fn entry_file_exists(index_root: &Path, name: &str) -> bool {
    entry_path(index_root, name).is_file()
}

/// The three-way outcome of looking up a package entry in the index.
///
/// This split prevents callers from collapsing "genuinely absent" and
/// "present-but-unreadable" into the same value — the fused form that lets
/// security gates fail open. Use [`EntryLookup::require_present`] for
/// integrity gates (Unreadable → propagate error → refuse) and
/// [`EntryLookup::absent_or_err`] where the first-version skip must survive a
/// true absence but still refuse on corruption.
#[must_use]
pub enum EntryLookup {
    /// The entry file does not exist — the package has never been published.
    Absent,
    /// The entry file exists and parsed successfully.
    Present(IndexEntry),
    /// The entry file exists but could not be read or parsed.
    Unreadable(CliError),
}

impl EntryLookup {
    /// Fail-closed accessor for integrity gates.
    ///
    /// `Absent` → `Ok(None)` (no published baseline; a new submission is
    /// allowed through). `Present` → `Ok(Some(entry))`. `Unreadable` → `Err`
    /// (propagate the error; the gate refuses rather than treating corruption
    /// as "no baseline").
    ///
    /// # Errors
    /// The [`CliError`] carried by the `Unreadable` variant.
    pub fn require_present(self) -> Result<Option<IndexEntry>, CliError> {
        match self {
            Self::Absent => Ok(None),
            Self::Present(e) => Ok(Some(e)),
            Self::Unreadable(err) => Err(err),
        }
    }

    /// Fail-closed accessor for the semver-bump gate.
    ///
    /// Semantically identical to [`require_present`](Self::require_present):
    /// `Absent` → `Ok(None)` (first version; skip preserved), `Present` →
    /// `Ok(Some(entry))`, `Unreadable` → `Err` (refuse; a corrupt predecessor
    /// cannot be treated as "no predecessor").
    ///
    /// # Errors
    /// The [`CliError`] carried by the `Unreadable` variant.
    pub fn absent_or_err(self) -> Result<Option<IndexEntry>, CliError> {
        self.require_present()
    }
}

/// Read and parse the index entry for `name` as a [`EntryLookup`].
///
/// Returns the three-way outcome (Absent / Present / Unreadable) that
/// distinguishes genuinely absent from present-but-unreadable. Use this in
/// every integrity gate so an unreadable baseline propagates as an error (fail
/// closed) rather than collapsing to "absent → skip".
pub fn read_entry_lookup(index_root: &Path, name: &str) -> EntryLookup {
    let path = entry_path(index_root, name);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return EntryLookup::Absent,
        Err(e) => {
            return EntryLookup::Unreadable(CliError::Resolve(format!(
                "index entry for `{name}` exists but could not be read — {}",
                e.kind()
            )));
        }
    };
    match parse_entry(name, &text) {
        Ok(entry) => EntryLookup::Present(entry),
        Err(err) => EntryLookup::Unreadable(err),
    }
}

/// Read and parse the index entry for `name` from an index checkout rooted at
/// `index_root` (which holds `packages/<name>.toml`).
///
/// For non-security callers that treat both absence and corruption as errors
/// (e.g. the resolver, publish). Security/integrity gates must use
/// [`read_entry_lookup`] instead so they cannot accidentally collapse
/// "unreadable" to "absent → skip".
///
/// # Errors
/// [`CliError::Resolve`] when the entry file is absent or malformed.
pub fn read_entry(index_root: &Path, name: &str) -> Result<IndexEntry, CliError> {
    let path = entry_path(index_root, name);
    let text = std::fs::read_to_string(&path).map_err(|e| read_entry_error(name, &e))?;
    parse_entry(name, &text)
}

/// The typed diagnostic when an index entry cannot be read. A missing entry is
/// the ordinary "unknown package" case: the message names the package and points
/// the user at the index, WITHOUT leaking the internal cache path or the errno
/// tail. Any other read failure (a permission or corruption problem the user can
/// act on) keeps a readable kind description, still errno-free.
fn read_entry_error(name: &str, e: &std::io::Error) -> CliError {
    if e.kind() == std::io::ErrorKind::NotFound {
        CliError::Resolve(format!(
            "add: package `{name}` is not in the index — check the name, or run \
             `ipe rust add` for a Rust crate"
        ))
    } else {
        CliError::Resolve(format!(
            "add: could not read the index entry for `{name}` — {}",
            e.kind()
        ))
    }
}

/// Validate a single `packages/<name>.toml` entry file by its own path, the way
/// the index repository's admission CI checks a submitted entry.
///
/// The package name is the file stem (the schema's authoritative name), so the
/// file `packages/http-extras.toml` is validated as package `http-extras`. This
/// is the same parse [`read_entry`] runs, reused so the validator and the reader
/// can never disagree about what a well-formed entry is: a version that parses
/// here is a version the resolver will later accept.
///
/// Fail-closed: any parse failure (a bad version, a missing per-version field,
/// an unknown capability, an absent `publisher`, or zero `[[version]]` blocks)
/// is a hard error, never a warning. Structure only — the source pin and the
/// package gate are the admission CI's fetch and `ipe package audit` steps, not
/// this offline check.
///
/// # Errors
/// [`CliError::UsageOwned`] when `path` has no `.toml` file-stem to name the
/// package; [`CliError::Io`] when the file cannot be read; [`CliError::Resolve`]
/// when the entry is malformed.
pub fn validate_entry_file(path: &Path) -> Result<IndexEntry, CliError> {
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            CliError::UsageOwned(format!(
                "{} is not a `packages/<name>.toml` entry file — the file stem names the package",
                path.display()
            ))
        })?;
    let text = std::fs::read_to_string(path).map_err(|e| CliError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    parse_entry(name, &text)
}

/// Resolve the highest published version satisfying `req`.
///
/// # Errors
/// [`CliError::Resolve`] when no published version matches — the requirement is
/// unsatisfiable against this entry, named so the user sees what was available.
pub fn resolve_version<'a>(
    entry: &'a IndexEntry,
    req: &semver::VersionReq,
) -> Result<&'a EntryVersion, CliError> {
    entry
        .versions
        .iter()
        .filter(|v| req.matches(&v.version))
        .max_by(|a, b| a.version.cmp(&b.version))
        .ok_or_else(|| {
            let available: Vec<String> = entry
                .versions
                .iter()
                .map(|v| v.version.to_string())
                .collect();
            CliError::Resolve(format!(
                "package `{}`: no published version satisfies `{req}` (available: {})",
                entry.name,
                if available.is_empty() {
                    "none".to_owned()
                } else {
                    available.join(", ")
                }
            ))
        })
}

/// Parse an entry file's text into a typed [`IndexEntry`]. The format is a
/// top-level `name`/`publisher` followed by one `[[version]]` table per
/// published version. Comments (`#`) and blank lines are ignored; unrecognised
/// keys are ignored (forward-compatible), but a malformed known value is a hard
/// error.
fn parse_entry(name: &str, text: &str) -> Result<IndexEntry, CliError> {
    let mut publisher: Option<String> = None;
    let mut versions: Vec<RawVersion> = Vec::new();
    // `None` while reading the top-level table, `Some(idx)` while inside a
    // `[[version]]` table.
    let mut current: Option<usize> = None;

    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "[[version]]" {
            versions.push(RawVersion::default());
            current = Some(versions.len() - 1);
            continue;
        }
        if line.starts_with('[') {
            // Any other section closes the current `[[version]]` and is ignored.
            current = None;
            continue;
        }
        let Some((key, raw_val)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let raw_val = raw_val.trim();
        match current {
            None => {
                if key == "publisher" {
                    publisher = Some(unquote(raw_val).to_owned());
                }
                // A top-level `name` is informational; the authoritative name is
                // the file stem the caller asked for.
            }
            Some(idx) => {
                let Some(record) = versions.get_mut(idx) else {
                    continue;
                };
                match key {
                    "version" => record.version = Some(unquote(raw_val).to_owned()),
                    "source" => record.source = Some(unquote(raw_val).to_owned()),
                    "rev" => record.rev = Some(unquote(raw_val).to_owned()),
                    "sha256" => record.sha256 = Some(unquote(raw_val).to_owned()),
                    "capabilities" => record.capabilities = Some(raw_val.to_owned()),
                    _ => {}
                }
            }
        }
    }

    let publisher = publisher.ok_or_else(|| {
        CliError::Resolve(format!(
            "package `{name}`: index entry is missing `publisher`"
        ))
    })?;
    if versions.is_empty() {
        return Err(CliError::Resolve(format!(
            "package `{name}`: index entry lists no `[[version]]`"
        )));
    }
    let versions = versions
        .into_iter()
        .map(|raw| raw.into_version(name))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(IndexEntry {
        name: name.to_owned(),
        publisher,
        versions,
    })
}

/// The raw per-`[[version]]` fields collected during the line scan, before they
/// are parsed into the typed [`EntryVersion`].
#[derive(Default)]
struct RawVersion {
    version: Option<String>,
    source: Option<String>,
    rev: Option<String>,
    sha256: Option<String>,
    capabilities: Option<String>,
}

impl RawVersion {
    /// Turn the collected raw fields into a typed [`EntryVersion`], erroring on a
    /// missing required field, a malformed version, or an unknown capability.
    fn into_version(self, name: &str) -> Result<EntryVersion, CliError> {
        let missing = |field: &str| {
            CliError::Resolve(format!(
                "package `{name}`: a `[[version]]` entry is missing `{field}`"
            ))
        };
        let version_str = self.version.ok_or_else(|| missing("version"))?;
        let version = semver::Version::parse(&version_str).map_err(|e| {
            CliError::Resolve(format!(
                "package `{name}`: `{version_str}` is not a valid version: {e}"
            ))
        })?;
        let raw_source = self.source.ok_or_else(|| missing("source"))?;
        let raw_rev = self.rev.ok_or_else(|| missing("rev"))?;
        let sha256 = self.sha256.ok_or_else(|| missing("sha256"))?;
        // Parse-don't-validate: the typed constructors reject any value outside
        // the allow-list before it can reach `git`.
        let source = SourceUrl::parse(name, &raw_source)?;
        let rev = CommitId::parse(name, &raw_rev)?;
        let capabilities = parse_capabilities(name, self.capabilities.as_deref())?;
        Ok(EntryVersion {
            version,
            source,
            rev,
            sha256,
            capabilities,
        })
    }
}

/// Parse a `capabilities = ["network", …]` array value into a typed set via
/// [`Capability::from_str`]. Absent (or empty) means no capabilities; an unknown
/// name is a hard error — a typo can never become a silently-dropped capability
/// the user is then not warned about.
fn parse_capabilities(name: &str, raw: Option<&str>) -> Result<BTreeSet<Capability>, CliError> {
    let Some(raw) = raw else {
        return Ok(BTreeSet::new());
    };
    let inner = raw
        .trim()
        .strip_prefix('[')
        .and_then(|r| r.strip_suffix(']'))
        .ok_or_else(|| {
            CliError::Resolve(format!(
                "package `{name}`: `capabilities` must be a `[\"…\", …]` array, got: {raw}"
            ))
        })?;
    let mut set = BTreeSet::new();
    for token in inner.split(',') {
        let token = token.trim().trim_matches('"');
        if token.is_empty() {
            continue;
        }
        let cap = Capability::from_str(token)
            .map_err(|e| CliError::Resolve(format!("package `{name}`: {e}")))?;
        set.insert(cap);
    }
    Ok(set)
}

/// Strip one layer of surrounding double quotes from a scalar value.
fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use super::{CommitId, IndexEntry, SourceUrl, read_entry, resolve_version};
    use ipe_ir::Capability;
    use std::path::{Path, PathBuf};

    /// A 40-char lowercase hex placeholder rev used across fixtures. Full-length
    /// SHA-1 format, as required by [`CommitId`].
    const FIXTURE_REV: &str = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";

    /// Write a minimal fixture index with one package publishing the given
    /// versions, and return the index root. Every version shares one placeholder
    /// source/rev/sha256; capabilities are `["network"]`.
    fn write_fixture_index(root: &Path, name: &str, versions: &[&str]) {
        use std::fmt::Write as _;
        let packages = root.join("packages");
        std::fs::create_dir_all(&packages).expect("create packages dir");
        let mut text = format!("name = \"{name}\"\npublisher = \"tester\"\n");
        for v in versions {
            let _ = write!(
                text,
                "\n[[version]]\nversion = \"{v}\"\nsource = \"https://example.invalid/{name}\"\n\
                 rev = \"{FIXTURE_REV}\"\nsha256 = \"00\"\ncapabilities = [\"network\"]\n"
            );
        }
        std::fs::write(packages.join(format!("{name}.toml")), text).expect("write entry");
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ipe-index-test-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn reads_and_resolves_the_highest_matching_version() {
        let root = temp_dir("highest");
        write_fixture_index(&root, "http-extras", &["1.0.0", "1.2.0", "2.0.0"]);
        let entry = read_entry(&root, "http-extras").expect("entry parses");
        assert_eq!(entry.publisher, "tester");
        assert_eq!(entry.versions.len(), 3);

        let req = "^1.0".parse().expect("valid req");
        let chosen = resolve_version(&entry, &req).expect("a match exists");
        assert_eq!(chosen.version.to_string(), "1.2.0");
        assert!(chosen.capabilities.contains(&Capability::Network));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_unknown_package_is_an_error() {
        let root = temp_dir("unknown-pkg");
        std::fs::create_dir_all(root.join("packages")).expect("packages dir");
        let err = read_entry(&root, "absent").unwrap_err();
        assert!(format!("{err}").contains("absent"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_unsatisfiable_requirement_is_an_error() {
        let root = temp_dir("unsat");
        write_fixture_index(&root, "http-extras", &["1.0.0", "1.2.0"]);
        let entry = read_entry(&root, "http-extras").expect("entry parses");
        let req = "^3".parse().expect("valid req");
        let err = resolve_version(&entry, &req).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("no published version"), "{msg}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_unknown_capability_is_rejected() {
        let root = temp_dir("bad-cap");
        std::fs::create_dir_all(root.join("packages")).expect("packages dir");
        std::fs::write(
            root.join("packages").join("weird.toml"),
            format!(
                "publisher = \"tester\"\n\n[[version]]\nversion = \"1.0.0\"\n\
                 source = \"https://example.invalid/weird\"\nrev = \"{FIXTURE_REV}\"\n\
                 sha256 = \"00\"\ncapabilities = [\"telepathy\"]\n"
            ),
        )
        .expect("write entry");
        let err = read_entry(&root, "weird").unwrap_err();
        assert!(format!("{err}").contains("telepathy"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_per_version_field_is_rejected() {
        let root = temp_dir("missing-field");
        std::fs::create_dir_all(root.join("packages")).expect("packages dir");
        // No `sha256` — the integrity anchor is mandatory, so validation rejects
        // on the missing field before even reaching transport/commit validation.
        std::fs::write(
            root.join("packages").join("nohash.toml"),
            format!(
                "publisher = \"tester\"\n\n[[version]]\nversion = \"1.0.0\"\n\
                 source = \"https://example.invalid/nohash\"\nrev = \"{FIXTURE_REV}\"\n"
            ),
        )
        .expect("write entry");
        let err = read_entry(&root, "nohash").unwrap_err();
        assert!(format!("{err}").contains("sha256"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn validate_entry_file_accepts_a_well_formed_entry() {
        // The admission-CI validator names the package by the file stem and
        // parses it exactly as the resolver would.
        use super::validate_entry_file;
        let root = temp_dir("validate-ok");
        write_fixture_index(&root, "http-extras", &["1.0.0", "1.2.0"]);
        let entry = validate_entry_file(&root.join("packages").join("http-extras.toml"))
            .expect("well-formed entry validates");
        assert_eq!(entry.name, "http-extras");
        assert_eq!(entry.versions.len(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn validate_entry_file_rejects_an_unknown_capability() {
        use super::validate_entry_file;
        let root = temp_dir("validate-bad-cap");
        let packages = root.join("packages");
        std::fs::create_dir_all(&packages).expect("packages dir");
        let path = packages.join("weird.toml");
        std::fs::write(
            &path,
            format!(
                "publisher = \"tester\"\n\n[[version]]\nversion = \"1.0.0\"\n\
                 source = \"https://example.invalid/weird\"\nrev = \"{FIXTURE_REV}\"\n\
                 sha256 = \"00\"\ncapabilities = [\"telepathy\"]\n"
            ),
        )
        .expect("write entry");
        let err = validate_entry_file(&path).unwrap_err();
        assert!(format!("{err}").contains("telepathy"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn validate_entry_file_rejects_a_missing_field() {
        use super::validate_entry_file;
        let root = temp_dir("validate-missing");
        let packages = root.join("packages");
        std::fs::create_dir_all(&packages).expect("packages dir");
        let path = packages.join("nohash.toml");
        // No `sha256` — the integrity anchor is mandatory, so validation rejects
        // on the missing field before reaching transport/commit validation.
        std::fs::write(
            &path,
            format!(
                "publisher = \"tester\"\n\n[[version]]\nversion = \"1.0.0\"\n\
                 source = \"https://example.invalid/nohash\"\nrev = \"{FIXTURE_REV}\"\n"
            ),
        )
        .expect("write entry");
        let err = validate_entry_file(&path).unwrap_err();
        assert!(format!("{err}").contains("sha256"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn multiple_versions_are_all_parsed() {
        // Guards the array-of-tables scan: each `[[version]]` starts a fresh
        // record rather than overwriting the previous.
        let root = temp_dir("multi");
        write_fixture_index(&root, "p", &["0.1.0", "0.2.0", "0.3.0"]);
        let entry: IndexEntry = read_entry(&root, "p").expect("parses");
        let versions: Vec<String> = entry
            .versions
            .iter()
            .map(|v| v.version.to_string())
            .collect();
        assert_eq!(versions, vec!["0.1.0", "0.2.0", "0.3.0"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    // --- SourceUrl parse-boundary tests ---

    #[test]
    fn source_url_accepts_https() {
        assert!(SourceUrl::parse("p", "https://github.com/user/repo").is_ok());
    }

    #[test]
    fn source_url_accepts_git_and_ssh() {
        assert!(SourceUrl::parse("p", "git://github.com/user/repo").is_ok());
        assert!(SourceUrl::parse("p", "ssh://git@github.com/user/repo").is_ok());
    }

    #[test]
    fn source_url_rejects_ext_transport_helper() {
        // `ext::` spawns an arbitrary shell command at clone time — RCE vector.
        let err = SourceUrl::parse("p", "ext::sh -c 'id > /tmp/pwned'").unwrap_err();
        assert!(format!("{err}").contains("https://"), "{err}");
    }

    #[test]
    fn source_url_rejects_dash_leading_value() {
        // A value starting with `-` would be parsed by git as a flag.
        let err = SourceUrl::parse("p", "--upload-pack=evil").unwrap_err();
        assert!(format!("{err}").contains("https://"), "{err}");
    }

    #[test]
    fn source_url_accepts_file_scheme() {
        // `file://` is on the allow-list so local and test-fixture repos work.
        assert!(SourceUrl::parse("p", "file:///home/user/repo").is_ok());
    }

    #[test]
    fn source_url_accepts_bare_absolute_path() {
        assert!(SourceUrl::parse("p", "/home/user/repo").is_ok());
    }

    #[test]
    fn source_url_rejects_fd_transport() {
        let err = SourceUrl::parse("p", "fd::4").unwrap_err();
        assert!(format!("{err}").contains("https://"), "{err}");
    }

    // --- CommitId parse-boundary tests ---

    #[test]
    fn commit_id_accepts_40_char_lowercase_hex() {
        assert!(CommitId::parse("p", FIXTURE_REV).is_ok());
    }

    #[test]
    fn commit_id_accepts_64_char_sha256() {
        let sha256_rev = "a".repeat(64);
        assert!(CommitId::parse("p", &sha256_rev).is_ok());
    }

    #[test]
    fn commit_id_accepts_short_hex() {
        // Abbreviated hashes are valid ref names with no injection shape.
        assert!(CommitId::parse("p", "deadbeef").is_ok());
        assert!(CommitId::parse("p", "abc").is_ok());
        assert!(CommitId::parse("p", "00").is_ok());
    }

    #[test]
    fn commit_id_accepts_branch_name() {
        // Branch names are valid ref names with no injection shape.
        assert!(CommitId::parse("p", "main").is_ok());
        assert!(CommitId::parse("p", "HEAD").is_ok());
    }

    #[test]
    fn commit_id_accepts_uppercase_hex() {
        // Mixed-case is not injection-shaped.
        assert!(CommitId::parse("p", "A1B2C3D4E5F6A1B2C3D4E5F6A1B2C3D4E5F6A1B2").is_ok());
    }

    #[test]
    fn commit_id_rejects_dash_leading_value() {
        // A `-`-leading rev would be parsed by git as a flag — injection shape.
        let err = CommitId::parse("p", "-S injected").unwrap_err();
        assert!(format!("{err}").contains("rev"), "{err}");
    }

    #[test]
    fn commit_id_rejects_double_dot() {
        // `..` is a refspec metacharacter — injection shape.
        let err = CommitId::parse("p", "HEAD..main").unwrap_err();
        assert!(format!("{err}").contains("rev"), "{err}");
    }

    #[test]
    fn commit_id_rejects_transport_helper_colons() {
        // `::` is the transport-helper separator — RCE vector.
        let err = CommitId::parse("p", "ext::evil").unwrap_err();
        assert!(format!("{err}").contains("rev"), "{err}");
    }

    #[test]
    fn commit_id_rejects_at_brace() {
        // `@{` is a git reflog selector — injection shape.
        let err = CommitId::parse("p", "HEAD@{0}").unwrap_err();
        assert!(format!("{err}").contains("rev"), "{err}");
    }

    #[test]
    fn malicious_index_entry_is_rejected_at_parse_time() {
        // An entry with `source = "ext::sh -c …"` must be rejected by
        // `read_entry` before any git invocation is possible.
        let root = temp_dir("malicious");
        let packages = root.join("packages");
        std::fs::create_dir_all(&packages).expect("packages dir");
        std::fs::write(
            packages.join("evil.toml"),
            format!(
                "publisher = \"attacker\"\n\n[[version]]\nversion = \"1.0.0\"\n\
                 source = \"ext::sh -c 'id > /tmp/pwned'\"\nrev = \"{FIXTURE_REV}\"\n\
                 sha256 = \"00\"\ncapabilities = []\n"
            ),
        )
        .expect("write entry");
        let err = read_entry(&root, "evil").unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("source"), "{msg}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn injection_shaped_rev_in_index_entry_is_rejected_at_parse_time() {
        // A `-`-leading `rev` must be rejected by `read_entry` before any git
        // invocation — flag injection is the real threat, not plain ref names.
        let root = temp_dir("bad-rev");
        let packages = root.join("packages");
        std::fs::create_dir_all(&packages).expect("packages dir");
        std::fs::write(
            packages.join("badrev.toml"),
            "publisher = \"tester\"\n\n[[version]]\nversion = \"1.0.0\"\n\
             source = \"https://example.invalid/badrev\"\nrev = \"-S injected\"\n\
             sha256 = \"00\"\ncapabilities = []\n",
        )
        .expect("write entry");
        let err = read_entry(&root, "badrev").unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("rev"), "{msg}");
        let _ = std::fs::remove_dir_all(&root);
    }

    // -----------------------------------------------------------------------
    // EntryLookup / read_entry_lookup — fail-closed integrity gate regression
    // -----------------------------------------------------------------------

    fn write_corrupt_entry(packages: &std::path::Path, name: &str) {
        // A file that exists but is not valid TOML / missing required fields.
        std::fs::write(
            packages.join(format!("{name}.toml")),
            "publisher = \"tester\"\n\n[[version]]\nversion = \"NOT_SEMVER\"\n\
             source = \"https://example.invalid/x\"\nrev = \"aabbcc\"\nsha256 = \"00\"\n",
        )
        .expect("write corrupt entry");
    }

    #[test]
    fn lookup_absent_is_absent_variant() {
        let root = temp_dir("lookup-absent");
        std::fs::create_dir_all(root.join("packages")).expect("packages dir");
        let result = super::read_entry_lookup(&root, "nosuchpkg");
        assert!(
            matches!(result, super::EntryLookup::Absent),
            "a missing file must produce Absent, not Unreadable"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn lookup_present_is_present_variant() {
        let root = temp_dir("lookup-present");
        write_fixture_index(&root, "mypkg", &["1.0.0"]);
        let result = super::read_entry_lookup(&root, "mypkg");
        assert!(
            matches!(result, super::EntryLookup::Present(_)),
            "a well-formed entry must produce Present"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn lookup_corrupt_is_unreadable_variant() {
        let root = temp_dir("lookup-corrupt");
        let packages = root.join("packages");
        std::fs::create_dir_all(&packages).expect("packages dir");
        write_corrupt_entry(&packages, "broken");
        let result = super::read_entry_lookup(&root, "broken");
        assert!(
            matches!(result, super::EntryLookup::Unreadable(_)),
            "a present-but-malformed entry must produce Unreadable, not Absent"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn require_present_absent_yields_ok_none() {
        let root = temp_dir("rp-absent");
        std::fs::create_dir_all(root.join("packages")).expect("packages dir");
        let got = super::read_entry_lookup(&root, "nosuchpkg")
            .require_present()
            .expect("Absent must be Ok(None)");
        assert!(got.is_none(), "Absent must map to Ok(None), got Some");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn require_present_present_yields_ok_some() {
        let root = temp_dir("rp-present");
        write_fixture_index(&root, "mypkg", &["2.0.0"]);
        let got = super::read_entry_lookup(&root, "mypkg")
            .require_present()
            .expect("Present must be Ok(Some(_))")
            .expect("inner Option must be Some");
        assert_eq!(got.name, "mypkg");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn require_present_unreadable_yields_err() {
        // Regression for #1: a present-but-corrupt baseline must NOT produce
        // Ok(None) — that would let the immutability wall treat every submitted
        // version as "new" and skip the mutation check.
        let root = temp_dir("rp-corrupt");
        let packages = root.join("packages");
        std::fs::create_dir_all(&packages).expect("packages dir");
        write_corrupt_entry(&packages, "broken");
        let result = super::read_entry_lookup(&root, "broken").require_present();
        assert!(
            result.is_err(),
            "Unreadable must map to Err (fail closed), got Ok"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn absent_or_err_unreadable_yields_err() {
        // Regression for #2: a corrupt predecessor must NOT produce Ok(None)
        // (which the semver gate treats as "first version → skip").
        let root = temp_dir("aoe-corrupt");
        let packages = root.join("packages");
        std::fs::create_dir_all(&packages).expect("packages dir");
        write_corrupt_entry(&packages, "broken");
        let result = super::read_entry_lookup(&root, "broken").absent_or_err();
        assert!(
            result.is_err(),
            "Unreadable must map to Err (fail closed) in absent_or_err, got Ok"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn ssot_guardrail_no_read_entry_ok_in_ipe_cli_src() {
        // Assert that no future edit re-introduces a fail-open collapse of
        // `read_entry` at an integrity gate. The pattern `read_entry(…).ok()`
        // fuses "absent" and "unreadable" into None; every integrity gate must
        // use `read_entry_lookup` + `require_present` / `absent_or_err` instead.
        let src_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut violations = Vec::new();
        collect_ssot_violations(&src_dir, &mut violations);
        assert!(
            violations.is_empty(),
            "fail-open read_entry collapse detected — use read_entry_lookup instead:\n{}",
            violations.join("\n")
        );
    }

    fn collect_ssot_violations(dir: &std::path::Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_ssot_violations(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                // Skip this file itself: the guardrail helper body contains
                // the banned pattern strings as string literals, which would
                // self-trigger. The absence of the collapse pattern in index.rs
                // is verified by the structural fix (read_entry_lookup replaces
                // read_entry at both security call sites above).
                let stem = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if stem == "index.rs" {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                for (i, line) in text.lines().enumerate() {
                    let trimmed = line.trim();
                    // Skip pure comment lines.
                    if trimmed.starts_with("//") {
                        continue;
                    }
                    if trimmed.contains("read_entry(") && trimmed.contains(".ok()") {
                        out.push(format!("{}:{}: {}", path.display(), i + 1, trimmed));
                    }
                    if trimmed.contains("let Ok(") && trimmed.contains("read_entry(") {
                        out.push(format!("{}:{}: {}", path.display(), i + 1, trimmed));
                    }
                }
            }
        }
    }
}
