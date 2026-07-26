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

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::str::FromStr as _;

use ipe_ir::Capability;

use crate::CliError;

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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryVersion {
    /// The exact published version.
    pub version: semver::Version,
    /// The source repository URL, fetched with `git`.
    pub source: String,
    /// The exact revision (commit) fetched — a version is pinned, never a
    /// moving branch.
    pub rev: String,
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

/// Read and parse the index entry for `name` from an index checkout rooted at
/// `index_root` (which holds `packages/<name>.toml`).
///
/// # Errors
/// [`CliError::Resolve`] when the entry file is absent (the package is not in
/// the index) or malformed (a bad version, a missing per-version field, or an
/// unknown capability name).
pub fn read_entry(index_root: &Path, name: &str) -> Result<IndexEntry, CliError> {
    let path = entry_path(index_root, name);
    let text = std::fs::read_to_string(&path).map_err(|e| {
        CliError::Resolve(format!(
            "package `{name}` is not in the index (could not read {}: {e})",
            path.display()
        ))
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
        let source = self.source.ok_or_else(|| missing("source"))?;
        let rev = self.rev.ok_or_else(|| missing("rev"))?;
        let sha256 = self.sha256.ok_or_else(|| missing("sha256"))?;
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
    use super::{IndexEntry, read_entry, resolve_version};
    use ipe_ir::Capability;
    use std::path::{Path, PathBuf};

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
                 rev = \"deadbeef\"\nsha256 = \"00\"\ncapabilities = [\"network\"]\n"
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
            "publisher = \"tester\"\n\n[[version]]\nversion = \"1.0.0\"\n\
             source = \"https://example.invalid/weird\"\nrev = \"ab\"\nsha256 = \"00\"\n\
             capabilities = [\"telepathy\"]\n",
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
        // No `sha256` — the integrity anchor is mandatory.
        std::fs::write(
            root.join("packages").join("nohash.toml"),
            "publisher = \"tester\"\n\n[[version]]\nversion = \"1.0.0\"\n\
             source = \"https://example.invalid/nohash\"\nrev = \"ab\"\n",
        )
        .expect("write entry");
        let err = read_entry(&root, "nohash").unwrap_err();
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
}
