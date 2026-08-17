//! Package resolution: turn `ipe add <name>` into a fetched, hash-verified,
//! locked dependency.
//!
//! The flow for an index dependency ([`resolve_and_add`]): read the index entry,
//! resolve the highest version satisfying the requirement, `git`-fetch that
//! version's source at its pinned revision into the package cache, hash the
//! fetched tree, and **verify the hash equals the one the index pinned before
//! anything is written**. Only then is the resolution recorded — in `ipe.lock`
//! (the exact pins) and in `ipe.toml`'s `[dependencies]` (the requirement) — and
//! the resolved version and its capability set printed for consent.
//!
//! The `{git=}` / `{path=}` escapes ([`resolve_escape`]) bypass the index by
//! design but still carry lockfile integrity: the fetched (or copied) tree is
//! hashed and that hash is locked, so a later build re-verifies the same source.
//!
//! Verify-before-trust is the security boundary: a content-hash mismatch is a
//! hard [`CliError::HashMismatch`], never a warning — the fetched bytes are not
//! the source the publisher registered, so nothing derived from them is trusted.

use std::path::{Path, PathBuf};
use std::process::Command;

use ipe_ir::Capability;

use crate::index::{self, CommitId, EntryVersion, SourceUrl};
use crate::lockfile::{LockedDep, Lockfile};
use crate::project::{self, IpeDep};
use crate::{CliError, cache};

/// The environment variable overriding the index checkout root; tests point it
/// at a fixture index. Absent, the standard location ([`default_index_root`]) is
/// used.
const INDEX_DIR_ENV: &str = "IPE_INDEX_DIR";

/// Resolve an index dependency and record it: fetch the source at the pinned
/// revision, verify its content hash, then write the lockfile and manifest.
///
/// `index_root` is the index checkout to read from (a fixture in tests, the
/// standard location otherwise). Nothing is written until the fetched source's
/// hash matches the index-pinned hash.
///
/// # Errors
/// [`CliError::Resolve`] if the package or a matching version is not found, or a
/// `git` fetch fails; [`CliError::HashMismatch`] if the fetched source's hash
/// does not equal the pinned hash; [`CliError::Io`] on a filesystem failure.
pub fn resolve_and_add(
    project_root: &Path,
    name: &str,
    req: &semver::VersionReq,
    index_root: &Path,
) -> Result<(), CliError> {
    let entry = index::read_entry(index_root, name)?;
    let version = index::resolve_version(&entry, req)?;

    let checkout = fetch_source(project_root, name, &version.version.to_string(), version)?;
    verify_hash(name, &checkout, &version.sha256)?;

    let locked = LockedDep {
        name: name.to_owned(),
        version: version.version.clone(),
        source: version.source.to_string(),
        rev: version.rev.to_string(),
        sha256: version.sha256.clone(),
    };
    write_records(project_root, name, &locked, &IpeDep::Index(req.clone()))?;

    report_added(name, &version.version.to_string(), &version.capabilities);
    Ok(())
}

/// Resolve one of the `{git=}` / `{path=}` escapes and record it.
///
/// Fetch (git) or copy (path) the source into the package cache, hash the tree,
/// and lock that hash. The escape bypasses the index but still carries lockfile
/// integrity.
///
/// The manifest is not rewritten here — the escape is already spelled in
/// `[dependencies]` by the author; this locks what it points at.
///
/// # Errors
/// [`CliError::Resolve`] if a `git` fetch fails or a path source is missing;
/// [`CliError::Io`] on a filesystem failure.
pub fn resolve_escape(project_root: &Path, name: &str, dep: &IpeDep) -> Result<(), CliError> {
    let (source, rev, checkout) = match dep {
        IpeDep::Git { url, rev } => {
            // Parse-don't-validate: convert the raw manifest strings to typed
            // newtypes at this escape-path boundary before they reach the git
            // sink, so the sink cannot be called with an unvalidated value.
            let typed_url = SourceUrl::parse(name, url)?;
            let raw_rev = rev.as_deref().unwrap_or("HEAD");
            let typed_rev = CommitId::parse(name, raw_rev)?;
            let checkout = fetch_git(project_root, name, &typed_url, &typed_rev)?;
            (url.clone(), typed_rev.to_string(), checkout)
        }
        IpeDep::Path(path) => {
            let resolved = if path.is_absolute() {
                path.clone()
            } else {
                project_root.join(path)
            };
            if !resolved.is_dir() {
                return Err(CliError::Resolve(format!(
                    "package `{name}`: path dependency `{}` does not exist",
                    resolved.display()
                )));
            }
            (path.display().to_string(), "local".to_owned(), resolved)
        }
        IpeDep::Index(_) => {
            return Err(CliError::Resolve(format!(
                "package `{name}`: an index dependency is resolved through `resolve_and_add`, \
                 not `resolve_escape`"
            )));
        }
    };

    let sha256 = hash_checkout(&checkout)?;
    // An escape has no published version; `0.0.0` marks "locked from an escape,
    // not the index" without inventing a version the source does not claim.
    let version = semver::Version::new(0, 0, 0);
    let locked = LockedDep {
        name: name.to_owned(),
        version,
        source,
        rev,
        sha256,
    };
    let mut lock = Lockfile::read(project_root)?;
    lock.upsert(locked);
    lock.write(project_root)?;
    Ok(())
}

/// Remove a dependency: drop it from both `ipe.toml` `[dependencies]` and
/// `ipe.lock`. A clean add→remove cycle leaves both files as they began.
///
/// # Errors
/// [`CliError::Io`] if the manifest or lockfile cannot be read or written.
pub fn resolve_and_remove(project_root: &Path, name: &str) -> Result<(), CliError> {
    project::remove_dependency(&manifest_path(project_root), name)?;
    let mut lock = Lockfile::read(project_root)?;
    let was_locked = lock.remove(name);
    lock.write(project_root)?;
    if was_locked {
        print!(
            "{}",
            crate::style::frame(&crate::style::gutter(&format!("Removed `{name}`.")))
        );
    } else {
        print!(
            "{}",
            crate::style::frame(&crate::style::gutter(&format!(
                "`{name}` was not a dependency; nothing to remove."
            )))
        );
    }
    Ok(())
}

/// The default index checkout root when `IPE_INDEX_DIR` is unset.
///
/// The standard per-user location. Provisioning and populating this checkout is
/// a separate, deliberate outward-facing step; the resolver only reads it.
#[must_use]
pub fn default_index_root() -> PathBuf {
    // Mirror the build cache's home discovery so the index lives beside it.
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from(".ipe"));
    base.join("ipe").join("index")
}

/// The index checkout root: `IPE_INDEX_DIR` when set, else [`default_index_root`].
#[must_use]
pub fn index_root() -> PathBuf {
    std::env::var_os(INDEX_DIR_ENV).map_or_else(default_index_root, PathBuf::from)
}

/// The content hash of a source tree.
///
/// The same hash the index pins and the resolver verifies against. Exposed so a
/// caller (e.g. `ipe package publish`, or a test building a fixture index)
/// computes the exact hash the resolver expects, rather than reimplementing the
/// tree walk.
///
/// # Errors
/// [`CliError::Io`] if the tree cannot be walked or a file cannot be read.
pub fn hash_source_tree(root: &Path) -> Result<String, CliError> {
    hash_checkout(root)
}

/// Fetch a specific published index version's source at its pinned revision into
/// the package cache and verify its content hash equals the index pin, returning
/// the verified checkout directory.
///
/// The SP4 package gate's enforced-semver check calls this to materialise the
/// previous published version's source as the semver baseline — the exact bytes
/// the index registered, since nothing derived from an unverified fetch is
/// returned (verify-before-trust, the same boundary [`resolve_and_add`] applies
/// at install).
///
/// # Errors
/// [`CliError::Resolve`] on a `git` fetch failure; [`CliError::HashMismatch`]
/// when the fetched tree's hash does not equal the pinned hash; [`CliError::Io`]
/// on a filesystem failure.
pub fn fetch_and_verify_index_version(
    project_root: &Path,
    name: &str,
    version: &EntryVersion,
) -> Result<PathBuf, CliError> {
    let checkout = fetch_source(project_root, name, &version.version.to_string(), version)?;
    verify_hash(name, &checkout, &version.sha256)?;
    Ok(checkout)
}

/// Re-verify that every locked Ipê dependency's cached source still hashes to the
/// pin recorded in `ipe.lock`.
///
/// The resolver verifies a fetched tree's hash at install ([`resolve_and_add`]);
/// this re-asserts the same integrity over the ALREADY-cached trees at publish
/// (the SP4 supply-chain check). A dependency whose cached bytes drifted from the
/// locked hash is a hard [`CliError::HashMismatch`] — the same verify-before-trust
/// boundary, never a warning. A dependency whose cache directory is absent is not
/// a mismatch (nothing was tampered; a build re-fetches it), so it is skipped.
///
/// # Errors
/// [`CliError::HashMismatch`] when a cached tree no longer matches its locked
/// hash; [`CliError::Io`] on a read failure; [`CliError::Resolve`] on a malformed
/// lockfile.
pub fn verify_lockfile_hashes(project_root: &Path) -> Result<(), CliError> {
    let lockfile = Lockfile::read(project_root)?;
    for dep in lockfile.packages() {
        let cache_dir = package_cache_dir(project_root, &dep.name, &dep.version.to_string());
        if !cache_dir.is_dir() {
            // Not cached locally — nothing to re-verify here; a build re-fetches
            // and re-verifies against this same pin.
            continue;
        }
        verify_hash(&dep.name, &cache_dir, &dep.sha256)?;
    }
    Ok(())
}

/// The manifest path for a project root.
fn manifest_path(project_root: &Path) -> PathBuf {
    project_root.join("ipe.toml")
}

/// The package cache directory for one resolved `(name, version)` under the
/// project's `.ipe/packages/` tree.
fn package_cache_dir(project_root: &Path, name: &str, version: &str) -> PathBuf {
    project_root
        .join(".ipe")
        .join("packages")
        .join(format!("{name}-{version}"))
}

/// Fetch an index version's source at its pinned revision into the package
/// cache, returning the checkout directory.
fn fetch_source(
    project_root: &Path,
    name: &str,
    version: &str,
    entry: &EntryVersion,
) -> Result<PathBuf, CliError> {
    let dest = package_cache_dir(project_root, name, version);
    fetch_git_into(name, &entry.source, &entry.rev, &dest)?;
    Ok(dest)
}

/// Fetch a git escape's source at `rev` into the package cache, keyed by the
/// revision so distinct revisions do not collide.
fn fetch_git(
    project_root: &Path,
    name: &str,
    url: &SourceUrl,
    rev: &CommitId,
) -> Result<PathBuf, CliError> {
    let dest = package_cache_dir(project_root, name, rev.as_str());
    fetch_git_into(name, url, rev, &dest)?;
    Ok(dest)
}

/// Clone `url` into `dest` and check out exactly `rev`. A pre-existing `dest` is
/// removed first so a re-add always fetches fresh (never a half-populated cache).
///
/// Uses the `git` subprocess (no HTTP-client dependency, per the design): a
/// bare-ish clone plus an explicit `checkout` of the pinned revision, so the
/// resolved tree is exactly that commit, never a moving branch tip.
///
/// Both `url` and `rev` are typed newtypes — a raw unvalidated string cannot
/// reach this function. Defense-in-depth: `GIT_ALLOW_PROTOCOL` restricts
/// transports (network + `file`) even if a value somehow bypassed the parse
/// boundary. `--` terminates git's option list for clone so the URL is always
/// a positional; checkout omits `--` because in checkout it means "treat as a
/// path, not a ref" — the `CommitId` boundary already guarantees `rev` cannot
/// start with `-`.
fn fetch_git_into(
    name: &str,
    url: &SourceUrl,
    rev: &CommitId,
    dest: &Path,
) -> Result<(), CliError> {
    if dest.exists() {
        std::fs::remove_dir_all(dest).map_err(|e| CliError::Io {
            path: dest.to_path_buf(),
            source: e,
        })?;
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| CliError::Io {
            path: parent.to_path_buf(),
            source: e,
        })?;
    }
    // Extract raw strings at the last moment before handing them to the
    // subprocess: the typed newtypes carry the validation guarantee through to
    // this point; `.as_str()` is the only site that converts back to `&str`.
    // `--` terminates git's option list for clone: the URL that follows is a
    // positional, never a flag. For checkout, `--` would make git treat the
    // rev as a file path instead of a ref, so it is omitted — the
    // `CommitId` parse boundary already ensures `rev` cannot start with `-`.
    run_git(name, &["clone", "--quiet", "--", url.as_str()], dest, None)?;
    run_git(
        name,
        &["checkout", "--quiet", rev.as_str()],
        dest,
        Some(dest),
    )?;
    Ok(())
}

/// Run `git <args>`, treating a spawn failure or a non-zero exit as a
/// [`CliError::Resolve`] naming the package. When `cwd` is `Some`, git runs
/// there; otherwise the final arg of a `clone` is the destination path (git's
/// own convention), so `dest` is appended.
///
/// `GIT_ALLOW_PROTOCOL` is always set, restricting git to the same transports
/// the index parse boundary allows (network transports plus `file`).
/// `GIT_TERMINAL_PROMPT=0` ensures git never blocks waiting for interactive
/// credentials.
fn run_git(name: &str, args: &[&str], dest: &Path, cwd: Option<&Path>) -> Result<(), CliError> {
    let mut command = Command::new("git");
    command.args(args);
    if cwd.is_none() {
        // A `clone` takes the destination as its final positional argument.
        command.arg(dest);
    }
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    // Defense-in-depth: restrict git transports at the subprocess level so a
    // value that bypassed the parse boundary still cannot open an arbitrary
    // transport (`file` is included for local-path and file:// sources).
    // `GIT_TERMINAL_PROMPT=0` prevents credential prompts that would block a
    // non-interactive `ipe add`.
    command
        .env("GIT_ALLOW_PROTOCOL", "https:git:ssh:file")
        .env("GIT_TERMINAL_PROMPT", "0");
    let output = command
        .output()
        .map_err(|e| CliError::Resolve(format!("package `{name}`: could not run `git`: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CliError::Resolve(format!(
            "package `{name}`: `git {}` failed: {}",
            args.join(" "),
            stderr.trim()
        )));
    }
    Ok(())
}

/// Hash the fetched source tree, mapping a walk/read failure to an IO error.
fn hash_checkout(checkout: &Path) -> Result<String, CliError> {
    cache::hash_tree(checkout).map_err(|(path, source)| CliError::Io { path, source })
}

/// Verify the fetched tree's content hash equals the index-pinned hash. This is
/// the verify-before-trust boundary: a mismatch is a hard error, so nothing
/// derived from an unverified fetch is ever written.
fn verify_hash(name: &str, checkout: &Path, expected: &str) -> Result<(), CliError> {
    let actual = hash_checkout(checkout)?;
    if actual == expected {
        Ok(())
    } else {
        Err(CliError::HashMismatch {
            package: name.to_owned(),
            expected: expected.to_owned(),
            actual,
        })
    }
}

/// Write the lockfile pin and the manifest requirement for a resolved index
/// dependency. Both writes happen only after the hash verified.
fn write_records(
    project_root: &Path,
    name: &str,
    locked: &LockedDep,
    dep: &IpeDep,
) -> Result<(), CliError> {
    let mut lock = Lockfile::read(project_root)?;
    lock.upsert(locked.clone());
    lock.write(project_root)?;
    project::upsert_dependency(&manifest_path(project_root), name, dep)
}

/// Print the resolved version and its capability set for consent.
fn report_added(name: &str, version: &str, capabilities: &std::collections::BTreeSet<Capability>) {
    print!(
        "{}",
        crate::style::frame(&crate::style::gutter(&added_report(
            name,
            version,
            capabilities
        )))
    );
}

/// The `ipe add` consent report: the resolved version, the capability set, and —
/// loud — a warning when the package uses `native-ffi` (it crosses into opaque
/// native code, the one capability inference cannot see past). A pure function of
/// its inputs so the exact wording is testable.
///
/// Returns unindented body text; the caller applies the 2-space gutter.
fn added_report(
    name: &str,
    version: &str,
    capabilities: &std::collections::BTreeSet<Capability>,
) -> String {
    use std::fmt::Write as _;
    let mut out = format!("Added `{name}` {version}.\n");
    if capabilities.is_empty() {
        out.push_str("capabilities: none\n");
    } else {
        let names: Vec<&str> = capabilities.iter().map(|c| c.as_str()).collect();
        let _ = writeln!(out, "capabilities: {}", names.join(", "));
    }
    if capabilities.contains(&Capability::NativeFfi) {
        let _ = writeln!(
            out,
            "WARNING: `{name}` uses native FFI (`native-ffi`) — it runs native code whose \
             true capabilities cannot be inferred from Ipê. Review its source before trusting it."
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        added_report, fetch_git_into, package_cache_dir, resolve_and_remove, resolve_escape,
        verify_hash,
    };
    use crate::cache;
    use crate::index::{CommitId, SourceUrl};
    use crate::lockfile::Lockfile;
    use crate::project::IpeDep;
    use ipe_ir::Capability;
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ipe-resolve-test-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn scaffold_project(root: &Path) {
        std::fs::create_dir_all(root.join("src")).expect("src dir");
        std::fs::write(root.join("src").join("Main.ipe"), "module Main\n").expect("main");
        std::fs::write(root.join("ipe.toml"), "name = \"app\"\n").expect("manifest");
    }

    /// Create a git repo with one file at HEAD, returning its path.
    fn git_source(tag: &str, content: &str) -> PathBuf {
        let repo = temp_dir(&format!("src-{tag}"));
        let git = |args: &[&str]| {
            let ok = Command::new("git")
                .args(args)
                .current_dir(&repo)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .expect("git runs")
                .status
                .success();
            assert!(ok, "git {args:?} must succeed");
        };
        git(&["init", "--quiet"]);
        std::fs::write(repo.join("lib.ipe"), content).expect("write file");
        git(&["add", "."]);
        git(&["commit", "--quiet", "-m", "seed"]);
        repo
    }

    #[test]
    fn verify_hash_rejects_a_mismatch() {
        // The verify-before-trust boundary: a wrong expected hash is a hard
        // HashMismatch, never accepted.
        let dir = temp_dir("verify");
        std::fs::write(dir.join("a.txt"), "hello").expect("write");
        let real = cache::hash_tree(&dir).expect("hash");
        verify_hash("p", &dir, &real).expect("matching hash passes");
        let err = verify_hash("p", &dir, "not-the-hash").unwrap_err();
        assert!(matches!(err, crate::CliError::HashMismatch { .. }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_path_escape_locks_its_hash() {
        let proj = temp_dir("path-escape");
        scaffold_project(&proj);
        let src = git_source("path", "module Lib\n");
        let dep = IpeDep::Path(src.clone());
        resolve_escape(&proj, "locallib", &dep).expect("path escape resolves");
        let lock = Lockfile::read(&proj).expect("lock");
        let entry = lock
            .packages()
            .iter()
            .find(|p| p.name == "locallib")
            .expect("locked");
        assert!(!entry.sha256.is_empty(), "an escape still locks a hash");
        let _ = std::fs::remove_dir_all(&proj);
        let _ = std::fs::remove_dir_all(&src);
    }

    #[test]
    fn a_git_escape_fetches_and_locks() {
        let proj = temp_dir("git-escape");
        scaffold_project(&proj);
        let src = git_source("git", "module Lib\ngreeting = \"hi\"\n");
        let dep = IpeDep::Git {
            url: src.display().to_string(),
            rev: None,
        };
        resolve_escape(&proj, "remotelib", &dep).expect("git escape resolves");
        let lock = Lockfile::read(&proj).expect("lock");
        assert!(lock.packages().iter().any(|p| p.name == "remotelib"));
        // The fetched tree landed in the package cache.
        assert!(package_cache_dir(&proj, "remotelib", "HEAD").exists());
        let _ = std::fs::remove_dir_all(&proj);
        let _ = std::fs::remove_dir_all(&src);
    }

    #[test]
    fn the_add_report_shows_the_capability_set() {
        let caps: BTreeSet<Capability> = [Capability::Network, Capability::Clock]
            .into_iter()
            .collect();
        let report = added_report("http-extras", "1.2.0", &caps);
        assert!(report.contains("Added `http-extras` 1.2.0."));
        assert!(report.contains("capabilities: network, clock"));
        assert!(
            !report.contains("WARNING"),
            "no native-ffi means no warning"
        );
    }

    #[test]
    fn the_add_report_is_loud_on_native_ffi() {
        let caps: BTreeSet<Capability> = std::iter::once(Capability::NativeFfi).collect();
        let report = added_report("risky", "0.1.0", &caps);
        assert!(report.contains("native-ffi"));
        assert!(
            report.contains("WARNING"),
            "native-ffi must be surfaced loudly"
        );
    }

    #[test]
    fn the_add_report_names_no_capabilities() {
        let report = added_report("pure", "1.0.0", &BTreeSet::new());
        assert!(report.contains("capabilities: none"));
    }

    #[test]
    fn remove_of_an_absent_dep_is_clean() {
        let proj = temp_dir("remove-absent");
        scaffold_project(&proj);
        resolve_and_remove(&proj, "nope").expect("removing an absent dep is not an error");
        let manifest = std::fs::read_to_string(proj.join("ipe.toml")).expect("manifest");
        assert!(!manifest.contains("nope"));
        let _ = std::fs::remove_dir_all(&proj);
    }

    // --- git hardening: env vars and `--` option terminator ---

    #[test]
    fn git_clone_uses_double_dash_before_url() {
        // Exercises the `git clone -- <url> <dest>` path: `--` terminates git's
        // option list so the URL is always a positional. The checkout uses the
        // validated rev without `--` (checkout's `--` means "path, not ref").
        // Both url and rev must be typed newtypes — raw strings cannot reach
        // `fetch_git_into` directly, enforcing parse-don't-validate at the sink.
        let src = git_source("dash-url-clone", "module Lib\n");
        let dest = temp_dir("dash-url-dest");
        let url = SourceUrl::parse("p", &src.display().to_string())
            .expect("local path is a valid source URL");
        let rev = CommitId::parse("p", "HEAD").expect("HEAD is a valid commit id");
        fetch_git_into("p", &url, &rev, &dest).expect("clone succeeds for a valid local repo");
        assert!(dest.is_dir(), "destination was populated");
        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&dest);
    }

    #[test]
    fn source_url_newtype_rejects_ext_transport_before_fetch() {
        // A `source` field containing `ext::` must be rejected by `SourceUrl::parse`
        // at the index-parse boundary; `fetch_git_into` is never called.
        let err = SourceUrl::parse("evil", "ext::sh -c 'id'").unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("source"), "{msg}");
    }

    #[test]
    fn source_url_newtype_rejects_dash_leading_before_fetch() {
        let err = SourceUrl::parse("evil", "--upload-pack=malicious").unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("source"), "{msg}");
    }

    #[test]
    fn commit_id_newtype_rejects_injection_shaped_rev_before_checkout() {
        // An injection-shaped `rev` (leading `-`) is rejected at parse time
        // so it never reaches `git checkout`. Ordinary ref names are accepted.
        assert!(
            CommitId::parse("ok", "main").is_ok(),
            "branch names are valid refs"
        );
        assert!(
            CommitId::parse("ok", "abc").is_ok(),
            "short hashes are valid refs"
        );
        let err = CommitId::parse("evil", "-S injected").unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("rev"), "{msg}");
    }

    #[test]
    fn commit_id_newtype_rejects_dash_rev_before_checkout() {
        let err = CommitId::parse("evil", "-S injected").unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("rev"), "{msg}");
    }

    #[test]
    fn git_escape_with_injection_shaped_url_is_rejected_before_fetch() {
        // The `{git=}` manifest-escape path parses `url` through `SourceUrl::parse`
        // before calling the git sink, so an injection-shaped value is caught at
        // the escape boundary, not silently forwarded to the subprocess.
        let proj = temp_dir("escape-bad-url");
        scaffold_project(&proj);
        let dep = IpeDep::Git {
            url: "ext::sh -c 'id > /tmp/pwned'".to_owned(),
            rev: None,
        };
        let err = resolve_escape(&proj, "evil", &dep).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("source"), "bad url rejected: {msg}");
        let _ = std::fs::remove_dir_all(&proj);
    }

    #[test]
    fn git_escape_with_injection_shaped_rev_is_rejected_before_fetch() {
        // The `{git=}` manifest-escape path also parses `rev` through
        // `CommitId::parse`, so a leading-dash rev is caught before git runs.
        let src = git_source("escape-rev-src", "module Lib\n");
        let proj = temp_dir("escape-bad-rev");
        scaffold_project(&proj);
        let dep = IpeDep::Git {
            url: src.display().to_string(),
            rev: Some("-S injected".to_owned()),
        };
        let err = resolve_escape(&proj, "evil", &dep).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("rev"), "bad rev rejected: {msg}");
        let _ = std::fs::remove_dir_all(&proj);
        let _ = std::fs::remove_dir_all(&src);
    }
}
