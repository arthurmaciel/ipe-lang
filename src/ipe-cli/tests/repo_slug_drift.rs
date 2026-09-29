//! Repo-slug drift guard.
//!
//! [`ipe_diagnostics::GITHUB_REPO_SLUG`] is the single source of truth for
//! this compiler's GitHub `org/repo` slug. Every tracked file that names it
//! must use that exact spelling; the retired owner slug is refused wherever
//! it appears. Listed exemptions (the maintainer-owned `README.md`,
//! `CHANGELOG.md` history, `docs/adr/` history, hand-generated goldens and
//! doc pages, and this file's own matcher literal) are the only files allowed
//! to carry a different spelling.
//!
//! This is deliberately independent of the registry slug
//! (`arthurmaciel/ipe-registry`, `DEFAULT_INDEX_REPO`,
//! `DEFAULT_REGISTRY_URL`): the registry repos stay under their own owner, so
//! the matcher below looks for the exact retired compiler-repo slug, never a
//! bare owner name that would also catch the registry.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The retired owner/repo spelling this test refuses.
///
/// Exact-substring match only, so it can never false-positive on the
/// unrelated registry slug (`arthurmaciel/ipe-registry`), which shares an
/// owner but not a repo name.
const STALE_SLUG: &str = "arthurmaciel/ipe-lang";

/// Paths allowed to carry the stale slug.
///
/// A bare entry is an exact repo-relative path; a trailing `/` marks a
/// directory prefix. Covers maintainer-owned prose, history, generated/golden
/// output this lane never hand-edits, and this test file's own matcher
/// literal.
const EXEMPT: &[&str] = &[
    "README.md",
    "CHANGELOG.md",
    "docs/adr/",
    "docs/reference/",
    "src/ipe-cli/tests/golden/",
    "src/compiler/diagnostics/tests/render_goldens/",
    "src/ipe-cli/tests/repo_slug_drift.rs",
];

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn is_exempt(rel_path: &str) -> bool {
    EXEMPT.iter().any(|e| {
        if let Some(dir) = e.strip_suffix('/') {
            rel_path.starts_with(dir)
        } else {
            rel_path == *e
        }
    })
}

/// Every path `git` tracks, repo-root-relative and forward-slash separated.
///
/// An unreadable git tree fails loudly here rather than returning an empty
/// list — an empty list would let the sweep below pass vacuously and
/// certify nothing.
fn tracked_files() -> Vec<String> {
    let root = repo_root();
    let out = Command::new("git")
        .args(["ls-files"])
        .current_dir(&root)
        .output();
    let ok = out.as_ref().is_ok_and(|o| o.status.success());
    assert!(ok, "`git ls-files` in {} failed: {out:?}", root.display());
    out.map(|o| {
        String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(str::to_owned)
            .collect()
    })
    .unwrap_or_default()
}

/// Proves the refusal: the matcher actually catches a planted stale slug.
///
/// A rejection no test drives is a rejection one edit away from vanishing
/// unnoticed — this pins that the matcher fires on the exact retired slug and
/// stays silent on the unrelated registry slug.
#[test]
fn matcher_catches_a_planted_stale_slug() {
    let planted = format!("see https://github.com/{STALE_SLUG}/issues for details");
    assert!(
        planted.contains(STALE_SLUG),
        "the matcher itself must detect the slug it plants"
    );

    // Must not fire on the registry slug, which shares an owner but names a
    // different repo.
    let registry = "https://github.com/arthurmaciel/ipe-registry";
    assert!(
        !registry.contains(STALE_SLUG),
        "the matcher must not confuse the registry slug for the compiler repo slug"
    );
}

/// No tracked file outside the listed exemptions may carry the retired slug.
///
/// A hit here means a rename swept some files but missed this one.
#[test]
fn no_tracked_file_carries_the_stale_repo_slug() {
    let root = repo_root();
    let mut offenders = Vec::new();

    for rel in tracked_files() {
        if is_exempt(&rel) {
            continue;
        }
        let path = root.join(&rel);
        // Skip anything that isn't readable UTF-8 text (e.g. a binary
        // fixture) rather than fail the sweep on it.
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        if contents.contains(STALE_SLUG) {
            offenders.push(rel);
        }
    }

    assert!(
        offenders.is_empty(),
        "these tracked files still carry the stale `{STALE_SLUG}` slug (add a listed \
         exemption if this is intentional maintainer-owned/history/generated content): \
         {offenders:#?}"
    );
}

/// The shell/toml mirrors agree with the Rust SSOT.
///
/// `install.sh`, the branch-protection script, the editor installers, and
/// the issue-ticket helper cannot `use ipe_diagnostics::GITHUB_REPO_SLUG`
/// directly, so each hand-mirrors its value; assert every one does, so a
/// slug change here without updating them fails this test instead of
/// silently 404ing at runtime.
#[test]
fn shell_and_toml_mirrors_agree_with_the_rust_ssot() {
    let root = repo_root();
    let slug = ipe_diagnostics::GITHUB_REPO_SLUG;

    // Each file assigns the slug to a shell var differently (a plain literal,
    // or a literal fallback inside `${VAR:-default}`); assert the slug value
    // itself is present rather than one fixed quoting shape.
    let cases: &[&str] = &[
        "install.sh",
        ".github/ci/enable-branch-protection.sh",
        "editors/lib/ipe-editors.sh",
        "tools/scripts/github/issue-ticket.sh",
    ];
    for rel in cases {
        let path = root.join(rel);
        let read = std::fs::read_to_string(&path);
        assert!(read.is_ok(), "could not read {}: {read:?}", path.display());
        let contents = read.unwrap_or_default();
        assert!(
            contents.contains(slug),
            "{rel} must mirror the SSOT repo slug `{slug}`"
        );
    }

    // The Zed extension manifest mirrors it as a full URL (extension +
    // grammar `repository` keys).
    let ext_toml = root.join("editors/zed-ipe/extension.toml");
    let read = std::fs::read_to_string(&ext_toml);
    assert!(
        read.is_ok(),
        "could not read {}: {read:?}",
        ext_toml.display()
    );
    let contents = read.unwrap_or_default();
    let url_needle = format!("https://github.com/{slug}");
    assert!(
        contents.contains(&url_needle),
        "editors/zed-ipe/extension.toml must mirror the SSOT repo URL `{url_needle}`"
    );
}
