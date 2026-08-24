//! The `ipe.lock` lockfile: exact resolved dependencies, pinned for a
//! reproducible build.
//!
//! Each locked dependency records the resolved version, the source repository,
//! its exact revision, and the sha256 of the fetched source tree. A build reads
//! these pins rather than re-resolving through the index, so it is reproducible
//! even when the index is unreachable, and the pinned hash lets a later build
//! re-verify the source it fetches.
//!
//! Serialization is deterministic: packages are always written sorted by name,
//! so two runs that resolve the same set produce byte-identical lockfiles (a
//! stable diff, no spurious churn).

use std::path::{Path, PathBuf};

use crate::CliError;
use crate::index::PinnedRev;

/// The lockfile's filename at a project root.
const LOCKFILE_NAME: &str = "ipe.lock";

/// Whether a locked dependency came from the package index or a `{git=}`/`{path=}` escape.
///
/// Set once at parse/construction and carried through the lockfile round-trip so
/// callers never re-derive it from field shapes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DepKind {
    /// Resolved through the package index.
    Index,
    /// Resolved via a `{git=}` or `{path=}` escape, bypassing the index.
    Escape,
}

impl DepKind {
    /// The on-disk keyword for this variant.
    const fn as_str(&self) -> &'static str {
        match self {
            Self::Index => "index",
            Self::Escape => "escape",
        }
    }

    /// Parse the on-disk keyword, failing closed on an unrecognised value.
    fn from_str(raw: &str) -> Result<Self, CliError> {
        match raw {
            "index" => Ok(Self::Index),
            "escape" => Ok(Self::Escape),
            other => Err(CliError::Resolve(format!(
                "ipe.lock: unrecognised `kind` value {other:?} — re-run `ipe add` to regenerate"
            ))),
        }
    }
}

/// The revision recorded for a locked dependency. Git-sourced deps pin an
/// immutable 40-hex SHA; local-path deps have no git history to pin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LockedRev {
    /// An immutable 40-hex SHA from a git fetch, validated at parse time.
    Pinned(PinnedRev),
    /// A local-path dep: no git commit to pin, integrity is the sha256 alone.
    Local,
}

impl LockedRev {
    /// The on-disk string for this value.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Pinned(p) => p.as_str(),
            Self::Local => "local",
        }
    }

    /// Parse the stored rev string: `"local"` → `Local`; a 40-hex SHA →
    /// `Pinned`; anything else → fail closed.
    fn from_stored(pkg: &str, raw: &str) -> Result<Self, CliError> {
        if raw == "local" {
            return Ok(Self::Local);
        }
        PinnedRev::from_full_sha(pkg, raw).map(LockedRev::Pinned)
    }

    /// Return the inner [`PinnedRev`] if this is a `Pinned` rev.
    #[must_use]
    pub const fn as_pinned(&self) -> Option<&PinnedRev> {
        match self {
            Self::Pinned(p) => Some(p),
            Self::Local => None,
        }
    }
}

impl std::fmt::Display for LockedRev {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One locked dependency: the exact resolved version and the integrity anchors
/// (`source` + `rev` + `sha256`) a reproducible, re-verifiable build needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockedDep {
    /// The package name (the lockfile's sort key).
    pub name: String,
    /// The exact resolved version.
    pub version: semver::Version,
    /// The source repository the package was fetched from.
    pub source: String,
    /// The exact revision fetched: an immutable SHA for git-sourced deps, or
    /// `Local` for path deps whose integrity is captured by the sha256 alone.
    pub rev: LockedRev,
    /// The sha256 of the fetched source tree, verified on fetch and re-verifiable
    /// on a later build.
    pub sha256: String,
    /// Whether this dep came from the package index or a `{git=}`/`{path=}` escape.
    pub kind: DepKind,
}

/// The parsed `ipe.lock`: the set of locked dependencies, held sorted by name so
/// every write is deterministic.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lockfile {
    packages: Vec<LockedDep>,
}

impl Lockfile {
    /// Read the lockfile at `project_root/ipe.lock`. A missing lockfile is an
    /// empty lockfile (a project with no locked dependencies yet), not an error.
    ///
    /// # Errors
    /// [`CliError::Io`] if the file exists but cannot be read; [`CliError::Resolve`]
    /// if its content is malformed (a bad version, a missing field, or a non-SHA rev).
    pub fn read(project_root: &Path) -> Result<Self, CliError> {
        let path = Self::path(project_root);
        let text = match crate::io_bounded::read_to_string_capped(
            &path,
            crate::io_bounded::SMALL_FILE_READ_CAP,
        ) {
            Ok(text) => text,
            Err(CliError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(e) => return Err(e),
        };
        parse(&text)
    }

    /// Write the lockfile to `project_root/ipe.lock`, packages sorted by name.
    ///
    /// # Errors
    /// [`CliError::Io`] if the file cannot be written.
    pub fn write(&self, project_root: &Path) -> Result<(), CliError> {
        let path = Self::path(project_root);
        std::fs::write(&path, self.render()).map_err(|e| CliError::Io { path, source: e })
    }

    /// Insert `dep`, replacing any existing entry with the same name. The set
    /// stays sorted by name.
    pub fn upsert(&mut self, dep: LockedDep) {
        match self.packages.binary_search_by(|p| p.name.cmp(&dep.name)) {
            Ok(at) => {
                if let Some(slot) = self.packages.get_mut(at) {
                    *slot = dep;
                }
            }
            Err(at) => self.packages.insert(at, dep),
        }
    }

    /// Remove the entry named `name`, returning whether one was present.
    pub fn remove(&mut self, name: &str) -> bool {
        match self
            .packages
            .binary_search_by(|p| p.name.as_str().cmp(name))
        {
            Ok(at) => {
                self.packages.remove(at);
                true
            }
            Err(_) => false,
        }
    }

    /// The locked dependencies, sorted by name.
    #[must_use]
    pub fn packages(&self) -> &[LockedDep] {
        &self.packages
    }

    /// The lockfile path for a project root.
    fn path(project_root: &Path) -> PathBuf {
        project_root.join(LOCKFILE_NAME)
    }

    /// Render the lockfile as deterministic TOML: a `[[package]]` table per
    /// dependency, in name order.
    fn render(&self) -> String {
        use std::fmt::Write as _;
        // Rendered from an already-sorted invariant; sort defensively so a
        // hand-constructed `Lockfile` still writes deterministically.
        let mut packages = self.packages.clone();
        packages.sort_by(|a, b| a.name.cmp(&b.name));
        let mut out = String::from(
            "# ipe.lock — resolved dependencies, pinned for a reproducible build.\n\
             # Generated by `ipe add`; do not edit by hand.\n",
        );
        for dep in &packages {
            let _ = write!(
                out,
                "\n[[package]]\nname = \"{}\"\nversion = \"{}\"\nsource = \"{}\"\n\
                 rev = \"{}\"\nsha256 = \"{}\"\nkind = \"{}\"\n",
                dep.name,
                dep.version,
                dep.source,
                dep.rev.as_str(),
                dep.sha256,
                dep.kind.as_str(),
            );
        }
        out
    }
}

/// Parse `ipe.lock` text into a [`Lockfile`], sorting packages by name so the
/// in-memory invariant holds regardless of file order.
fn parse(text: &str) -> Result<Lockfile, CliError> {
    let mut packages: Vec<LockedDep> = Vec::new();
    let mut current: Option<RawLocked> = None;

    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "[[package]]" {
            if let Some(raw) = current.take() {
                packages.push(raw.into_dep()?);
            }
            current = Some(RawLocked::default());
            continue;
        }
        if line.starts_with('[') {
            if let Some(raw) = current.take() {
                packages.push(raw.into_dep()?);
            }
            continue;
        }
        let Some((key, raw_val)) = line.split_once('=') else {
            continue;
        };
        let Some(record) = current.as_mut() else {
            continue;
        };
        let value = unquote(raw_val.trim()).to_owned();
        match key.trim() {
            "name" => record.name = Some(value),
            "version" => record.version = Some(value),
            "source" => record.source = Some(value),
            "rev" => record.rev = Some(value),
            "sha256" => record.sha256 = Some(value),
            "kind" => record.kind = Some(value),
            _ => {}
        }
    }
    if let Some(raw) = current.take() {
        packages.push(raw.into_dep()?);
    }

    packages.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Lockfile { packages })
}

/// The raw per-`[[package]]` fields collected during the line scan.
#[derive(Default)]
struct RawLocked {
    name: Option<String>,
    version: Option<String>,
    source: Option<String>,
    rev: Option<String>,
    sha256: Option<String>,
    kind: Option<String>,
}

impl RawLocked {
    /// Turn the collected fields into a typed [`LockedDep`], erroring on a
    /// missing field or a malformed value.
    ///
    /// The `rev` field is parsed through [`LockedRev::from_stored`]: `"local"`
    /// (path deps) is accepted as-is; any other value must be a 40-hex SHA or
    /// the parse fails closed — a legacy `"HEAD"` or branch name in the lockfile
    /// is rejected here rather than silently accepted as a moving ref.
    ///
    /// The `kind` field is optional for backward-compatibility with lockfiles
    /// written before this field existed: when absent, it is inferred from
    /// `version` (escape deps always carry `version = "0.0.0"`). When present,
    /// it is parsed and trusted directly so a pathological index dep at `0.0.0`
    /// is correctly classified as `Index`.
    fn into_dep(self) -> Result<LockedDep, CliError> {
        let missing = |field: &str| {
            CliError::Resolve(format!("ipe.lock: a `[[package]]` is missing `{field}`"))
        };
        let name = self.name.ok_or_else(|| missing("name"))?;
        let version_str = self.version.ok_or_else(|| missing("version"))?;
        let version = semver::Version::parse(&version_str).map_err(|e| {
            CliError::Resolve(format!(
                "ipe.lock: `{version_str}` is not a valid version: {e}"
            ))
        })?;
        let raw_rev = self.rev.ok_or_else(|| missing("rev"))?;
        // Fail closed: a non-SHA rev (e.g. "HEAD" from a legacy lockfile) is
        // rejected here — only "local" (path deps) or a 40-hex SHA are accepted.
        let rev = LockedRev::from_stored(&name, &raw_rev)?;
        let kind = match self.kind {
            Some(raw_kind) => DepKind::from_str(&raw_kind)?,
            // Backward-compat: infer from version for lockfiles written before
            // the `kind` field existed. Escape deps always use version 0.0.0.
            None => {
                if version == semver::Version::new(0, 0, 0) {
                    DepKind::Escape
                } else {
                    DepKind::Index
                }
            }
        };
        Ok(LockedDep {
            name,
            version,
            source: self.source.ok_or_else(|| missing("source"))?,
            rev,
            sha256: self.sha256.ok_or_else(|| missing("sha256"))?,
            kind,
        })
    }
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
    use super::{DepKind, LockedDep, LockedRev, Lockfile};
    use crate::index::PinnedRev;
    use std::path::PathBuf;

    /// A valid 40-hex SHA used in fixtures.
    const FIXTURE_SHA: &str = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";

    fn dep(name: &str, version: &str) -> LockedDep {
        LockedDep {
            name: name.to_owned(),
            version: semver::Version::parse(version).expect("valid version"),
            source: format!("https://example.invalid/{name}"),
            rev: LockedRev::Pinned(PinnedRev::from_full_sha(name, FIXTURE_SHA).expect("valid sha")),
            sha256: format!("hash-of-{name}"),
            kind: DepKind::Index,
        }
    }

    fn escape_dep(name: &str) -> LockedDep {
        LockedDep {
            name: name.to_owned(),
            version: semver::Version::new(0, 0, 0),
            source: format!("https://example.invalid/{name}"),
            rev: LockedRev::Pinned(PinnedRev::from_full_sha(name, FIXTURE_SHA).expect("valid sha")),
            sha256: format!("hash-of-{name}"),
            kind: DepKind::Escape,
        }
    }

    fn path_dep(name: &str) -> LockedDep {
        LockedDep {
            name: name.to_owned(),
            version: semver::Version::new(0, 0, 0),
            source: format!("/local/path/{name}"),
            rev: LockedRev::Local,
            sha256: format!("hash-of-{name}"),
            kind: DepKind::Escape,
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ipe-lockfile-test-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn write_then_read_round_trips() {
        let root = temp_dir("roundtrip");
        let mut lock = Lockfile::default();
        lock.upsert(dep("http-extras", "1.2.0"));
        lock.upsert(dep("json-tools", "0.4.1"));
        lock.write(&root).expect("write");

        let read = Lockfile::read(&root).expect("read");
        assert_eq!(read, lock);
        assert_eq!(read.packages().len(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn output_is_deterministic_and_sorted() {
        // Insert out of order; both the in-memory order and the rendered text
        // are sorted by name, so a second write is byte-identical.
        let root_a = temp_dir("det-a");
        let root_b = temp_dir("det-b");
        let mut a = Lockfile::default();
        a.upsert(dep("zeta", "1.0.0"));
        a.upsert(dep("alpha", "2.0.0"));
        let mut b = Lockfile::default();
        b.upsert(dep("alpha", "2.0.0"));
        b.upsert(dep("zeta", "1.0.0"));
        a.write(&root_a).expect("write a");
        b.write(&root_b).expect("write b");
        let text_a = std::fs::read_to_string(root_a.join("ipe.lock")).expect("read a");
        let text_b = std::fs::read_to_string(root_b.join("ipe.lock")).expect("read b");
        assert_eq!(text_a, text_b);
        // `alpha` sorts before `zeta` in the rendered file.
        let alpha_at = text_a.find("alpha").expect("alpha present");
        let zeta_at = text_a.find("zeta").expect("zeta present");
        assert!(alpha_at < zeta_at, "packages must be name-sorted");
        let _ = std::fs::remove_dir_all(&root_a);
        let _ = std::fs::remove_dir_all(&root_b);
    }

    #[test]
    fn upsert_replaces_an_existing_entry() {
        let mut lock = Lockfile::default();
        lock.upsert(dep("http-extras", "1.0.0"));
        lock.upsert(dep("http-extras", "1.2.0"));
        assert_eq!(lock.packages().len(), 1);
        let only = lock.packages().first().expect("one package");
        assert_eq!(
            only.version,
            semver::Version::parse("1.2.0").expect("valid")
        );
    }

    #[test]
    fn remove_deletes_and_reports_presence() {
        let mut lock = Lockfile::default();
        lock.upsert(dep("http-extras", "1.0.0"));
        assert!(lock.remove("http-extras"));
        assert!(lock.packages().is_empty());
        assert!(
            !lock.remove("http-extras"),
            "removing an absent dep is false"
        );
    }

    #[test]
    fn a_missing_lockfile_reads_as_empty() {
        let root = temp_dir("absent");
        let lock = Lockfile::read(&root).expect("absent lockfile is empty, not an error");
        assert!(lock.packages().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn path_dep_round_trips_with_local_rev() {
        // A local-path escape dep uses `rev = "local"` and must round-trip.
        let root = temp_dir("path-roundtrip");
        let mut lock = Lockfile::default();
        lock.upsert(path_dep("mylocal"));
        lock.write(&root).expect("write");

        let read = Lockfile::read(&root).expect("read");
        let entry = read
            .packages()
            .iter()
            .find(|p| p.name == "mylocal")
            .expect("mylocal present");
        assert_eq!(entry.rev, LockedRev::Local);
        assert_eq!(entry.kind, DepKind::Escape);
        let _ = std::fs::remove_dir_all(&root);
    }

    // --- New tests for PinnedRev typing and DepKind tag ---

    #[test]
    fn legacy_head_rev_fails_closed_on_read() {
        // A lockfile carrying `rev = "HEAD"` must fail closed at parse time.
        let root = temp_dir("legacy-head");
        let lockfile_text = "# ipe.lock\n\n[[package]]\nname = \"mylib\"\nversion = \"1.0.0\"\n\
             source = \"https://example.invalid/mylib\"\nrev = \"HEAD\"\n\
             sha256 = \"abc\"\n"
            .to_owned();
        std::fs::write(root.join("ipe.lock"), lockfile_text).expect("write");
        let err = Lockfile::read(&root).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("rev") || msg.contains("immutable") || msg.contains("SHA"),
            "error must name the bad rev, got: {msg}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn legacy_branch_rev_fails_closed_on_read() {
        // A lockfile carrying `rev = "main"` must fail closed.
        let root = temp_dir("legacy-branch");
        let lockfile_text = "# ipe.lock\n\n[[package]]\nname = \"mylib\"\nversion = \"1.0.0\"\n\
             source = \"https://example.invalid/mylib\"\nrev = \"main\"\n\
             sha256 = \"abc\"\n"
            .to_owned();
        std::fs::write(root.join("ipe.lock"), lockfile_text).expect("write");
        let err = Lockfile::read(&root).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("rev") || msg.contains("immutable") || msg.contains("SHA"),
            "error must name the bad rev, got: {msg}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn escape_dep_round_trips_with_escape_kind() {
        // A `{git=}` escape dep round-trips through write→read with its Escape
        // kind tag intact.
        let root = temp_dir("escape-roundtrip");
        let mut lock = Lockfile::default();
        lock.upsert(escape_dep("myescape"));
        lock.write(&root).expect("write");

        let read = Lockfile::read(&root).expect("read");
        let entry = read
            .packages()
            .iter()
            .find(|p| p.name == "myescape")
            .expect("myescape present");
        assert_eq!(
            entry.kind,
            DepKind::Escape,
            "escape dep must round-trip with Escape kind"
        );
        assert_eq!(entry.rev.as_str(), FIXTURE_SHA);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn index_dep_round_trips_with_index_kind() {
        // An index dep round-trips through write→read with its Index kind tag
        // intact.
        let root = temp_dir("index-roundtrip");
        let mut lock = Lockfile::default();
        lock.upsert(dep("mypkg", "1.2.0"));
        lock.write(&root).expect("write");

        let read = Lockfile::read(&root).expect("read");
        let entry = read
            .packages()
            .iter()
            .find(|p| p.name == "mypkg")
            .expect("mypkg present");
        assert_eq!(
            entry.kind,
            DepKind::Index,
            "index dep must round-trip with Index kind"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn pathological_index_dep_at_0_0_0_is_not_misclassified() {
        // An index dep published at exactly version 0.0.0 with a 40-hex rev
        // must be classified as Index (not Escape) when the `kind` tag is present.
        let root = temp_dir("pathological-index");
        let lockfile_text = format!(
            "# ipe.lock\n\n[[package]]\nname = \"weirdpkg\"\nversion = \"0.0.0\"\n\
             source = \"https://example.invalid/weirdpkg\"\nrev = \"{FIXTURE_SHA}\"\n\
             sha256 = \"abc\"\nkind = \"index\"\n"
        );
        std::fs::write(root.join("ipe.lock"), lockfile_text).expect("write");
        let lock = Lockfile::read(&root).expect("read");
        let entry = lock
            .packages()
            .iter()
            .find(|p| p.name == "weirdpkg")
            .expect("weirdpkg present");
        assert_eq!(
            entry.kind,
            DepKind::Index,
            "a 0.0.0 index dep with the kind tag must not be misclassified as Escape"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn legacy_lockfile_without_kind_infers_from_version() {
        // A lockfile written before the `kind` field existed has no `kind` line.
        // The parser infers: version 0.0.0 → Escape, anything else → Index.
        let root = temp_dir("legacy-no-kind");
        let escape_text = format!(
            "# ipe.lock\n\n[[package]]\nname = \"oldescape\"\nversion = \"0.0.0\"\n\
             source = \"https://example.invalid/oldescape\"\nrev = \"{FIXTURE_SHA}\"\n\
             sha256 = \"abc\"\n\
             \n[[package]]\nname = \"oldindex\"\nversion = \"1.2.0\"\n\
             source = \"https://example.invalid/oldindex\"\nrev = \"{FIXTURE_SHA}\"\n\
             sha256 = \"def\"\n"
        );
        std::fs::write(root.join("ipe.lock"), escape_text).expect("write");
        let lock = Lockfile::read(&root).expect("read");
        let escape_entry = lock
            .packages()
            .iter()
            .find(|p| p.name == "oldescape")
            .expect("oldescape present");
        let index_entry = lock
            .packages()
            .iter()
            .find(|p| p.name == "oldindex")
            .expect("oldindex present");
        assert_eq!(
            escape_entry.kind,
            DepKind::Escape,
            "legacy 0.0.0 dep infers Escape"
        );
        assert_eq!(
            index_entry.kind,
            DepKind::Index,
            "legacy non-0.0.0 dep infers Index"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unrecognised_kind_value_fails_closed() {
        // An unrecognised `kind` value is a hard error, not silently ignored.
        let root = temp_dir("bad-kind");
        let lockfile_text = format!(
            "# ipe.lock\n\n[[package]]\nname = \"mypkg\"\nversion = \"1.0.0\"\n\
             source = \"https://example.invalid/mypkg\"\nrev = \"{FIXTURE_SHA}\"\n\
             sha256 = \"abc\"\nkind = \"frobnicator\"\n"
        );
        std::fs::write(root.join("ipe.lock"), lockfile_text).expect("write");
        let err = Lockfile::read(&root).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("kind") || msg.contains("frobnicator"),
            "error must mention the bad kind value, got: {msg}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
