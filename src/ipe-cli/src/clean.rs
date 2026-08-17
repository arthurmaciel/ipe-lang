//! `ipe clean` — remove a project's build-generated output.
//!
//! Deletes only the directories `ipe` itself generates — the emitted Rust
//! project (`out/`), the Cargo build tree (`target/`), and the per-project
//! cache (`.ipe/`) — and never user source or `ipe.toml`. The command is
//! fail-closed on two axes: it refuses to run outside an Ipê project (no
//! `ipe.toml` at the resolved root), and every deletion target is proven to sit
//! inside the canonicalised project root before a byte is removed, so a symlink
//! or a `..` component can never carry the delete outside the project.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::CliError;
use crate::style::{self, glyph};

/// The generated directories `clean` removes, named relative to the project
/// root. This is the deletion allowlist — nothing outside it is ever a
/// candidate — and it mirrors the ignore set the scaffolded `.gitignore`
/// carries (`out/`, `target/`, `.ipe/`).
const GENERATED_DIRS: &[&str] = &["out", "target", ".ipe"];

/// `ipe clean` — remove the current project's generated build output.
///
/// Takes no positional argument: it operates on the project rooted at the
/// current directory. Prints one line per removed directory and a closing
/// summary.
///
/// # Errors
/// [`CliError::UsageOwned`] on any argument (it takes none) or when the current
/// directory is not an Ipê project (no `ipe.toml`); [`CliError::Io`] on a
/// filesystem failure while removing a directory.
pub fn run_clean(rest: &[String]) -> Result<(), CliError> {
    if let Some(arg) = rest.first() {
        return Err(crate::cli_args::usage_unexpected_argument("clean", arg));
    }

    let root = project_root()?;
    let mut removed: Vec<String> = Vec::new();
    for name in GENERATED_DIRS {
        if let Some(display) = remove_generated_dir(&root, name)? {
            removed.push(display);
        }
    }

    print_summary(&removed);
    Ok(())
}

/// Resolve and validate the project root: the current directory, which must
/// hold an `ipe.toml`. Returns the canonicalised root so every later
/// containment check compares real, symlink-resolved paths.
///
/// # Errors
/// [`CliError::UsageOwned`] when there is no `ipe.toml` here (fail-closed: no
/// project, nothing to clean); [`CliError::Io`] when the directory cannot be
/// canonicalised.
fn project_root() -> Result<PathBuf, CliError> {
    let cwd = PathBuf::from(".");
    if !cwd.join("ipe.toml").is_file() {
        return Err(CliError::UsageOwned(
            "clean: no ipe.toml here — run it from an Ipê project root".to_owned(),
        ));
    }
    std::fs::canonicalize(&cwd).map_err(|e| CliError::Io {
        path: cwd,
        source: e,
    })
}

/// Remove one generated directory under `root`, returning the path removed (for
/// the summary) or `None` when it was absent or not a directory. The target is
/// canonicalised and proven to sit strictly inside `root` before removal, so a
/// symlinked `out/`/`target/`/`.ipe/` pointing outside the project is refused
/// rather than followed.
///
/// # Errors
/// [`CliError::UsageOwned`] if the resolved target escapes the project root (a
/// fail-closed refusal — the delete never leaves the project); [`CliError::Io`]
/// on a canonicalise or remove failure.
fn remove_generated_dir(root: &Path, name: &str) -> Result<Option<String>, CliError> {
    let candidate = root.join(name);
    if !candidate.is_dir() {
        return Ok(None);
    }
    let real = std::fs::canonicalize(&candidate).map_err(|e| CliError::Io {
        path: candidate.clone(),
        source: e,
    })?;
    // Containment guard: the resolved directory must be a strict descendant of
    // the resolved root, and never the root itself. A `..`/symlink that resolves
    // outside is refused, not deleted.
    if !real.starts_with(root) || real == *root {
        return Err(CliError::UsageOwned(format!(
            "clean: refusing to remove {} — it resolves outside the project root {}",
            real.display(),
            root.display()
        )));
    }
    std::fs::remove_dir_all(&real).map_err(|e| CliError::Io {
        path: real,
        source: e,
    })?;
    Ok(Some(format!("{name}/")))
}

/// Print the friendly result: one `removed <dir>` line per deleted directory,
/// then a one-line summary. When nothing was generated, say so plainly.
fn print_summary(removed: &[String]) {
    let p = style::Palette::for_stream(&std::io::stdout());
    let mut body = String::new();
    if removed.is_empty() {
        body.push_str("Nothing to clean — no generated output found.\n");
    } else {
        for dir in removed {
            let _ = writeln!(body, "{} removed {dir}", glyph::OK);
        }
        let n = removed.len();
        let noun = if n == 1 { "directory" } else { "directories" };
        let _ = writeln!(body, "\nCleaned {n} generated {noun}.");
    }
    print!(
        "{}",
        style::frame(&style::gutter(&format!("{}{body}{}", p.green, p.reset)))
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `remove_generated_dir` deletes a real subdirectory and reports it.
    #[test]
    fn removes_a_generated_subdir() {
        let root = std::env::temp_dir().join(format!("ipe_clean_ok_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("out").join("rust")).expect("make out/rust");
        let real_root = std::fs::canonicalize(&root).expect("canonicalize root");

        let removed = remove_generated_dir(&real_root, "out").expect("remove must succeed");
        assert_eq!(removed.as_deref(), Some("out/"));
        assert!(!real_root.join("out").exists(), "out/ must be gone");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// An absent directory is a no-op, not an error.
    #[test]
    fn absent_dir_is_a_noop() {
        let root = std::env::temp_dir().join(format!("ipe_clean_absent_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("make root");
        let real_root = std::fs::canonicalize(&root).expect("canonicalize root");

        let removed = remove_generated_dir(&real_root, "target").expect("must succeed");
        assert!(removed.is_none(), "an absent dir yields no removal");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A generated name that is a symlink escaping the project root is refused,
    /// and the escape target is left intact — the delete never leaves the root.
    #[cfg(unix)]
    #[test]
    fn refuses_a_symlink_escaping_the_root() {
        use std::os::unix::fs::symlink;

        let base = std::env::temp_dir().join(format!("ipe_clean_escape_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("project");
        let outside = base.join("precious");
        std::fs::create_dir_all(&root).expect("make root");
        std::fs::create_dir_all(&outside).expect("make outside");
        std::fs::write(outside.join("keep.txt"), b"do not delete").expect("write victim");
        let real_root = std::fs::canonicalize(&root).expect("canonicalize root");

        // `out` inside the project is a symlink to the outside directory.
        symlink(&outside, root.join("out")).expect("make escaping symlink");

        let result = remove_generated_dir(&real_root, "out");
        assert!(
            matches!(result, Err(CliError::UsageOwned(_))),
            "an escaping symlink must be refused, got: {result:?}"
        );
        assert!(
            outside.join("keep.txt").exists(),
            "the escape target must be left untouched"
        );

        let _ = std::fs::remove_dir_all(&base);
    }
}
