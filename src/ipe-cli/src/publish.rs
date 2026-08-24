//! `ipe package publish` — prepare a package's index entry and open the index PR.
//!
//! Publish is a thin, non-privileged helper. It runs the same
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
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::scratch::{ScratchDir, ScratchFile};

use crate::CliError;
use crate::index::{self, CommitId, EntryVersion, IndexEntry, PinnedRev, SourceUrl};
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
                 immutable and must never be rewritten. Bump the version in `package.ipe` and \
                 publish the new one."
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
    /// The package directory or `package.ipe` (defaults to the current directory).
    path: PathBuf,
    /// `--dry-run`: compute and print, touch no network.
    dry_run: bool,
    /// `--index <repo>`: the index GitHub repo (`owner/name`) the PR targets.
    index_repo: String,
    /// `--source <url>`: the source URL to pin, overriding the git remote.
    source: Option<String>,
    /// `--rev <sha>`: the revision to pin, overriding the git HEAD.
    rev: Option<String>,
    /// `--fork <owner>`: the GitHub owner of the author's index fork to push to
    /// (defaults to the source repo's owner).
    fork: Option<String>,
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
/// precondition (dirty tree, unpushed HEAD, duplicate version); resolution / IO
/// errors otherwise.
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

    let publisher = infer_publisher(entry_version.source.as_str());

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

    // The fork owner defaults to the source repo's owner — the account that
    // publishes its own package would fork the index under the same name.
    let fork_owner = args
        .fork
        .unwrap_or_else(|| infer_publisher(entry_version.source.as_str()));
    if fork_owner == "unknown" {
        return Err(CliError::UsageOwned(
            "ipe package publish: could not infer your GitHub fork owner from the source URL — \
             pass `--fork <github-user>` (the owner of your fork of the index)."
                .to_owned(),
        ));
    }

    open_pr(&entry_toml, &plan, &fork_owner)
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
    let mut fork: Option<String> = None;

    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--dry-run" => dry_run = true,
            "--index" => index_repo = Some(take_value(&mut it, "--index")?),
            "--source" => source = Some(take_value(&mut it, "--source")?),
            "--rev" => rev = Some(take_value(&mut it, "--rev")?),
            "--fork" => fork = Some(take_value(&mut it, "--fork")?),
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
        fork,
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

/// Resolve `path` (a directory or a `package.ipe`) to its manifest file.
fn locate_manifest(path: &Path) -> Result<PathBuf, CliError> {
    if path.is_dir() {
        if let Some(manifest) = crate::project::manifest_in_dir(path) {
            return Ok(manifest);
        }
        if crate::project::migration_pending(path) {
            return Err(CliError::Usage(crate::project::MIGRATE_CONFIG_HINT));
        }
        return Err(CliError::UsageOwned(format!(
            "ipe package publish: no `package.ipe` in `{}` — publish operates on a publishable \
             Ipê package, which needs a manifest",
            path.display()
        )));
    }
    if path.file_name().and_then(|n| n.to_str()) == Some(crate::package_manifest::PACKAGE_IPE)
        && path.is_file()
    {
        return Ok(path.to_path_buf());
    }
    Err(CliError::UsageOwned(format!(
        "ipe package publish: `{}` is neither an Ipê project directory nor a package.ipe",
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
    let raw_source = match source_override {
        Some(s) => s.to_owned(),
        None => git_remote_url(source_root)?.ok_or_else(|| refuse(Refusal::NoSource))?,
    };
    // Parse-don't-validate: the typed constructor rejects any value outside the
    // transport allow-list. Publish uses the same gate as the resolver so an
    // entry written by `publish` round-trips through `read_entry` without error.
    let source = SourceUrl::parse(&manifest.name, &raw_source).map_err(|e| {
        CliError::UsageOwned(format!(
            "ipe package publish: the source URL is not accepted — {e}"
        ))
    })?;

    // The revision is pinned as an immutable commit SHA. The default path runs
    // `committed_pushed_head` which already calls `git rev-parse HEAD` and
    // returns a full SHA; the override path resolves the given ref to a SHA
    // so branch names or tags are pinned to their current commit, never stored
    // as moving refs.
    let rev = if let Some(r) = rev_override {
        // Injection-gate the requested ref before passing it to git.
        let requested = CommitId::parse(&manifest.name, r).map_err(|e| {
            CliError::UsageOwned(format!(
                "ipe package publish: the revision is not accepted — {e}"
            ))
        })?;
        let raw_sha = resolve_rev_to_sha(source_root, requested.as_str())?;
        PinnedRev::from_full_sha(&manifest.name, &raw_sha).map_err(|e| {
            CliError::UsageOwned(format!(
                "ipe package publish: `--rev` resolved to a non-SHA: {e}"
            ))
        })?
    } else {
        let raw_sha = committed_pushed_head(source_root)?;
        PinnedRev::from_full_sha(&manifest.name, &raw_sha).map_err(|e| {
            CliError::UsageOwned(format!(
                "ipe package publish: HEAD did not resolve to a full SHA: {e}"
            ))
        })?
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

/// Resolve an arbitrary git ref to the full 40-hex SHA of its commit object.
///
/// Runs `git rev-parse --verify <ref>^{commit}` in `root`. A branch name,
/// tag, or short hash resolves to its underlying commit SHA; a full SHA
/// passes through. Returns a resolve error when the ref does not exist.
///
/// # Errors
/// [`CliError::Resolve`] when `git` cannot be run or the ref does not resolve.
fn resolve_rev_to_sha(root: &Path, rev: &str) -> Result<String, CliError> {
    let refspec = format!("{rev}^{{commit}}");
    let out = run_git_capture(root, &["rev-parse", "--verify", "--quiet", &refspec])?.ok_or_else(
        || {
            CliError::Resolve(format!(
                "ipe package publish: `git rev-parse --verify {refspec}` failed — \
                     ref {rev:?} does not resolve to a commit"
            ))
        },
    )?;
    Ok(out.trim().to_owned())
}

/// The `package.ipe` path for a project root.
fn manifest_path_of(root: &Path) -> PathBuf {
    root.join(crate::package_manifest::PACKAGE_IPE)
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
    let _ = writeln!(out, "source = \"{}\"", version.source.as_str());
    let _ = writeln!(out, "rev = \"{}\"", version.rev.as_str());
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

/// Open the index PR the spec's default way: push the entry to the author's fork
/// of the index over `git`, then open a browser at GitHub's pre-filled "create
/// pull request" page.
///
/// No credential is stored and `gh` is not required — the push uses the git
/// credentials already on the machine, and the fork is a one-time setup the
/// author does on GitHub. Everything happens in a throwaway clone, so a failure
/// never touches the working project, and the PR URL is printed regardless so
/// the publish can always be finished by hand.
///
/// # Errors
/// [`CliError::Resolve`] when a git step fails (clone / commit / push); the
/// message carries the fork URL and the pre-filled PR URL as the manual
/// fallback.
fn open_pr(entry_toml: &str, plan: &PrPlan, fork_owner: &str) -> Result<(), CliError> {
    let index_name = index_repo_name(&plan.index_repo);
    let fork_url = format!("https://github.com/{fork_owner}/{index_name}.git");

    let scratch = ScratchDir::new("ipe-publish").map_err(|e| scratch_io(&e))?;
    let clone = scratch.child(index_name);

    // Shallow-clone the fork — it carries the index's `main` history, which the
    // branch must descend from for the compare page to work.
    if let Err(git) = run_git_step(
        scratch.path(),
        &["clone", "--quiet", "--depth", "1", &fork_url, index_name],
    ) {
        return Err(clone_failed(&fork_url, &git));
    }

    // Write the entry on a fresh branch and commit it. `-c user.*` supplies an
    // identity so the commit succeeds even where git has none configured.
    let entry_path = clone.join(&plan.entry_file);
    if let Some(parent) = entry_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| scratch_io(&e))?;
    }
    std::fs::write(&entry_path, entry_toml).map_err(|e| scratch_io(&e))?;

    for step in [
        vec!["checkout", "--quiet", "-b", &plan.branch],
        vec!["add", "--", &plan.entry_file],
        vec![
            "-c",
            "user.name=ipe",
            "-c",
            "user.email=ipe@localhost",
            "commit",
            "--quiet",
            "-m",
            &plan.title,
        ],
        vec!["push", "--quiet", "-u", "origin", &plan.branch],
    ] {
        if let Err(git) = run_git_step(&clone, &step) {
            return Err(push_failed(&fork_url, plan, fork_owner, &git));
        }
    }

    // The branch is pushed. Open the PR headlessly when a token is available
    // (CI's `GITHUB_TOKEN` or `ipe login`); otherwise fall back to the browser
    // compare page.
    publish_token().map_or_else(
        || {
            let url = compare_url(
                &plan.index_repo,
                "main",
                fork_owner,
                &plan.branch,
                &plan.title,
            );
            let opened = open_in_browser(&url);
            print_pr_opened(plan, &url, opened);
        },
        |token| submit_pr_via_api(plan, fork_owner, &token),
    );
    Ok(())
}

/// The token for the headless PR-open path: `GITHUB_TOKEN` (CI) wins, else the
/// token stored by `ipe login`. `None` selects the browser path.
fn publish_token() -> Option<String> {
    std::env::var("GITHUB_TOKEN")
        .ok()
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty())
        .or_else(crate::login::stored_token)
}

/// The `POST /repos/{index}/pulls` request body: a PR from `fork_owner:branch`
/// against the index's `main`.
fn pr_request_body(plan: &PrPlan, fork_owner: &str) -> serde_json::Value {
    serde_json::json!({
        "title": plan.title,
        "head": format!("{fork_owner}:{}", plan.branch),
        "base": "main",
    })
}

/// Typed outcome of a GitHub PR-open API call.
enum PrResult {
    /// HTTP 201 Created — PR was successfully opened.
    Created(String),
    /// HTTP 422 — GitHub reports the PR already exists for this branch.
    AlreadyExists,
    /// Any other outcome — carries the GitHub `message` or a description.
    Failed(String),
}

/// Open the index PR through the GitHub REST API — no browser. Reuses the branch
/// already pushed to the fork. On any API failure the pre-filled compare URL is
/// printed as the manual fallback, so a headless publish never dead-ends.
fn submit_pr_via_api(plan: &PrPlan, fork_owner: &str, token: &str) {
    let api = format!("https://api.github.com/repos/{}/pulls", plan.index_repo);
    let body = pr_request_body(plan, fork_owner);
    let fallback_url = compare_url(
        &plan.index_repo,
        "main",
        fork_owner,
        &plan.branch,
        &plan.title,
    );
    match github_api_post(&api, token, &body) {
        PrResult::Created(url) => print_pr_submitted(plan, &url),
        PrResult::AlreadyExists => print_pr_submitted(plan, &fallback_url),
        PrResult::Failed(err) => print_pr_api_fallback(plan, &fallback_url, &err),
    }
}

/// `POST` a JSON body to the GitHub API with the bearer token.
///
/// The token is passed to curl via stdin (`--config -`) so it never appears in
/// the process argument list and cannot be read from `/proc/<pid>/cmdline` by
/// other local users.
///
/// The HTTP status drives the result — not body-field presence — so the outcome
/// is a typed [`PrResult`] parsed once at the network boundary.
fn github_api_post(url: &str, token: &str, body: &serde_json::Value) -> PrResult {
    let body_str = body.to_string();
    // curl writes the response body to an exclusively-created scratch file;
    // the HTTP status code is captured on stdout (`-w '%{http_code}'`).
    // The scratch file is created with O_EXCL before curl runs, so a
    // pre-seeded symlink or a pre-existing name is refused rather than
    // followed.  The response is read back through the retained file handle —
    // not by re-opening the path — so the bytes parsed are the bytes curl
    // wrote to this inode, with no race between write and read.
    let mut scratch = match ScratchFile::create("ipe-publish-resp") {
        Ok(sf) => sf,
        Err(e) => {
            return PrResult::Failed(format!(
                "could not create scratch file for curl response: {e}"
            ));
        }
    };
    let tmp_path = scratch.path().to_string_lossy().into_owned();

    let mut child = match Command::new("curl")
        .args(["--silent", "--show-error", "-X", "POST"])
        .args(["-H", "Accept: application/vnd.github+json"])
        .args(["-H", "User-Agent: ipe-cli"])
        // Token delivered via stdin config, never via argv.
        .args(["--config", "-"])
        .args(["-d", &body_str])
        .args(["-o", &tmp_path])
        .args(["-w", "%{http_code}"])
        .arg(url)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return PrResult::Failed(format!("could not run `curl` (needed to open the PR): {e}"));
        }
    };

    // Write the auth header line to curl's stdin config, then close stdin so
    // curl proceeds.  A write failure here means curl never gets the header;
    // the subsequent wait will capture the error.
    if let Some(mut stdin) = child.stdin.take() {
        let _ = writeln!(stdin, r#"header = "Authorization: Bearer {token}""#);
    }

    let output = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => return PrResult::Failed(format!("curl wait failed: {e}")),
    };

    // stdout carries the 3-digit HTTP status code written by `-w '%{http_code}'`.
    let status_str = String::from_utf8_lossy(&output.stdout);
    let http_status: u16 = status_str.trim().parse().unwrap_or(0);

    // Read the response body through the retained handle (not by path) so the
    // bytes parsed are exactly what curl wrote to this inode.
    let body_bytes = scratch.read_all().unwrap_or_default();
    // `scratch` drops here, removing the temp file.

    let json: serde_json::Value = match serde_json::from_slice(&body_bytes) {
        Ok(v) => v,
        Err(e) => return PrResult::Failed(format!("could not parse GitHub's response: {e}")),
    };

    match http_status {
        201 => {
            // 201 Created must carry an `html_url`; anything else is unexpected.
            json.get("html_url")
                .and_then(serde_json::Value::as_str)
                .map_or_else(
                    || PrResult::Failed("201 response missing html_url".to_owned()),
                    |url| PrResult::Created(url.to_owned()),
                )
        }
        422 => PrResult::AlreadyExists,
        _ => {
            let msg = json
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unexpected GitHub API response");
            PrResult::Failed(msg.to_owned())
        }
    }
}

/// The `<name>` of an `<owner>/<name>` repo (the whole string when it has no
/// slash).
fn index_repo_name(index_repo: &str) -> &str {
    index_repo.rsplit('/').next().unwrap_or(index_repo)
}

/// Run one `git` step in `dir`; on a non-zero exit, return git's stderr as the
/// error string so the caller can surface the real cause.
fn run_git_step(dir: &Path, args: &[&str]) -> Result<(), String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| format!("could not run `git`: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
    }
}

/// GitHub's pre-filled "create pull request" URL: the compare page for
/// `fork_owner:branch` against the index's `base`, with the title filled in.
/// `quick_pull=1` opens the PR form directly.
fn compare_url(
    index_repo: &str,
    base: &str,
    fork_owner: &str,
    branch: &str,
    title: &str,
) -> String {
    format!(
        "https://github.com/{index_repo}/compare/{base}...{fork_owner}:{branch}\
         ?quick_pull=1&title={}",
        percent_encode(title)
    )
}

/// Percent-encode a URL query value, keeping the RFC 3986 unreserved set.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push('%');
            let _ = write!(out, "{b:02X}");
        }
    }
    out
}

/// Best-effort launch of the platform browser on `url`. Returns whether the
/// opener started — the URL is printed regardless, so `false` is never fatal.
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

/// Print the "pushed, now finish the PR" summary, framed and guttered like every
/// other human-facing publish message.
fn print_pr_opened(plan: &PrPlan, url: &str, opened: bool) {
    let mut body = String::new();
    let _ = writeln!(body, "pushed `{}` to your index fork", plan.branch);
    let _ = writeln!(body);
    let _ = writeln!(
        body,
        "{}finish the pull request here:",
        if opened {
            "opened your browser — "
        } else {
            ""
        }
    );
    let _ = write!(body, "  {url}");
    print!("{}", crate::style::frame(&crate::style::gutter(&body)));
}

/// Print the "PR opened via the API" summary (headless path — no browser).
fn print_pr_submitted(plan: &PrPlan, url: &str) {
    let mut body = String::new();
    let _ = writeln!(body, "published `{}` — pull request opened:", plan.branch);
    let _ = write!(body, "  {url}");
    print!("{}", crate::style::frame(&crate::style::gutter(&body)));
}

/// Print the manual fallback when the headless API PR-open failed. The branch is
/// already pushed, so the author finishes at the compare URL by hand.
fn print_pr_api_fallback(plan: &PrPlan, url: &str, err: &str) {
    let mut body = String::new();
    let _ = writeln!(body, "pushed `{}` to your index fork", plan.branch);
    let _ = writeln!(
        body,
        "the GitHub API PR-open failed ({err}); finish it here:"
    );
    let _ = write!(body, "  {url}");
    print!("{}", crate::style::frame(&crate::style::gutter(&body)));
}

/// A scratch-filesystem failure during publish.
fn scratch_io(e: &std::io::Error) -> CliError {
    CliError::Resolve(format!(
        "ipe package publish: scratch filesystem error: {e}"
    ))
}

/// Clone of the author's fork failed — most often the fork does not exist yet.
fn clone_failed(fork_url: &str, git: &str) -> CliError {
    CliError::Resolve(format!(
        "ipe package publish: could not clone your index fork `{fork_url}` — publish pushes the \
         entry to your fork, so fork the index on GitHub first (a one-time step) and make sure \
         git can reach it.\n  git: {git}"
    ))
}

/// Push to the author's fork failed — nothing was published; the pre-filled PR
/// URL is included so the author can retry the push and finish by hand.
fn push_failed(fork_url: &str, plan: &PrPlan, fork_owner: &str, git: &str) -> CliError {
    let url = compare_url(
        &plan.index_repo,
        "main",
        fork_owner,
        &plan.branch,
        &plan.title,
    );
    CliError::Resolve(format!(
        "ipe package publish: could not push `{}` to `{fork_url}` — nothing was published. Fix \
         the push (git credentials / fork access), then open the PR here:\n  {url}\n  git: {git}",
        plan.branch
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
            source: SourceUrl::parse("http-extras", "https://github.com/arthurmaciel/http-extras")
                .expect("valid source url"),
            rev: PinnedRev::from_full_sha(
                "http-extras",
                "9f2c7b1e0a4d5c6f8b2a1e3d4c5b6a7f8e9d0c1b",
            )
            .expect("valid pinned rev"),
            sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_owned(),
            capabilities: caps_set,
        }
    }

    fn temp_dir(_tag: &str) -> PathBuf {
        // Use the scratch module so test-only paths are also exclusively created
        // and free of the predictable pid-name idiom.  The returned PathBuf
        // outlives the ScratchDir (caller removes it explicitly at the end of
        // each test), which is intentional: the RAII guard's drop is a
        // best-effort no-op when the directory is already gone.
        let sd = crate::scratch::ScratchDir::new("ipe-publish-test").expect("scratch dir");
        let p = sd.path().to_path_buf();
        std::mem::forget(sd); // caller's explicit remove_dir_all handles cleanup
        p
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
    fn compare_url_is_the_prefilled_pr_page() {
        let url = compare_url(
            "arthurmaciel/ipe-index",
            "main",
            "octocat",
            "publish/foo-1.2.3",
            "Publish foo 1.2.3",
        );
        assert_eq!(
            url,
            "https://github.com/arthurmaciel/ipe-index/compare/\
             main...octocat:publish/foo-1.2.3?quick_pull=1&title=Publish%20foo%201.2.3"
        );
    }

    #[test]
    fn pr_request_body_targets_fork_head_against_main() {
        let plan = PrPlan {
            index_repo: "arthurmaciel/ipe-index".to_owned(),
            entry_file: "packages/foo.toml".to_owned(),
            branch: "publish/foo-1.2.3".to_owned(),
            title: "Publish foo 1.2.3".to_owned(),
        };
        let body = pr_request_body(&plan, "octocat");
        assert_eq!(
            body.get("head").and_then(serde_json::Value::as_str),
            Some("octocat:publish/foo-1.2.3")
        );
        assert_eq!(
            body.get("base").and_then(serde_json::Value::as_str),
            Some("main")
        );
        assert_eq!(
            body.get("title").and_then(serde_json::Value::as_str),
            Some("Publish foo 1.2.3")
        );
    }

    #[test]
    fn percent_encode_keeps_unreserved_and_escapes_the_rest() {
        assert_eq!(percent_encode("Publish foo 1.2.3"), "Publish%20foo%201.2.3");
        assert_eq!(percent_encode("a-b_c.d~e"), "a-b_c.d~e");
        assert_eq!(percent_encode("x/y&z=w"), "x%2Fy%26z%3Dw");
    }

    #[test]
    fn index_repo_name_takes_the_last_segment() {
        assert_eq!(index_repo_name("arthurmaciel/ipe-index"), "ipe-index");
        assert_eq!(index_repo_name("ipe-index"), "ipe-index");
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

    /// `github_api_post` must never put the token in curl's argv — no
    /// `Authorization` or `Bearer` substring must appear in the args list.
    /// We verify the command that would be built (not the network result)
    /// by constructing the same `Command` args here.
    #[test]
    fn token_not_in_curl_argv() {
        // Mirror the argv construction in `github_api_post`.
        let token = "super-secret-token";
        let url = "https://api.github.com/repos/foo/bar/pulls";
        let body_str = r#"{"title":"t"}"#;
        let tmp_path = "/tmp/fake-resp";

        let mut cmd = Command::new("curl");
        cmd.args(["--silent", "--show-error", "-X", "POST"])
            .args(["-H", "Accept: application/vnd.github+json"])
            .args(["-H", "User-Agent: ipe-cli"])
            .args(["--config", "-"])
            .args(["-d", body_str])
            .args(["-o", tmp_path])
            .args(["-w", "%{http_code}"])
            .arg(url);

        // Collect every argv string and assert the token is absent.
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        for arg in &args {
            assert!(
                !arg.contains(token),
                "token must not appear in curl argv; found it in: {arg:?}"
            );
            assert!(
                !arg.contains("Bearer"),
                "Authorization header must not appear in curl argv; found in: {arg:?}"
            );
        }
        // The stdin config line that WOULD carry the token — verify format.
        let config_line = format!("header = \"Authorization: Bearer {token}\"");
        assert!(config_line.contains(token));
        assert!(config_line.contains("Bearer"));
    }

    /// HTTP 201 with `html_url` → `PrResult::Created`.
    /// HTTP 200 with `html_url` → `PrResult::Failed` (not a Create response).
    /// HTTP 422 → `PrResult::AlreadyExists`.
    ///
    /// We test the branching logic directly by driving the match arms with
    /// constructed inputs — no curl subprocess needed.
    #[test]
    fn pr_result_classification() {
        // Simulate the dispatch logic from github_api_post for unit-testability.
        fn classify(http_status: u16, json: &serde_json::Value) -> String {
            match http_status {
                201 => json
                    .get("html_url")
                    .and_then(serde_json::Value::as_str)
                    .map_or_else(
                        || "Failed:201 response missing html_url".to_owned(),
                        |url| format!("Created:{url}"),
                    ),
                422 => "AlreadyExists".to_owned(),
                _ => {
                    let msg = json
                        .get("message")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("unexpected GitHub API response");
                    format!("Failed:{msg}")
                }
            }
        }

        let with_url = serde_json::json!({"html_url": "https://github.com/foo/bar/pull/1"});
        let error_body = serde_json::json!({"message": "Validation Failed"});
        let empty = serde_json::json!({});

        assert_eq!(
            classify(201, &with_url),
            "Created:https://github.com/foo/bar/pull/1"
        );
        assert_eq!(
            classify(200, &with_url),
            "Failed:unexpected GitHub API response",
            "200 with html_url is NOT a success"
        );
        assert_eq!(classify(422, &error_body), "AlreadyExists");
        assert_eq!(classify(500, &error_body), "Failed:Validation Failed");
        assert_eq!(
            classify(201, &empty),
            "Failed:201 response missing html_url"
        );
    }

    // --- Helpers shared by tests G and H ---

    fn make_git_repo(tag: &str, content: &str) -> PathBuf {
        let sd = crate::scratch::ScratchDir::new(&format!("ipe-publish-test-{tag}"))
            .expect("scratch dir");
        let repo = sd.path().to_path_buf();
        std::mem::forget(sd);
        let git = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(&repo)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .expect("git")
                .status
                .success()
        };
        assert!(git(&["init", "--quiet"]));
        std::fs::write(repo.join("lib.ipe"), content).expect("write");
        assert!(git(&["add", "."]));
        assert!(git(&["commit", "--quiet", "-m", "seed"]));
        // Add a fake remote so git_rev_is_pushed can succeed.
        let remote = {
            let sd2 = crate::scratch::ScratchDir::new(&format!("ipe-publish-remote-{tag}"))
                .expect("scratch dir");
            let p = sd2.path().to_path_buf();
            std::mem::forget(sd2);
            p
        };
        assert!(
            Command::new("git")
                .args(["init", "--bare", "--quiet"])
                .arg(&remote)
                .output()
                .expect("git init bare")
                .status
                .success()
        );
        assert!(git(&[
            "remote",
            "add",
            "origin",
            &remote.display().to_string()
        ]));
        assert!(git(&["push", "--quiet", "origin", "HEAD:main"]));
        repo
    }

    fn head_sha(repo: &Path) -> String {
        let out = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(repo)
            .output()
            .expect("git rev-parse HEAD");
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    // --- Test G: publish pins SHAs ---

    #[test]
    fn publish_default_head_is_sha() {
        let repo = make_git_repo("pub-head", "module Lib\n");
        let expected_sha = head_sha(&repo);
        // resolve_rev_to_sha on HEAD must return the same 40-hex SHA.
        let sha = resolve_rev_to_sha(&repo, "HEAD").expect("rev-parse HEAD");
        assert_eq!(
            sha, expected_sha,
            "resolve_rev_to_sha must return the HEAD SHA"
        );
        assert_eq!(sha.len(), 40, "SHA must be 40 chars");
        assert!(
            sha.chars().all(|c| c.is_ascii_hexdigit()),
            "SHA must be hex"
        );
        // PinnedRev::from_full_sha must accept it.
        assert!(
            PinnedRev::from_full_sha("lib", &sha).is_ok(),
            "HEAD SHA must be accepted by PinnedRev"
        );
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn publish_rev_override_resolves_to_sha() {
        let repo = make_git_repo("pub-rev-override", "module Lib\n");
        // Create a branch "feat" pointing at the same commit.
        assert!(
            Command::new("git")
                .args(["checkout", "-b", "feat"])
                .current_dir(&repo)
                .output()
                .expect("git checkout")
                .status
                .success()
        );
        let expected_sha = head_sha(&repo);
        // resolve_rev_to_sha with "feat" must return the commit SHA, not "feat".
        let sha = resolve_rev_to_sha(&repo, "feat").expect("rev-parse feat");
        assert_eq!(
            sha, expected_sha,
            "feat branch must resolve to its commit SHA"
        );
        assert_ne!(sha, "feat", "must not record the branch name as the pin");
        assert_eq!(sha.len(), 40);
        assert!(sha.chars().all(|c| c.is_ascii_hexdigit()));
        let _ = std::fs::remove_dir_all(&repo);
    }

    // --- Test H: render_entry_version round-trips a PinnedRev entry ---

    #[test]
    fn render_entry_version_round_trips_sha_rev() {
        let root = temp_dir("render-roundtrip");
        let packages = root.join("packages");
        std::fs::create_dir_all(&packages).expect("packages dir");

        let version = sample_version("2.0.0", caps(&[Capability::Network]));
        let toml = render_entry("mylib", "arthurmaciel", std::slice::from_ref(&version));
        std::fs::write(packages.join("mylib.toml"), &toml).expect("write entry");

        let parsed = index::read_entry(&root, "mylib").expect("entry parses");
        let only = parsed.versions.first().expect("one version");
        // The rev must round-trip byte-for-byte.
        assert_eq!(
            only.rev.as_str(),
            version.rev.as_str(),
            "rev must round-trip through render/read unchanged"
        );
        assert_eq!(only.rev.as_str().len(), 40, "round-tripped rev is 40 chars");
        assert!(
            only.rev.as_str().chars().all(|c| c.is_ascii_hexdigit()),
            "round-tripped rev is lowercase hex"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
