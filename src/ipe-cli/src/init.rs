//! `ipe init` — scaffold a new Ipê project.
//!
//! `ipe init <name>` creates `<name>/` and fills it; `ipe init .` (or no
//! argument) scaffolds in the current directory, following the same convention
//! as `cargo new` / `cargo init`. The scaffold is a minimal, working
//! [`Ipe.Live`] counter: a Model holding an `Int` count, `Increment` /
//! `Decrement` messages, and a two-button view — a real program that
//! `ipe build` compiles and `ipe run` serves.
//!
//! Templates are embedded at build time via [`include_str!`] (the sibling
//! `templates/` directory), so scaffolding is self-contained and offline.

use std::path::{Path, PathBuf};

use crate::CliError;

/// `src/Main.ipe` — the working counter. No substitution: this is a complete
/// program, byte-identical for every scaffolded project.
const MAIN_IPE: &str = include_str!("../templates/Main.ipe");

/// Templates carrying a `{name}` hole filled from the project name.
const IPE_TOML: &str = include_str!("../templates/ipe.toml.in");
const README_MD: &str = include_str!("../templates/README.md.in");

/// `.gitignore` — no substitution; the same ignore set fits every project.
const GITIGNORE: &str = include_str!("../templates/gitignore.in");

/// `AGENTS.md` — the Ipê authoring reference, embedded from the repository root
/// so every scaffolded project (and `ipe upgrade-agents`) ships the same
/// self-contained guide an agent or developer needs to write Ipê. No
/// substitution: it is byte-identical for every project.
const AGENTS_MD: &str = include_str!("../../../AGENTS.md");

/// `ipe init [<name>] [--force]`.
///
/// With `<name>`, create the directory `<name>/` and scaffold inside it, using
/// `<name>` as the project name. With no argument or `.`, scaffold in the
/// current directory and take the project name from the directory's own name.
///
/// # Errors
/// Returns [`CliError::Usage`] on an unrecognised flag,
/// [`CliError::UsageOwned`] when the target already holds a project (an
/// existing `ipe.toml` or a non-empty `src/`) and `--force` was not given, and
/// [`CliError::Io`] on any filesystem failure.
pub fn run_init(rest: &[String]) -> Result<(), CliError> {
    let mut target_arg: Option<String> = None;
    let mut force = false;
    for arg in rest {
        match arg.as_str() {
            "--force" => force = true,
            flag if flag.starts_with('-') => return Err(CliError::Usage(crate::USAGE)),
            positional if target_arg.is_none() => target_arg = Some(positional.to_owned()),
            _ => return Err(CliError::Usage(crate::USAGE)),
        }
    }

    let target = target_arg.as_deref().unwrap_or(".");
    let target_dir = PathBuf::from(target);
    let project_name = project_name_for(&target_dir)?;

    if !force {
        guard_empty(&target_dir)?;
    }

    scaffold(&target_dir, &project_name)?;
    print_next_steps(target, &project_name);
    Ok(())
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

/// Refuse to overwrite an existing project: a present `ipe.toml`, or a `src/`
/// directory that already holds files. An empty or absent target is fine.
fn guard_empty(target_dir: &Path) -> Result<(), CliError> {
    if target_dir.join("ipe.toml").exists() {
        return Err(CliError::UsageOwned(format!(
            "init: {} already contains an ipe.toml — pass --force to scaffold anyway",
            target_dir.display()
        )));
    }
    let src = target_dir.join("src");
    if src.is_dir() {
        let mut entries = std::fs::read_dir(&src).map_err(|e| CliError::Io {
            path: src.clone(),
            source: e,
        })?;
        if entries.next().is_some() {
            return Err(CliError::UsageOwned(format!(
                "init: {} already has a non-empty src/ — pass --force to scaffold anyway",
                target_dir.display()
            )));
        }
    }
    Ok(())
}

/// Write every scaffold file, creating `<target>/` and `<target>/src/` as
/// needed. `{name}` in the templated files is replaced with `project_name`.
fn scaffold(target_dir: &Path, project_name: &str) -> Result<(), CliError> {
    let src_dir = target_dir.join("src");
    create_dir_all(&src_dir)?;

    write_file(
        &target_dir.join("ipe.toml"),
        &IPE_TOML.replace("{name}", project_name),
    )?;
    write_file(&src_dir.join("Main.ipe"), MAIN_IPE)?;
    write_file(
        &target_dir.join("README.md"),
        &README_MD.replace("{name}", project_name),
    )?;
    write_file(&target_dir.join(".gitignore"), GITIGNORE)?;
    write_file(&target_dir.join("AGENTS.md"), AGENTS_MD)?;
    Ok(())
}

/// `ipe upgrade-agents` — (re)write `AGENTS.md` in the current directory.
///
/// Writes the version of the Ipê authoring reference this `ipe` ships, so an
/// existing project can refresh it as the reference evolves. Overwrites any
/// existing `AGENTS.md` (that is the point — it is a generated, non-hand-edited
/// reference).
///
/// # Errors
/// [`CliError::UsageOwned`] on any unexpected argument; [`CliError::Io`] when the
/// file cannot be written.
pub fn run_upgrade_agents(rest: &[String]) -> Result<(), CliError> {
    if let Some(arg) = rest.first() {
        return Err(CliError::UsageOwned(format!(
            "upgrade-agents: unexpected argument `{arg}` (it takes none)"
        )));
    }
    write_file(Path::new("AGENTS.md"), AGENTS_MD)?;
    println!("wrote AGENTS.md ({} bytes)", AGENTS_MD.len());
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
    println!("Created Ipê project `{project_name}`.");
    println!();
    println!("Next steps:");
    if target_arg == "." {
        println!("    ipe run");
    } else {
        println!("    cd {target_arg} && ipe run");
    }
    println!();
    println!("Then open http://localhost:8000 and click the counter buttons.");
}
