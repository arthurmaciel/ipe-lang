//! `ipe package publish` — prepare a package's index entry and open the index PR.
//!
//! Publish is a thin, non-privileged helper (design:
//! `docs/architecture/tbd/package-publish-and-index-plan.md`). It runs the same
//! [`crate::audit::run_audit`] gate the author and the index CI run, computes the
//! [`crate::index::EntryVersion`] for the working package, merges it into the
//! package's `packages/<name>.toml` entry file, and opens a pull request against
//! the index repository. It holds no index credentials: the index CI is the
//! authority, publish only opens the PR.
//!
//! Verify-before-trust at authoring time: the pinned `rev` and `sha256` are
//! COMPUTED from the working tree, never authored, so the pin the resolver later
//! re-verifies cannot be mistyped. Publish refuses anything that would pin a
//! non-reproducible source — a dirty working tree or an unpushed HEAD — so a
//! merged entry always names an immutable, fetchable revision.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::CliError;
use crate::index::{self, EntryVersion, IndexEntry};
use crate::project::{self, ProjectManifest};

/// A publish refusal: a typed reason publish declined to proceed.
///
/// Each variant carries its own already-rendered message. Distinct from an
/// [`crate::audit::Rejection`] (a gate reject) — these are the preconditions
/// publish itself enforces before a PR is ever prepared. A closed value so the
/// CLI boundary need only print it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The working tree has uncommitted changes — the pinned revision would not
    /// name the bytes actually published.
    DirtyTree { source_root: PathBuf },
    /// The current HEAD is not reachable from any remote branch — a consumer
    /// could not fetch the pinned revision.
    UnpushedHead { rev: String },
    /// The package's version is already published in the index — a published
    /// version is immutable and must never be rewritten.
    DuplicateVersion { name: String, version: String },
    /// Non-dry-run publishing needs a `GITHUB_TOKEN` (or a `gh`/browser path),
    /// and none was found.
    MissingToken,
    /// The source URL could not be determined (no `--source` and no git remote).
    NoSource,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DirtyTree { source_root } => write!(
                f,
                "the working tree at {} has uncommitted changes — publish pins the exact \
                 committed revision, so commit (or stash) every change first; otherwise the \
                 pinned `sha256`/`rev` would not name the bytes you publish.",
                source_root.display()
            ),
            Self::UnpushedHead { rev } => write!(
                f,
                "HEAD ({rev}) is not reachable from any remote branch — a published version \
                 pins an immutable, fetchable revision, so push this commit to its remote \
                 before publishing."
            ),
            Self::DuplicateVersion { name, version } => write!(
                f,
                "`{name}` {version} is already published in the index — a published version is \
                 immutable and must never be rewritten. Bump the version in `ipe.toml` and \
                 publish the new one."
            ),
            Self::MissingToken => f.write_str(
                "publishing over the network needs a GitHub token — set `GITHUB_TOKEN` to a \
                 token that can open a pull request against the index repo, or re-run with \
                 `--dry-run` to see the computed entry and intended PR without touching the \
                 network.",
            ),
            Self::NoSource => f.write_str(
                "could not determine the package's source URL — the index needs a public git \
                 URL the resolver can fetch. Pass `--source <url>`, or set an `origin` remote \
                 on the package's git repository.",
            ),
        }
    }
}

/// Build a [`CliError::Publish`] from a [`Refusal`].
const fn refuse(refusal: Refusal) -> CliError {
    CliError::Publish(refusal)
}

/// The parsed `ipe package publish` invocation.
#[derive(Debug)]
struct Args {
    /// The package directory or `ipe.toml` (defaults to the current directory).
    path: PathBuf,
    /// `--dry-run`: compute and print, touch no network.
    dry_run: bool,
    /// `--index <repo>`: the index GitHub repo (`owner/name`) the PR targets.
    index_repo: String,
    /// `--source <url>`: the source URL to pin, overriding the git remote.
    source: Option<String>,
    /// `--rev <sha>`: the revision to pin, overriding the git HEAD.
    rev: Option<String>,
}

/// The real curated index repository. A `--index` override retargets the PR (a
/// fork, a fixture) without changing any computed bytes.
const DEFAULT_INDEX_REPO: &str = "arthurmaciel/ipe-index";

/// `ipe package publish [--dry-run] [--index <repo>] [--source <url>] [--rev <sha>]`
/// — run the gate, compute the index entry, and open (or, under `--dry-run`,
/// print) the index PR.
///
/// # Errors
/// [`CliError::UsageOwned`] on argument misuse; [`CliError::PackageAudit`] when
/// the local gate rejects the package; [`CliError::Publish`] on a publish
/// precondition (dirty tree, unpushed HEAD, duplicate version, missing token);
/// resolution / IO errors otherwise.
pub fn run_publish(rest: &[String]) -> Result<(), CliError> {
    let args = parse_args(rest)?;

    // 1. Gate locally — refuse to publish a package that fails its own audit.
    //    `run_audit` prints its own passing line and returns the typed reject.
    //    It reads the previous published version from the resolver's index root
    //    (the same checkout `merge_into_entry` reads below), so the semver check
    //    and the duplicate-version check see one consistent index view.
    let audit_args: Vec<String> = vec![args.path.display().to_string()];
    crate::audit::run_audit(&audit_args)?;

    // 2. Compute the entry version from the working package.
    let manifest_path = locate_manifest(&args.path)?;
    let manifest = project::parse_manifest(&manifest_path)?;
    let entry_version =
        compute_entry_version(&manifest, args.source.as_deref(), args.rev.as_deref())?;

    let publisher = infer_publisher(&entry_version.source);

    // 3. Merge into the existing entry (append; refuse a duplicate version).
    let index_root = crate::resolve::index_root();
    let entry_toml = merge_into_entry(&index_root, &manifest.name, &publisher, &entry_version)?;

    // 4/5. Open the PR, or under --dry-run print the entry + intended PR.
    let plan = PrPlan {
        index_repo: args.index_repo.clone(),
        entry_file: format!("packages/{}.toml", manifest.name),
        branch: format!("publish/{}-{}", manifest.name, entry_version.version),
        title: format!("Publish {} {}", manifest.name, entry_version.version),
    };

    if args.dry_run {
        print_dry_run(&entry_toml, &plan);
        return Ok(());
    }

    open_pr(&entry_toml, &plan)
}

/// Parse `publish`'s tail into typed [`Args`].
///
/// # Errors
/// [`CliError::UsageOwned`] on an unknown flag, a missing flag value, or a second
/// positional.
fn parse_args(rest: &[String]) -> Result<Args, CliError> {
    let mut path: Option<PathBuf> = None;
    let mut dry_run = false;
    let mut index_repo: Option<String> = None;
    let mut source: Option<String> = None;
    let mut rev: Option<String> = None;

    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--dry-run" => dry_run = true,
            "--index" => index_repo = Some(take_value(&mut it, "--index")?),
            "--source" => source = Some(take_value(&mut it, "--source")?),
            "--rev" => rev = Some(take_value(&mut it, "--rev")?),
            flag if flag.starts_with('-') => {
                return Err(CliError::UsageOwned(format!(
                    "ipe package publish: unknown flag `{flag}`"
                )));
            }
            positional => {
                if path.is_some() {
                    return Err(CliError::Usage(
                        "ipe package publish: expected a single <path> argument",
                    ));
                }
                path = Some(PathBuf::from(positional));
            }
        }
    }

    Ok(Args {
        path: path.unwrap_or_else(|| PathBuf::from(".")),
        dry_run,
        index_repo: index_repo.unwrap_or_else(|| DEFAULT_INDEX_REPO.to_owned()),
        source,
        rev,
    })
}

/// Take the value following a flag, erroring when it is absent.
fn take_value<'a>(
    it: &mut impl Iterator<Item = &'a String>,
    flag: &str,
) -> Result<String, CliError> {
    it.next()
        .cloned()
        .ok_or_else(|| CliError::UsageOwned(format!("ipe package publish: {flag} needs a value")))
}

/// Resolve `path` (a directory or an `ipe.toml`) to its manifest file.
fn locate_manifest(path: &Path) -> Result<PathBuf, CliError> {
    if path.is_dir() {
        let candidate = path.join("ipe.toml");
        if candidate.is_file() {
            return Ok(candidate);
        }
        return Err(CliError::UsageOwned(format!(
            "ipe package publish: no `ipe.toml` in `{}` — publish operates on a publishable \
             Ipê package, which needs a manifest",
            path.display()
        )));
    }
    if path.extension().and_then(|e| e.to_str()) == Some("toml") && path.is_file() {
        return Ok(path.to_path_buf());
    }
    Err(CliError::UsageOwned(format!(
        "ipe package publish: `{}` is neither an Ipê project directory nor an ipe.toml",
        path.display()
    )))
}

/// Compute the [`EntryVersion`] for the working package: version from the
/// manifest, source from `--source` or the git remote, rev from `--rev` or the
/// committed HEAD, sha256 over the source tree, and the inferred capability set.
///
/// Refuses a dirty tree or an unpushed HEAD (unless an explicit `--rev` was
/// given, which the caller is asserting is immutable) so the pin names a
/// reproducible revision.
///
/// # Errors
/// [`CliError::Publish`] on a publish precondition; [`CliError::UsageOwned`] when
/// the manifest declares no version; resolution / IO errors otherwise.
fn compute_entry_version(
    manifest: &ProjectManifest,
    source_override: Option<&str>,
    rev_override: Option<&str>,
) -> Result<EntryVersion, CliError> {
    let version = manifest.version.clone().ok_or_else(|| {
        CliError::UsageOwned(format!(
            "ipe package publish: `{}` declares no `version = \"…\"` — publish records the \
             version being published, so the manifest must name one.",
            manifest.name
        ))
    })?;

    let source_root = &manifest.root;
    let source = match source_override {
        Some(s) => s.to_owned(),
        None => git_remote_url(source_root)?.ok_or_else(|| refuse(Refusal::NoSource))?,
    };

    // The revision is pinned exactly. An explicit `--rev` is taken as-is (the
    // caller asserts it is immutable); otherwise it is the committed HEAD, which
    // publish insists is clean and pushed so the pinned bytes are reproducible.
    let rev = match rev_override {
        Some(r) => r.to_owned(),
        None => committed_pushed_head(source_root)?,
    };

    let sha256 = crate::resolve::hash_source_tree(source_root)?;
    let capabilities = crate::infer_package_capabilities(&manifest_path_of(source_root))?;

    Ok(EntryVersion {
        version,
        source,
        rev,
        sha256,
        capabilities,
    })
}

/// The committed HEAD revision of the git repo at `source_root`, after insisting
/// the working tree is clean and the commit is pushed — the two preconditions for
/// pinning a reproducible, fetchable revision.
///
/// # Errors
/// [`CliError::Publish`] on a dirty tree or an unpushed HEAD; [`CliError::Resolve`]
/// when the path is not a git repository.
fn committed_pushed_head(source_root: &Path) -> Result<String, CliError> {
    if git_tree_is_dirty(source_root)? {
        return Err(refuse(Refusal::DirtyTree {
            source_root: source_root.to_path_buf(),
        }));
    }
    let head = git_head_rev(source_root)?;
    if git_rev_is_pushed(source_root, &head)? {
        Ok(head)
    } else {
        Err(refuse(Refusal::UnpushedHead { rev: head }))
    }
}

/// The `ipe.toml` path for a project root.
fn manifest_path_of(root: &Path) -> PathBuf {
    root.join("ipe.toml")
}

/// Render one [`EntryVersion`] as the `[[version]]` TOML block the index reader
/// parses, exactly round-tripping through [`crate::index::read_entry`].
///
/// Pure: a function of the version alone, so the rendered bytes are testable
/// against the reader without any filesystem or network.
#[must_use]
pub fn render_entry_version(version: &EntryVersion) -> String {
    let mut out = String::from("[[version]]\n");
    let _ = writeln!(out, "version = \"{}\"", version.version);
    let _ = writeln!(out, "source = \"{}\"", version.source);
    let _ = writeln!(out, "rev = \"{}\"", version.rev);
    let _ = writeln!(out, "sha256 = \"{}\"", version.sha256);
    let caps: Vec<String> = version
        .capabilities
        .iter()
        .map(|c| format!("\"{}\"", c.as_str()))
        .collect();
    let _ = writeln!(out, "capabilities = [{}]", caps.join(", "));
    out
}

/// Render a whole entry file: the `name`/`publisher` header followed by every
/// version block, in ascending version order.
#[must_use]
pub fn render_entry(name: &str, publisher: &str, versions: &[EntryVersion]) -> String {
    let mut out = format!("name = \"{name}\"\npublisher = \"{publisher}\"\n");
    let mut ordered: Vec<&EntryVersion> = versions.iter().collect();
    ordered.sort_by(|a, b| a.version.cmp(&b.version));
    for v in ordered {
        out.push('\n');
        out.push_str(&render_entry_version(v));
    }
    out
}

/// Merge `new_version` into the package's existing index entry (or create a first
/// entry), returning the rendered entry-file TOML.
///
/// Reads the current `packages/<name>.toml` from `index_root` if present, appends
/// the new version, and re-renders. Refuses a version already published — a
/// published version is immutable.
///
/// # Errors
/// [`CliError::Publish`] on a duplicate version; the reader's errors when an
/// existing entry file is present but malformed.
fn merge_into_entry(
    index_root: &Path,
    name: &str,
    publisher: &str,
    new_version: &EntryVersion,
) -> Result<String, CliError> {
    // An absent entry file ⇒ a first publish. A present-but-malformed entry is a
    // read error we surface rather than silently overwrite.
    let existing: Option<IndexEntry> = if index::entry_file_exists(index_root, name) {
        Some(index::read_entry(index_root, name)?)
    } else {
        None
    };

    let mut versions: Vec<EntryVersion> = existing.map(|e| e.versions).unwrap_or_default();
    if versions.iter().any(|v| v.version == new_version.version) {
        return Err(refuse(Refusal::DuplicateVersion {
            name: name.to_owned(),
            version: new_version.version.to_string(),
        }));
    }
    versions.push(new_version.clone());

    Ok(render_entry(name, publisher, &versions))
}

/// The intended pull request — everything publish would push, so `--dry-run` can
/// print it and the network path can act on it.
struct PrPlan {
    index_repo: String,
    entry_file: String,
    branch: String,
    title: String,
}

/// Print the computed entry and the intended PR, touching no network.
fn print_dry_run(entry_toml: &str, plan: &PrPlan) {
    let toml_block = if entry_toml.ends_with('\n') {
        entry_toml.to_owned()
    } else {
        format!("{entry_toml}\n")
    };
    let body = format!(
        "ipe package publish --dry-run: computed index entry\n\
         \n\
         --- {} ---\n\
         {toml_block}\
         \n\
         --- intended pull request ---\n\
           target repo: {}\n\
           branch:      {}\n\
           file:        {}\n\
           title:       {}\n\
         \n\
         No network was touched (--dry-run).",
        plan.entry_file, plan.index_repo, plan.branch, plan.entry_file, plan.title,
    );
    print!("{}", crate::style::frame(&crate::style::gutter(&body)));
}

/// Open the index PR over the network. Thin and non-privileged: it requires a
/// `GITHUB_TOKEN` and never stores a credential of its own.
///
/// # Errors
/// [`CliError::Publish`] when no token is available (the actionable instruction
/// to set one or use `--dry-run`); [`CliError::UsageOwned`] otherwise, until the
/// networked path is wired (a separate ticket — publish holds no OAuth flow).
fn open_pr(_entry_toml: &str, _plan: &PrPlan) -> Result<(), CliError> {
    // The network path is gated on an explicit token: publish holds no index
    // credentials and does not implement an OAuth device flow (that is a separate
    // ticket). Absent a token, refuse with a clear instruction rather than
    // silently doing nothing.
    if std::env::var_os("GITHUB_TOKEN").is_none() {
        return Err(refuse(Refusal::MissingToken));
    }
    // With a token present, the API call to create the branch + PR is the
    // headless path the plan describes. It is intentionally not exercised by the
    // test suite (which must never open a real PR); the offline-testable contract
    // is the `--dry-run` path above.
    Err(CliError::UsageOwned(
        "ipe package publish: networked publishing over `GITHUB_TOKEN` is not wired yet — \
         re-run with `--dry-run` to produce the entry and open the PR by hand."
            .to_owned(),
    ))
}

/// Derive a plausible `publisher` from a GitHub source URL (the `owner` segment
/// of `github.com/<owner>/<repo>`), falling back to `unknown` when the URL is not
/// a recognised GitHub URL. Informational only — the index CI binds the
/// authoritative publisher to the authenticated PR account.
fn infer_publisher(source: &str) -> String {
    github_owner(source).unwrap_or_else(|| "unknown".to_owned())
}

/// The `<owner>` of a `github.com/<owner>/<repo>` URL, for either the `https://`
/// or `git@` form. `None` when the URL is not a GitHub URL.
fn github_owner(source: &str) -> Option<String> {
    let after_host = source
        .split_once("github.com/")
        .or_else(|| source.split_once("github.com:"))
        .map(|(_, rest)| rest)?;
    let owner = after_host.split('/').next()?;
    if owner.is_empty() {
        None
    } else {
        Some(owner.to_owned())
    }
}

// ===========================================================================
// git introspection — the package's source repository
// ===========================================================================

/// The `origin` remote's fetch URL for the git repo at `root`, or `None` when
/// there is no such remote (or no git repo).
fn git_remote_url(root: &Path) -> Result<Option<String>, CliError> {
    let out = run_git_capture(root, &["remote", "get-url", "origin"])?;
    Ok(out.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()))
}

/// Whether the working tree at `root` has any uncommitted change (a non-empty
/// `git status --porcelain`).
fn git_tree_is_dirty(root: &Path) -> Result<bool, CliError> {
    let out = run_git_capture(root, &["status", "--porcelain"])?.ok_or_else(|| not_a_repo(root))?;
    Ok(!out.trim().is_empty())
}

/// The full commit id of HEAD at `root`.
fn git_head_rev(root: &Path) -> Result<String, CliError> {
    let out = run_git_capture(root, &["rev-parse", "HEAD"])?.ok_or_else(|| not_a_repo(root))?;
    Ok(out.trim().to_owned())
}

/// Whether `rev` is reachable from at least one remote-tracking branch — i.e.
/// the commit has been pushed and a consumer could fetch it.
fn git_rev_is_pushed(root: &Path, rev: &str) -> Result<bool, CliError> {
    let out = run_git_capture(root, &["branch", "-r", "--contains", rev])?
        .ok_or_else(|| not_a_repo(root))?;
    Ok(!out.trim().is_empty())
}

/// A "not a git repository" resolve error, the shared fallback when a git
/// introspection command cannot run at `root`.
fn not_a_repo(root: &Path) -> CliError {
    CliError::Resolve(format!(
        "ipe package publish: `{}` is not a git repository — publish pins a committed, pushed \
         revision, so the package must live in a git repo (or pass `--source`/`--rev`).",
        root.display()
    ))
}

/// Run `git <args>` in `root`, returning its stdout on success, `None` when git
/// exits non-zero (the "no such remote / not a repo" signal the caller
/// interprets), and a resolve error only when git cannot be spawned at all.
fn run_git_capture(root: &Path, args: &[&str]) -> Result<Option<String>, CliError> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| CliError::Resolve(format!("ipe package publish: could not run `git`: {e}")))?;
    if output.status.success() {
        Ok(Some(String::from_utf8_lossy(&output.stdout).into_owned()))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ipe_ir::Capability;
    use std::collections::BTreeSet;

    fn caps(names: &[Capability]) -> BTreeSet<Capability> {
        names.iter().copied().collect()
    }

    fn sample_version(v: &str, caps_set: BTreeSet<Capability>) -> EntryVersion {
        EntryVersion {
            version: semver::Version::parse(v).expect("valid version"),
            source: "https://github.com/arthurmaciel/http-extras".to_owned(),
            rev: "9f2c7b1e0a4d5c6f8b2a1e3d4c5b6a7f8e9d0c1b".to_owned(),
            sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_owned(),
            capabilities: caps_set,
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ipe-publish-test-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    /// The rendered single entry parses back through the index reader into an
    /// identical `EntryVersion` — the authoring contract the reader defines.
    #[test]
    fn a_rendered_entry_round_trips_through_the_reader() {
        let root = temp_dir("round-trip");
        let packages = root.join("packages");
        std::fs::create_dir_all(&packages).expect("packages dir");

        let version = sample_version("1.2.0", caps(&[Capability::Network]));
        let toml = render_entry(
            "http-extras",
            "arthurmaciel",
            std::slice::from_ref(&version),
        );
        std::fs::write(packages.join("http-extras.toml"), &toml).expect("write entry");

        let parsed = index::read_entry(&root, "http-extras").expect("entry parses");
        assert_eq!(parsed.name, "http-extras");
        assert_eq!(parsed.publisher, "arthurmaciel");
        assert_eq!(parsed.versions, vec![version]);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Every capability wire name survives the render → read round-trip.
    #[test]
    fn capabilities_round_trip_including_native_ffi() {
        let root = temp_dir("caps-round-trip");
        let packages = root.join("packages");
        std::fs::create_dir_all(&packages).expect("packages dir");

        let version = sample_version(
            "0.1.0",
            caps(&[
                Capability::Network,
                Capability::NativeFfi,
                Capability::Clock,
            ]),
        );
        let toml = render_entry("risky", "arthurmaciel", std::slice::from_ref(&version));
        std::fs::write(packages.join("risky.toml"), &toml).expect("write entry");

        let parsed = index::read_entry(&root, "risky").expect("entry parses");
        let only = parsed.versions.first().expect("one version");
        assert_eq!(only.capabilities, version.capabilities);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A version with no capabilities renders an empty array and round-trips to
    /// the empty set (the reader's "absent/empty ⇒ no capabilities" contract).
    #[test]
    fn no_capabilities_round_trips_to_the_empty_set() {
        let root = temp_dir("no-caps");
        std::fs::create_dir_all(root.join("packages")).expect("packages dir");
        let version = sample_version("1.0.0", caps(&[]));
        let toml = render_entry("pure", "arthurmaciel", std::slice::from_ref(&version));
        std::fs::write(root.join("packages").join("pure.toml"), &toml).expect("write");
        let parsed = index::read_entry(&root, "pure").expect("entry parses");
        let only = parsed.versions.first().expect("one version");
        assert!(only.capabilities.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Appending a version preserves the prior one and both round-trip; the file
    /// is re-rendered in ascending version order.
    #[test]
    fn appending_a_version_preserves_the_prior_one() {
        let root = temp_dir("append");
        let packages = root.join("packages");
        std::fs::create_dir_all(&packages).expect("packages dir");

        let v1 = sample_version("1.2.0", caps(&[Capability::Network]));
        std::fs::write(
            packages.join("http-extras.toml"),
            render_entry("http-extras", "arthurmaciel", std::slice::from_ref(&v1)),
        )
        .expect("write first entry");

        let v2 = sample_version("1.3.0", caps(&[Capability::Network, Capability::Clock]));
        let merged =
            merge_into_entry(&root, "http-extras", "arthurmaciel", &v2).expect("merge appends");
        std::fs::write(packages.join("http-extras.toml"), &merged).expect("rewrite entry");

        let parsed = index::read_entry(&root, "http-extras").expect("entry parses");
        let versions: Vec<String> = parsed
            .versions
            .iter()
            .map(|v| v.version.to_string())
            .collect();
        assert_eq!(versions, vec!["1.2.0", "1.3.0"]);
        assert!(parsed.versions.contains(&v1), "prior version preserved");
        assert!(parsed.versions.contains(&v2), "new version appended");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A first publish (no existing entry file) produces a valid, parseable
    /// entry.
    #[test]
    fn a_first_publish_creates_the_entry() {
        let root = temp_dir("first");
        std::fs::create_dir_all(root.join("packages")).expect("packages dir");

        let v = sample_version("0.1.0", caps(&[]));
        let toml = merge_into_entry(&root, "brand-new", "arthurmaciel", &v).expect("first publish");
        std::fs::write(root.join("packages").join("brand-new.toml"), &toml).expect("write");

        let parsed = index::read_entry(&root, "brand-new").expect("entry parses");
        assert_eq!(parsed.versions.len(), 1);
        let only = parsed.versions.first().expect("one version");
        assert_eq!(only.version.to_string(), "0.1.0");
        assert!(only.capabilities.is_empty());

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Re-publishing an already-published version is a typed refusal, never a
    /// silent overwrite.
    #[test]
    fn a_duplicate_version_is_a_typed_refusal() {
        let root = temp_dir("dup");
        let packages = root.join("packages");
        std::fs::create_dir_all(&packages).expect("packages dir");

        let v1 = sample_version("1.2.0", caps(&[Capability::Network]));
        std::fs::write(
            packages.join("http-extras.toml"),
            render_entry("http-extras", "arthurmaciel", std::slice::from_ref(&v1)),
        )
        .expect("write entry");

        let dup = sample_version("1.2.0", caps(&[Capability::Network]));
        let err = merge_into_entry(&root, "http-extras", "arthurmaciel", &dup).unwrap_err();
        assert!(matches!(
            err,
            CliError::Publish(Refusal::DuplicateVersion { .. })
        ));
        assert!(format!("{err}").contains("already published"));

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The `--dry-run` path prints the entry and the intended PR from pure
    /// artifacts (no network). Asserts the rendered TOML + plan it consumes, then
    /// exercises the printer itself.
    #[test]
    fn dry_run_prints_the_entry_and_pr_plan() {
        let version = sample_version("1.2.0", caps(&[Capability::Network]));
        let toml = render_entry(
            "http-extras",
            "arthurmaciel",
            std::slice::from_ref(&version),
        );
        let plan = PrPlan {
            index_repo: DEFAULT_INDEX_REPO.to_owned(),
            entry_file: "packages/http-extras.toml".to_owned(),
            branch: "publish/http-extras-1.2.0".to_owned(),
            title: "Publish http-extras 1.2.0".to_owned(),
        };
        assert!(toml.contains("[[version]]"));
        assert!(toml.contains("version = \"1.2.0\""));
        assert_eq!(plan.index_repo, DEFAULT_INDEX_REPO);
        assert_eq!(plan.entry_file, "packages/http-extras.toml");
        print_dry_run(&toml, &plan);
    }

    /// The networked path refuses with a clear instruction when no token is set —
    /// a typed refusal, not a panic. Skipped when a runner sets `GITHUB_TOKEN`.
    #[test]
    fn open_pr_without_a_token_is_a_typed_refusal() {
        if std::env::var_os("GITHUB_TOKEN").is_some() {
            return;
        }
        let plan = PrPlan {
            index_repo: DEFAULT_INDEX_REPO.to_owned(),
            entry_file: "packages/x.toml".to_owned(),
            branch: "publish/x-1.0.0".to_owned(),
            title: "Publish x 1.0.0".to_owned(),
        };
        let err = open_pr("name = \"x\"\n", &plan).unwrap_err();
        assert!(matches!(err, CliError::Publish(Refusal::MissingToken)));
        assert!(format!("{err}").contains("GITHUB_TOKEN"));
    }

    #[test]
    fn github_owner_is_extracted_for_both_url_forms() {
        assert_eq!(
            github_owner("https://github.com/arthurmaciel/http-extras").as_deref(),
            Some("arthurmaciel")
        );
        assert_eq!(
            github_owner("git@github.com:arthurmaciel/http-extras.git").as_deref(),
            Some("arthurmaciel")
        );
        assert_eq!(github_owner("https://example.invalid/x"), None);
    }

    #[test]
    fn unknown_flag_is_a_usage_error() {
        let err = parse_args(&["--nope".to_owned()]).unwrap_err();
        assert!(matches!(err, CliError::UsageOwned(_)));
    }

    #[test]
    fn a_missing_flag_value_is_a_usage_error() {
        let err = parse_args(&["--index".to_owned()]).unwrap_err();
        assert!(matches!(err, CliError::UsageOwned(_)));
    }
}
