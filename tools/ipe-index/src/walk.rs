use crate::model::{Lang, Role, lang_of, role_of};
use anyhow::{Result, bail};
use std::process::{Command, Stdio};

pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024; // 2 MB cap (anti-OOM)

pub struct Tracked {
    pub path: String,
    pub lang: Lang,
    #[allow(dead_code)]
    pub role: Role,
}

/// Build-output / VCS / generated path segments the index must NEVER touch, even
/// when `.gitignore` hygiene is imperfect. The `--others` walk only respects
/// `.gitignore`, so a crate that forgot to ignore its `target/` would otherwise
/// flood the index with build artifacts (deps/fingerprints/rlibs) — defeating
/// the bounded-memory guarantee that is this tool's whole reason to exist.
/// Matched as a WHOLE path segment and case-sensitively, so legitimate source
/// dirs are never hit.
const SKIP_SEGMENTS: &[&str] = &[
    "target",
    "node_modules",
    "dist-newstyle",
    "ipe-out",
    ".ipe-index",
    ".git",
];

fn is_indexable(path: &str) -> bool {
    !path.split('/').any(|seg| SKIP_SEGMENTS.contains(&seg))
}

/// A pre-configured `git -C <repo>` invocation shared by every git plumbing
/// call below. Centralising it lets us harden all call sites at once:
///   * `core.quotePath=false` — emit non-ASCII paths literally (UTF-8) instead
///     of C-quoting them (`"src/\303\251.rs"`), which the line/tab parsers would
///     otherwise read verbatim as the wrong path.
///   * `GIT_TERMINAL_PROMPT=0` + a null stdin — a credential helper / askpass /
///     pager that tries to prompt would otherwise block on stdin forever and
///     wedge the indexer; both make any such prompt fail fast instead.
fn git_command(repo: &str) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-c")
        .arg("core.quotePath=false")
        .arg("-C")
        .arg(repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null());
    cmd
}

pub fn parse_tracked(ls_output: &str) -> Vec<Tracked> {
    ls_output
        .lines()
        .filter(|l| !l.is_empty())
        .filter(|p| is_indexable(p))
        .map(|p| Tracked {
            path: p.to_string(),
            lang: lang_of(p),
            role: role_of(p),
        })
        .collect()
}

/// All git-tracked AND untracked-but-not-ignored files in `repo` (respects
/// .gitignore via `--exclude-standard`, so generated dirs stay excluded —
/// bounded, no OOM risk). Including untracked-non-ignored files keeps the index
/// faithful to the working tree: a newly-added, not-yet-staged source file is
/// part of the repo's current state, and omitting it produces false "missing"
/// results — e.g. a fresh Rust kernel impl in a new `*.rs` file read as a
/// go-only parity gap purely because it hasn't been `git add`ed yet.
/// `--others` (untracked) is disjoint from the default tracked listing, so the
/// two outputs concatenate without dedup.
pub fn tracked(repo: &str) -> Result<Vec<Tracked>> {
    let run = |extra: &[&str]| -> Result<String> {
        let out = git_command(repo).arg("ls-files").args(extra).output()?;
        // Distinguish a real empty index from a git failure (non-repo, corrupt
        // state): a failed `ls-files` yields empty stdout and would otherwise be
        // read as "0 files" silently.
        if !out.status.success() {
            bail!(
                "git ls-files failed in {repo}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let mut combined = run(&[])?; // tracked
    combined.push_str(&run(&["--others", "--exclude-standard"])?); // untracked, not ignored
    Ok(parse_tracked(&combined))
}

/// Changed/added + deleted paths between `since` sha and HEAD (for incremental update).
pub fn changed(repo: &str, since: &str) -> Result<(Vec<Tracked>, Vec<String>)> {
    // `since` is interpolated into a positional commit-range token
    // (`{since}..HEAD`). Reject anything that git could parse as an option
    // (leading '-', e.g. `--output=…`) or that carries path/shell-hostile
    // bytes, so a crafted ref can't smuggle options or write arbitrary files.
    if since.is_empty()
        || since.starts_with('-')
        || !since
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'/' | b'-'))
    {
        bail!("refusing unsafe git since-ref: {since:?}");
    }
    // `--no-renames` decomposes renames into a `D oldpath` + `A newpath` pair so
    // the loop below upserts the new path and deletes the old one; without it,
    // git emits a 3-field `R100\told\tnew` line whose first two fields we'd
    // misread as `status=R100, path=old`, dropping the new path and indexing a
    // deleted one.
    let out = git_command(repo)
        .args([
            "diff",
            "--no-renames",
            "--name-status",
            &format!("{since}..HEAD"),
        ])
        .output()?;
    if !out.status.success() {
        bail!(
            "git diff failed in {repo}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut upserts = Vec::new();
    let mut deletes = Vec::new();
    for line in text.lines() {
        let mut it = line.split('\t');
        let (status, path) = (it.next().unwrap_or(""), it.next().unwrap_or(""));
        if path.is_empty() || !is_indexable(path) {
            continue;
        }
        if status.starts_with('D') {
            deletes.push(path.to_string());
        } else {
            upserts.push(Tracked {
                path: path.to_string(),
                lang: lang_of(path),
                role: role_of(path),
            });
        }
    }
    Ok((upserts, deletes))
}

pub fn head_sha(repo: &str) -> Result<String> {
    let out = git_command(repo).args(["rev-parse", "HEAD"]).output()?;
    if !out.status.success() {
        bail!(
            "git rev-parse HEAD failed in {repo}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_ls_files_output() {
        let out = "crates/ipe_parse/src/lexer.rs\nruntime/src/list.rs\nIpe/Core/List.ipe\n";
        let v = parse_tracked(out);
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].path, "crates/ipe_parse/src/lexer.rs");
        assert_eq!(v[1].lang, crate::model::Lang::Rust);
        assert_eq!(v[2].role, crate::model::Role::StdlibIpe);
    }

    #[test]
    fn skips_build_output_dirs() {
        // tracked source survives; untracked build artifacts (target/, deps,
        // node_modules) are dropped even though they appear in the walk output.
        let out = "runtime/src/path.rs\n\
                   tools/ipe-index/target/release/deps/foo.rs\n\
                   tools/ipe-index/target/debug/build/bar/out/baz.rs\n\
                   web/node_modules/pkg/index.ts\n\
                   examples/01/ipe-out/main.rs\n\
                   crates/ipe_lower/src/compile.rs\n";
        let v = parse_tracked(out);
        let paths: Vec<&str> = v.iter().map(|t| t.path.as_str()).collect();
        assert!(paths.contains(&"runtime/src/path.rs"));
        assert!(paths.contains(&"crates/ipe_lower/src/compile.rs"));
        assert!(!paths.iter().any(|p| p.contains("/target/")));
        assert!(!paths.iter().any(|p| p.contains("node_modules")));
        assert!(!paths.iter().any(|p| p.contains("ipe-out")));
        assert_eq!(v.len(), 2);
    }
}
