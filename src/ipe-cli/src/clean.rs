//! `ipe clean` — remove a project's build-generated output.
//!
//! Deletes only the directories `ipe` itself owns — the build output (`out/`,
//! and only while it carries ipe's ownership marker) and the per-project cache
//! (`.ipe/`) — and never user source or `package.ipe`. The command is
//! fail-closed on three axes: it refuses to run outside an Ipê project (no
//! `package.ipe` at the resolved root), it refuses an `out/` ipe did not create,
//! and every deletion target is proven to sit inside the canonicalised project
//! root before a byte is removed, so a symlink or a `..` component can never
//! carry the delete outside the project.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::CliError;
use crate::cli_args::{self, OutputFormat};
use crate::style;

/// A directory `clean` may remove.
///
/// Named relative to the project root, with the proof of ownership it must show
/// first.
struct Generated {
    name: &'static str,
    /// Whether the directory must carry [`crate::output_dir::OWNERSHIP_MARKER`].
    ///
    /// `out/` is a common name a user may have chosen for their own files, so
    /// it is removed only when ipe marked it; `.ipe/` is ipe's own namespace.
    needs_marker: bool,
}

/// The deletion allowlist — nothing outside it is ever a candidate.
const GENERATED_DIRS: &[Generated] = &[
    Generated {
        name: crate::output_dir::DEFAULT_OUTPUT_DIR,
        needs_marker: true,
    },
    Generated {
        name: ".ipe",
        needs_marker: false,
    },
];

/// Parsed `ipe clean` arguments.
pub(crate) struct CleanArgs {
    /// The output format for the removal report.
    pub(crate) format: OutputFormat,
}

/// Parse `ipe clean` arguments: only the shared `--json`/`--plain` format flags;
/// the command takes no positional argument.
///
/// # Errors
/// [`CliError::UsageOwned`] on an unknown flag or any positional argument.
pub(crate) fn parse_clean_args(rest: &[String]) -> Result<CleanArgs, CliError> {
    let mut format: Option<OutputFormat> = None;
    for arg in rest {
        if cli_args::consume_format_flag(&mut format, arg, "clean")? {
            continue;
        }
        if arg.starts_with('-') {
            return Err(crate::cli_args::usage_unknown_flag("clean", arg));
        }
        return Err(crate::cli_args::usage_unexpected_argument("clean", arg));
    }
    Ok(CleanArgs {
        format: format.unwrap_or_default(),
    })
}

/// `ipe clean` — remove the current project's generated build output.
///
/// Takes no positional argument: it operates on the project rooted at the
/// current directory. Prints one line per removed directory and a closing
/// summary.
///
/// `--json` emits `{"schema":"ipe.cli.clean/1","removed":[…]}`.
/// `--plain` prints one removed path per line, flush-left.
///
/// # Errors
/// [`CliError::UsageOwned`] on any unrecognised argument or when the current
/// directory is not an Ipê project (no `package.ipe`); [`CliError::Io`] on a
/// filesystem failure while removing a directory.
pub fn run_clean(rest: &[String]) -> Result<(), CliError> {
    let args = parse_clean_args(rest)?;

    let root = project_root()?;
    let mut removed: Vec<String> = Vec::new();
    for generated in GENERATED_DIRS {
        if let Some(display) = remove_generated_dir(&root, generated)? {
            removed.push(display);
        }
    }

    print_summary(&removed, args.format);
    Ok(())
}

/// Resolve and validate the project root, the current directory holding a `package.ipe`.
///
/// Returns the canonicalised root so every later containment check compares
/// real, symlink-resolved paths.
///
/// # Errors
/// [`CliError::UsageOwned`] when there is no `package.ipe` here (fail-closed: no
/// project, nothing to clean), with the legacy-toml hint when only a legacy
/// `ipe.toml` is present; [`CliError::Io`] when the directory cannot be
/// canonicalised.
fn project_root() -> Result<PathBuf, CliError> {
    let cwd = PathBuf::from(".");
    if crate::project::manifest_in_dir(&cwd).is_none() {
        if crate::project::has_only_legacy_toml(&cwd) {
            return Err(CliError::Usage(crate::project::LEGACY_TOML_HINT));
        }
        return Err(CliError::UsageOwned(
            "clean: no package.ipe here — run it from an Ipê project root".to_owned(),
        ));
    }
    std::fs::canonicalize(&cwd).map_err(|e| CliError::Io {
        path: cwd,
        source: e,
    })
}

/// Remove one generated directory under `root`, returning its name for the summary.
///
/// `None` when it is absent or not a directory. The entry is lstat'd, never
/// followed: a symlinked `out/`/`.ipe/` is refused (its target untouched), an
/// `out/` without ipe's ownership marker is refused untouched, and the removal
/// itself never follows a symlink met inside the tree.
///
/// # Errors
/// [`CliError::OutputRefused`] for a symlink or an unmarked `out/`;
/// [`CliError::Io`] on a stat or remove failure.
fn remove_generated_dir(root: &Path, generated: &Generated) -> Result<Option<String>, CliError> {
    let name = generated.name;
    let candidate = root.join(name);
    let meta = match std::fs::symlink_metadata(&candidate) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(CliError::Io {
                path: candidate,
                source: e,
            });
        }
    };
    if meta.file_type().is_symlink() {
        return Err(crate::output_dir::OutputRefusal::Symlink(candidate).into());
    }
    if !meta.is_dir() {
        return Ok(None);
    }
    if generated.needs_marker && !crate::output_dir::has_marker(&candidate)? {
        return Err(crate::output_dir::OutputRefusal::NotIpeOwned(candidate).into());
    }
    std::fs::remove_dir_all(&candidate).map_err(|e| CliError::Io {
        path: candidate,
        source: e,
    })?;
    Ok(Some(format!("{name}/")))
}

/// Print the removal result in the requested format.
///
/// `--json` emits `{"schema":"ipe.cli.clean/1","removed":[…]}`.
/// `--plain` prints one removed path per line, flush-left, with no summary.
/// Human (default) prints a decorated frame with a one-line count.
fn print_summary(removed: &[String], format: OutputFormat) {
    use OutputFormat::{Human, Json, Plain};
    match format {
        Json => {
            use crate::cli_args::json;
            let items: Vec<String> = removed.iter().map(|s| json::string(s)).collect();
            println!(
                "{}",
                json::object(&[
                    ("schema", json::string("ipe.cli.clean/1")),
                    ("removed", json::array(&items)),
                ])
            );
        }
        Plain => {
            for dir in removed {
                println!("{dir}");
            }
        }
        Human => {
            let p = style::Palette::for_stream(&std::io::stdout());
            // A completed clean is a success: its glyph and green tint come from
            // the style SSOT, not a per-site glyph/colour pairing.
            let (glyph, tint) = style::Outcome::Success.glyph_and_tint(p);
            let mut body = String::new();
            if removed.is_empty() {
                body.push_str("Nothing to clean — no generated output found.\n");
            } else {
                for dir in removed {
                    let _ = writeln!(body, "{glyph} removed {dir}");
                }
                let n = removed.len();
                let noun = if n == 1 { "directory" } else { "directories" };
                let _ = writeln!(body, "\nCleaned {n} generated {noun}.");
            }
            print!(
                "{}",
                style::frame(&style::gutter(&format!("{tint}{body}{}", p.reset)))
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUT: &Generated = &Generated {
        name: "out",
        needs_marker: true,
    };
    const DOT_IPE: &Generated = &Generated {
        name: ".ipe",
        needs_marker: false,
    };

    /// `remove_generated_dir` deletes an ipe-owned output directory and reports it.
    #[test]
    fn removes_a_generated_subdir() {
        let root = std::env::temp_dir().join(format!("ipe_clean_ok_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("make root");
        let real_root = std::fs::canonicalize(&root).expect("canonicalize root");
        crate::output_dir::OwnedDir::claim(&real_root.join("out").join("rust"))
            .expect("claim out/rust");

        let removed = remove_generated_dir(&real_root, OUT).expect("remove must succeed");
        assert_eq!(removed.as_deref(), Some("out/"));
        assert!(!real_root.join("out").exists(), "out/ must be gone");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// An `out/` ipe did not create — no ownership marker — is refused and kept.
    #[test]
    fn refuses_an_unowned_out_dir() {
        let root = std::env::temp_dir().join(format!("ipe_clean_unowned_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("out")).expect("make out");
        std::fs::write(root.join("out").join("thesis.tex"), "mine").expect("user file");
        let real_root = std::fs::canonicalize(&root).expect("canonicalize root");

        let result = remove_generated_dir(&real_root, OUT);
        assert!(
            matches!(result, Err(CliError::OutputRefused(_))),
            "an unmarked out/ must be refused, got: {result:?}"
        );
        assert!(
            real_root.join("out").join("thesis.tex").is_file(),
            "user file kept"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// An absent directory is a no-op, not an error.
    #[test]
    fn absent_dir_is_a_noop() {
        let root = std::env::temp_dir().join(format!("ipe_clean_absent_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("make root");
        let real_root = std::fs::canonicalize(&root).expect("canonicalize root");

        let removed = remove_generated_dir(&real_root, DOT_IPE).expect("must succeed");
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

        let result = remove_generated_dir(&real_root, OUT);
        assert!(
            matches!(result, Err(CliError::OutputRefused(_))),
            "an escaping symlink must be refused, got: {result:?}"
        );
        assert!(
            outside.join("keep.txt").exists(),
            "the escape target must be left untouched"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// Planted symlinks in a marked `out/` are removed as links.
    ///
    /// Neither target is followed or touched.
    #[cfg(unix)]
    #[test]
    fn removes_planted_links_without_following_them() {
        use std::os::unix::fs::symlink;

        let base = std::env::temp_dir().join(format!("ipe_clean_planted_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("project");
        let outside = base.join("precious");
        std::fs::create_dir_all(&root).expect("make root");
        std::fs::create_dir_all(&outside).expect("make outside");
        std::fs::write(outside.join("keep.txt"), b"do not delete").expect("write victim");
        let real_root = std::fs::canonicalize(&root).expect("canonicalize root");
        crate::output_dir::OwnedDir::claim(&real_root.join("out")).expect("claim out");
        symlink(&outside, real_root.join("out").join("rust")).expect("dir link");
        symlink(outside.join("keep.txt"), real_root.join("out").join("bin")).expect("file link");

        let removed = remove_generated_dir(&real_root, OUT).expect("remove must succeed");
        assert_eq!(removed.as_deref(), Some("out/"));
        assert_eq!(
            std::fs::read(outside.join("keep.txt")).ok().as_deref(),
            Some(&b"do not delete"[..]),
            "link targets survive byte-for-byte"
        );

        let _ = std::fs::remove_dir_all(&base);
    }
}
