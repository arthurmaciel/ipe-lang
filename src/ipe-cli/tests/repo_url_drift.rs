//! Drift guard: the repository moved to `ipe-lang/compiler`. Every in-binary URL derives from the one Rust SSOT
//! (`ipe_diagnostics::REPO_URL`, re-exported as `ipe::style::REPO_URL`), but a
//! shell script, an editor manifest, a `.md` doc, or an example cannot import
//! that constant — each hand-spells the URL instead. This test is the single
//! net that catches EVERY such file: it walks every tracked file (`git
//! ls-files`) and fails if any still carries the old, now-redirect-only slug.
//!
//! `CHANGELOG.md` is exempt: it is release history and must keep the slug that
//! was true at the time of each past release. Goldens are not exempt: they are
//! regenerated from the SSOT, so a stale one fails here until it is.

use std::path::Path;
use std::process::Command;

mod support;

/// The retired slug: a redirect-only address today, and a stale link in any
/// file that still spells it (help output, diagnostics footers, editor setup,
/// the installer). Spelled in two halves so this file never carries it whole.
const OLD_SLUG: &str = concat!("arthurmaciel", "/ipe-lang");

/// A file exempt from the drift check, relative to the repository root.
fn is_exempt(rel_path: &str) -> bool {
    rel_path == "CHANGELOG.md"
}

/// Every line in `text` containing `needle`, as `(1-based line number, line)`.
///
/// Shared by the workspace scan below and by the unit test that proves this
/// matcher actually flags an offending line (PRINCIPLES.md "prove the
/// refusals" — a drift guard with no test on the matcher itself is a guard one
/// edit away from silently matching nothing).
fn lines_containing<'a>(text: &'a str, needle: &str) -> Vec<(usize, &'a str)> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| line.contains(needle))
        .map(|(i, line)| (i + 1, line))
        .collect()
}

/// Every file `git` tracks, repository-root-relative, forward-slash separated.
fn tracked_files(repo_root: &Path) -> Vec<String> {
    let result = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["ls-files"])
        .output();
    assert!(
        result.is_ok(),
        "failed to run `git ls-files` in {}: {result:?}",
        repo_root.display()
    );
    // The assert above already failed the test on `Err`; this arm is
    // unreachable in practice, so an empty file list (rather than an
    // unwrap/expect/panic the workspace lints deny) is a safe placeholder.
    let Ok(out) = result else {
        return Vec::new();
    };
    assert!(
        out.status.success(),
        "`git ls-files` failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_owned)
        .collect()
}

/// No tracked file — outside the narrow, documented exemptions — still spells
/// the retired slug.
#[test]
fn no_tracked_file_carries_the_retired_repo_slug() {
    let repo_root = support::repo_root();
    let mut offenders = Vec::new();

    for rel_path in tracked_files(&repo_root) {
        if is_exempt(&rel_path) {
            continue;
        }
        let abs_path = repo_root.join(&rel_path);
        // Binary/non-UTF8 tracked files (images, fonts, …) read as invalid
        // UTF-8; they cannot contain the slug as searchable text, so skip them
        // rather than fail the scan on an unrelated encoding mismatch.
        let Ok(contents) = std::fs::read_to_string(&abs_path) else {
            continue;
        };
        for (line_no, line) in lines_containing(&contents, OLD_SLUG) {
            offenders.push(format!("{rel_path}:{line_no}: {}", line.trim()));
        }
    }

    assert!(
        offenders.is_empty(),
        "these tracked files still carry the retired `{OLD_SLUG}` slug \
         (repo moved to ipe-lang/compiler) — repoint them; regenerate goldens \
         with `cargo run -p regen-cli-transcripts` and `UPDATE_GOLDENS=1`:\n{}",
        offenders.join("\n")
    );
}

#[cfg(test)]
mod matcher_tests {
    use super::*;

    /// Proves the refusal: the matcher actually flags a line carrying the old
    /// slug, at the right line number — not a vacuously-passing scan.
    #[test]
    fn lines_containing_flags_a_line_with_the_old_slug() {
        let offending = format!("see https://github.com/{OLD_SLUG}/issues");
        let fixture = format!("line one\n{offending}\nline three");
        let hits = lines_containing(&fixture, OLD_SLUG);
        assert_eq!(
            hits,
            vec![(2, offending.as_str())],
            "the matcher must flag exactly the offending line, by 1-based number"
        );
    }

    /// A file with no occurrence yields no hits.
    #[test]
    fn lines_containing_is_empty_for_a_clean_file() {
        let fixture = "nothing here\nnor here\nhttps://github.com/ipe-lang/compiler is fine";
        assert!(lines_containing(fixture, OLD_SLUG).is_empty());
    }

    /// Exemption list stays exact: only `CHANGELOG.md` is skipped, so the
    /// exemption can never silently widen to swallow real files.
    #[test]
    fn exemption_is_narrow() {
        assert!(is_exempt("CHANGELOG.md"));
        assert!(!is_exempt("src/ipe-cli/tests/golden/cli/toplevel_help.txt"));
        assert!(!is_exempt(
            "src/compiler/diagnostics/tests/render_goldens/internal_compiler_error_generic.txt"
        ));
        assert!(!is_exempt("README.md"));
        assert!(!is_exempt("install.sh"));
        assert!(!is_exempt("src/ipe-cli/src/style.rs"));
    }
}
