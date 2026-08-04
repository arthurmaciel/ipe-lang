//! `ipe init` — scaffold a new Ipê project.
//!
//! `ipe init <name>` creates `<name>/` and fills it; `ipe init .` (or no
//! argument) scaffolds in the current directory, following the same convention
//! as `cargo new` / `cargo init`. The scaffold is a minimal, working
//! [`Ipe.Web`] counter: a Model holding an `Int` count, `Increment` /
//! `Decrement` messages, and a two-button view — a real program that
//! `ipe build` compiles and `ipe run` serves.
//!
//! Templates are embedded at build time via [`include_str!`] (the sibling
//! `templates/` directory), so scaffolding is self-contained and offline.

use std::fmt::Write as _;
use std::io::{IsTerminal as _, Write as _};
use std::path::{Path, PathBuf};

use crate::CliError;
use crate::style;

/// `src/Main.ipe` — the working counter. No substitution: this is a complete
/// program, byte-identical for every scaffolded project.
const MAIN_IPE: &str = include_str!("../templates/Main.ipe");

/// Templates carrying a `{name}` hole filled from the project name.
const IPE_TOML: &str = include_str!("../templates/ipe.toml.in");
const README_MD: &str = include_str!("../templates/README.md.in");

/// `.gitignore` — no substitution; the same ignore set fits every project.
const GITIGNORE: &str = include_str!("../templates/gitignore.in");

/// `AGENTS.md` — the Ipê authoring reference, embedded from the repository root
/// so every scaffolded project ships the same self-contained guide an agent or
/// developer needs to write Ipê. No substitution: it is byte-identical for every
/// project.
const AGENTS_MD: &str = include_str!("../../../AGENTS.md");

/// One file `ipe init` manages: where it lives, relative to the target, and the
/// content it would write there.
struct ManagedFile {
    /// The file's path relative to the target directory.
    rel: PathBuf,
    /// The exact bytes `init` would write.
    content: String,
}

/// What `init` will do with a single managed file, decided once from its
/// on-disk presence and the run's consent mode. Making the choice a value
/// keeps the decision (which may prompt) separate from the write (which never
/// prompts), so no write path re-derives consent.
enum FileAction {
    /// Write the file — it is absent, or the user consented to overwrite it.
    Write,
    /// Leave the file untouched — the user declined, or a non-interactive run
    /// refused to overwrite silently.
    Skip,
}

/// `ipe init [<name>] [--force]`.
///
/// With `<name>`, create the directory `<name>/` and scaffold inside it, using
/// `<name>` as the project name. With no argument or `.`, scaffold in the
/// current directory and take the project name from the directory's own name.
///
/// In an already-populated directory `init` never clobbers silently: for each
/// managed file it asks per file — restoring a missing one (default yes) or
/// overwriting an existing one (default no). `--force` overwrites every managed
/// file without asking. A non-interactive run (no TTY) never prompts: it writes
/// only the missing files and reports the existing ones it left untouched.
///
/// # Errors
/// Returns [`CliError::UsageOwned`] on an unrecognised flag or an unexpected
/// argument, and [`CliError::Io`] on any filesystem failure.
pub fn run_init(rest: &[String]) -> Result<(), CliError> {
    let mut target_arg: Option<String> = None;
    let mut force = false;
    for arg in rest {
        match arg.as_str() {
            "--force" => force = true,
            flag if flag.starts_with('-') => {
                return Err(CliError::UsageOwned(format!(
                    "ipe init: unknown flag {flag:?}"
                )));
            }
            positional if target_arg.is_none() => target_arg = Some(positional.to_owned()),
            other => {
                return Err(CliError::UsageOwned(format!(
                    "ipe init: unexpected argument {other:?}"
                )));
            }
        }
    }

    let target = target_arg.as_deref().unwrap_or(".");
    let target_dir = PathBuf::from(target);
    let project_name = project_name_for(&target_dir)?;

    let files = managed_files(&project_name);
    let fresh = is_fresh_target(&target_dir)?;
    if fresh || force {
        scaffold(&target_dir, &files)?;
        print_next_steps(target, &project_name);
    } else {
        reconcile_existing(&target_dir, &files)?;
    }
    Ok(())
}

/// The complete set of files `init` writes, each with its final content
/// (`{name}` already substituted). The single source of truth for both a fresh
/// scaffold and a per-file reconcile of an existing project.
fn managed_files(project_name: &str) -> Vec<ManagedFile> {
    vec![
        ManagedFile {
            rel: PathBuf::from("ipe.toml"),
            content: IPE_TOML.replace("{name}", project_name),
        },
        ManagedFile {
            rel: PathBuf::from("src").join("Main.ipe"),
            content: MAIN_IPE.to_owned(),
        },
        ManagedFile {
            rel: PathBuf::from("README.md"),
            content: README_MD.replace("{name}", project_name),
        },
        ManagedFile {
            rel: PathBuf::from(".gitignore"),
            content: GITIGNORE.to_owned(),
        },
        ManagedFile {
            rel: PathBuf::from("AGENTS.md"),
            content: AGENTS_MD.to_owned(),
        },
    ]
}

/// Whether the target holds none of `init`'s footprint: no `ipe.toml` and no
/// non-empty `src/`. A fresh (or absent) target scaffolds without prompting.
fn is_fresh_target(target_dir: &Path) -> Result<bool, CliError> {
    if target_dir.join("ipe.toml").exists() {
        return Ok(false);
    }
    let src = target_dir.join("src");
    if src.is_dir() {
        let mut entries = std::fs::read_dir(&src).map_err(|e| CliError::Io {
            path: src.clone(),
            source: e,
        })?;
        if entries.next().is_some() {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Reconcile an already-populated target against the managed set, one file at a
/// time. An interactive run asks per file (restore a missing file, default yes;
/// overwrite an existing one, default no). A non-interactive run writes only the
/// missing files and reports every existing file it left untouched — it never
/// overwrites without a visible decision.
fn reconcile_existing(target_dir: &Path, files: &[ManagedFile]) -> Result<(), CliError> {
    let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let mut restored: Vec<PathBuf> = Vec::new();
    let mut skipped: Vec<PathBuf> = Vec::new();

    for file in files {
        let path = target_dir.join(&file.rel);
        let exists = path.exists();
        let action = decide_action(&file.rel, exists, interactive);
        match action {
            FileAction::Write => {
                if let Some(parent) = path.parent() {
                    create_dir_all(parent)?;
                }
                write_file(&path, &file.content)?;
                restored.push(file.rel.clone());
            }
            FileAction::Skip => skipped.push(file.rel.clone()),
        }
    }

    print_reconcile_summary(interactive, &restored, &skipped);
    Ok(())
}

/// Decide what to do with one managed file. A missing file defaults to being
/// restored; an existing file defaults to being left alone — the fail-closed
/// choice, so a project's own edits are never overwritten without an explicit
/// yes. An interactive run may override either default through a per-file
/// prompt; a non-interactive run takes the default silently.
fn decide_action(rel: &Path, exists: bool, interactive: bool) -> FileAction {
    if !exists {
        // Restoring a missing file: default yes.
        if !interactive || prompt_yes_no(&format!("Restore {}?", rel.display()), true) {
            return FileAction::Write;
        }
        return FileAction::Skip;
    }
    // Overwriting an existing file: default no, and never without a prompt.
    if interactive && prompt_yes_no(&format!("Overwrite {}?", rel.display()), false) {
        FileAction::Write
    } else {
        FileAction::Skip
    }
}

/// Ask a `[Y/n]` / `[y/N]` question and read the answer, echoing the default in
/// the brackets. An empty answer (a bare Enter) takes `default`.
fn prompt_yes_no(question: &str, default: bool) -> bool {
    let hint = if default { "[Y/n]" } else { "[y/N]" };
    print!("{}", style::gutter(&format!("{question} {hint} ")));
    let _ = std::io::stdout().flush();
    crate::read_yes_no_default(default)
}

/// Report what `reconcile_existing` did: the files restored and the ones left in
/// place. A non-interactive run frames the skipped set as "would change" so a
/// script's operator sees what an interactive run would have offered.
fn print_reconcile_summary(interactive: bool, restored: &[PathBuf], skipped: &[PathBuf]) {
    let mut body = String::new();
    for rel in restored {
        let _ = writeln!(body, "restored {}", rel.display());
    }
    for rel in skipped {
        if interactive {
            let _ = writeln!(body, "kept {} (unchanged)", rel.display());
        } else {
            let _ = writeln!(body, "would overwrite {} (skipped: no TTY)", rel.display());
        }
    }
    if body.is_empty() {
        body.push_str("nothing to do.\n");
    }
    print!("{}", style::frame(&style::gutter(&body)));
}

/// Derive the project name: the last path component of the target, resolved to
/// an absolute path first so `.` yields the current directory's own name rather
/// than the literal `"."`.
fn project_name_for(target_dir: &Path) -> Result<String, CliError> {
    let absolute = if target_dir.is_absolute() {
        target_dir.to_path_buf()
    } else {
        let cwd = std::env::current_dir().map_err(|e| CliError::Io {
            path: PathBuf::from("."),
            source: e,
        })?;
        cwd.join(target_dir)
    };
    let name = absolute
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_owned)
        .ok_or_else(|| {
            CliError::UsageOwned(format!(
                "init: cannot derive a project name from target {}",
                target_dir.display()
            ))
        })?;
    Ok(name)
}

/// Write every managed file into a fresh (or force-overwritten) target,
/// creating each file's parent directory as needed.
fn scaffold(target_dir: &Path, files: &[ManagedFile]) -> Result<(), CliError> {
    for file in files {
        let path = target_dir.join(&file.rel);
        if let Some(parent) = path.parent() {
            create_dir_all(parent)?;
        }
        write_file(&path, &file.content)?;
    }
    Ok(())
}

fn create_dir_all(path: &Path) -> Result<(), CliError> {
    std::fs::create_dir_all(path).map_err(|e| CliError::Io {
        path: path.to_path_buf(),
        source: e,
    })
}

fn write_file(path: &Path, contents: &str) -> Result<(), CliError> {
    std::fs::write(path, contents).map_err(|e| CliError::Io {
        path: path.to_path_buf(),
        source: e,
    })
}

/// Print the friendly next-steps message. When scaffolding in place (`.`), the
/// `cd` step is omitted.
fn print_next_steps(target_arg: &str, project_name: &str) {
    let run_cmd = if target_arg == "." {
        "    ipe run".to_owned()
    } else {
        format!("    cd {target_arg} && ipe run")
    };
    let body = format!(
        "Created Ipê project `{project_name}`.\n\
         \n\
         Next steps:\n\
         {run_cmd}\n\
         \n\
         Then open http://localhost:8000 and click the counter buttons.\n"
    );
    print!("{}", style::frame(&style::gutter(&body)));
}
