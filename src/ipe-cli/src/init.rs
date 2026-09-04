//! `ipe init` — scaffold a new Ipê project.
//!
//! `ipe init <name>` creates `<name>/` and fills it; `ipe init .` (or no
//! argument) scaffolds in the current directory, following the same convention
//! as `cargo new` / `cargo init`.
//!
//! On a TTY the command runs a short wizard: shape → (if web) runtime → host.
//! The scaffold is the matching entry for that shape:
//!
//! - `script` — a `Task Error ()` main
//! - `tui`    — a `Tui.app` main
//! - `cli`    — a `Cli.app` main
//! - `server` — a `Server.listen` main
//! - `web`    — a `Web.app` counter (the default)
//!
//! Non-TTY runs and explicit `--shape` / `--runtime` / `--host` flags skip the
//! wizard and use the supplied or default values. Templates are embedded at
//! build time via [`include_str!`], so scaffolding is self-contained and offline.

use std::fmt::Write as _;
use std::io::{IsTerminal as _, Write as _};
use std::path::{Path, PathBuf};

use crate::{CliError, health, style};

// ── per-shape Main.ipe templates ─────────────────────────────────────────────

const MAIN_WEB_IPE: &str = include_str!("../templates/Main.ipe");
const MAIN_TUI_IPE: &str = include_str!("../templates/Main.tui.ipe");
const MAIN_CLI_IPE: &str = include_str!("../templates/Main.cli.ipe");
const MAIN_SERVER_IPE: &str = include_str!("../templates/Main.server.ipe");
const MAIN_SCRIPT_IPE: &str = include_str!("../templates/Main.script.ipe");

// ── per-shape package.ipe templates (carry a `{name}` hole) ──────────────────

const PACKAGE_WEB_IPE: &str = include_str!("../templates/package.web.ipe.in");
const PACKAGE_TUI_IPE: &str = include_str!("../templates/package.tui.ipe.in");
const PACKAGE_CLI_IPE: &str = include_str!("../templates/package.cli.ipe.in");
const PACKAGE_SERVER_IPE: &str = include_str!("../templates/package.server.ipe.in");
const PACKAGE_SCRIPT_IPE: &str = include_str!("../templates/package.script.ipe.in");

// ── shared templates ──────────────────────────────────────────────────────────

const README_MD: &str = include_str!("../templates/README.md.in");

const PACKAGE_LIB_IPE: &str = include_str!("../templates/package.lib.ipe.in");
const LIB_IPE: &str = include_str!("../templates/Lib.ipe.in");
const README_LIB_MD: &str = include_str!("../templates/README.lib.md.in");

const GITIGNORE: &str = include_str!("../templates/gitignore.in");
const AGENTS_MD: &str = include_str!("../templates/AGENTS.md.in");

// ── shape model ───────────────────────────────────────────────────────────────

/// The five project shapes a wizard or `--shape` flag may select.
///
/// The shape is pinned by `main`'s entry function; the manifest's `delivery`
/// sections provide per-host build configuration for each resolved target.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum InitShape {
    /// `main = Web.app …` — DOM rendering, live or SPA.
    #[default]
    Web,
    /// `main = Tui.app …` — terminal cells rendering.
    Tui,
    /// `main = Cli.app …` — terminal lines rendering.
    Cli,
    /// `main = Server.listen …` — HTTP server.
    Server,
    /// `main : Task Error ()` — plain task, no rendering.
    Script,
}

impl InitShape {
    /// Parse a `--shape` flag value. Returns `None` for an unrecognised token.
    fn parse(s: &str) -> Option<Self> {
        match s {
            "web" => Some(Self::Web),
            "tui" => Some(Self::Tui),
            "cli" => Some(Self::Cli),
            "server" => Some(Self::Server),
            "script" => Some(Self::Script),
            _ => None,
        }
    }

    /// The display name used in prompts and messages.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Web => "web",
            Self::Tui => "tui",
            Self::Cli => "cli",
            Self::Server => "server",
            Self::Script => "script",
        }
    }

    /// The `Main.ipe` source for this shape (byte-stable, no substitution).
    const fn main_ipe(self) -> &'static str {
        match self {
            Self::Web => MAIN_WEB_IPE,
            Self::Tui => MAIN_TUI_IPE,
            Self::Cli => MAIN_CLI_IPE,
            Self::Server => MAIN_SERVER_IPE,
            Self::Script => MAIN_SCRIPT_IPE,
        }
    }

    /// The `package.ipe` template for this shape (carries a `{name}` hole).
    const fn package_ipe(self) -> &'static str {
        match self {
            Self::Web => PACKAGE_WEB_IPE,
            Self::Tui => PACKAGE_TUI_IPE,
            Self::Cli => PACKAGE_CLI_IPE,
            Self::Server => PACKAGE_SERVER_IPE,
            Self::Script => PACKAGE_SCRIPT_IPE,
        }
    }
}

// ── init args ─────────────────────────────────────────────────────────────────

/// Parsed and validated arguments for `ipe init`.
struct InitArgs {
    target_arg: Option<String>,
    force: bool,
    lib: bool,
    /// When set by `--shape <shape>`, skips the wizard shape prompt.
    shape: Option<InitShape>,
}

// ── managed-file model ────────────────────────────────────────────────────────

/// One file `ipe init` manages: where it lives, relative to the target, and
/// the content it would write there.
struct ManagedFile {
    /// The file's path relative to the target directory.
    rel: PathBuf,
    /// The exact bytes `init` would write.
    content: String,
}

/// What `init` will do with a single managed file, decided once from its
/// on-disk presence and the run's consent mode. Keeping the choice as a value
/// separates the decision (which may prompt) from the write (which never prompts).
enum FileAction {
    /// Write the file — it is absent, or the user consented to overwrite it.
    Write,
    /// Leave the file untouched — the user declined, or a non-interactive run
    /// refused to overwrite silently.
    Skip,
}

// ── entry point ───────────────────────────────────────────────────────────────

/// `ipe init [<name>] [--force] [--lib] [--shape <shape>]`.
///
/// With `<name>`, create the directory `<name>/` and scaffold inside it. With
/// no argument or `.`, scaffold in the current directory.
///
/// On a TTY, without `--shape`, the wizard prompts for shape (and, for `web`,
/// runtime and host) before scaffolding. With `--shape` or no TTY, the
/// supplied shape (or the default, `web`) is used directly.
///
/// # Errors
/// [`CliError::UsageOwned`] on an unrecognised flag or an unexpected argument;
/// [`CliError::Io`] on any filesystem failure.
pub fn run_init(rest: &[String]) -> Result<(), CliError> {
    let args = parse_init_args(rest)?;

    let target = args.target_arg.as_deref().unwrap_or(".");
    let target_dir = PathBuf::from(target);
    let project_name = project_name_for(&target_dir)?;

    if args.lib {
        let files = library_files(&project_name);
        return run_scaffold(target, &target_dir, &project_name, &files, args.force, true);
    }

    let is_tty = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();

    // Determine shape: explicit flag → no wizard; TTY → wizard; else default.
    let shape = match args.shape {
        Some(s) => s,
        None if is_tty && !args.force => wizard_shape()?,
        None => InitShape::default(),
    };

    let files = managed_files(&project_name, shape);
    run_scaffold(
        target,
        &target_dir,
        &project_name,
        &files,
        args.force,
        false,
    )
}

/// Parse raw `init` arguments into [`InitArgs`].
fn parse_init_args(rest: &[String]) -> Result<InitArgs, CliError> {
    let mut target_arg: Option<String> = None;
    let mut force = false;
    let mut lib = false;
    let mut shape: Option<InitShape> = None;
    let mut iter = rest.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--force" => force = true,
            "--lib" => lib = true,
            "--shape" => {
                let val = iter.next().ok_or(CliError::Usage(
                    "ipe init: `--shape` requires a value: script, tui, cli, server, web",
                ))?;
                shape = Some(InitShape::parse(val).ok_or_else(|| {
                    CliError::UsageOwned(format!(
                        "ipe init: unknown shape `{val}` — expected: script, tui, cli, server, web"
                    ))
                })?);
            }
            flag if flag.starts_with('-') => {
                return Err(crate::cli_args::usage_unknown_flag("init", flag));
            }
            positional if target_arg.is_none() => target_arg = Some(positional.to_owned()),
            other => {
                return Err(crate::cli_args::usage_unexpected_argument("init", other));
            }
        }
    }
    Ok(InitArgs {
        target_arg,
        force,
        lib,
        shape,
    })
}

/// TTY wizard: prompt the user for shape and (for web) runtime+host.
///
/// Returns the selected [`InitShape`]. The wizard is only called when stdin
/// and stdout are both TTYs and `--shape` was not passed.
fn wizard_shape() -> Result<InitShape, CliError> {
    print!(
        "{}",
        style::gutter(
            "What kind of program is this?\n\
             \n\
             [1] web    — browser / desktop / mobile app  (default)\n\
             [2] tui    — terminal UI with cells\n\
             [3] cli    — command-line program with text output\n\
             [4] server — HTTP server\n\
             [5] script — plain task, no rendering\n\
             \n\
             Shape [1]: "
        )
    );
    let _ = std::io::stdout().flush();
    let line = read_line_trimmed();
    let shape = match line.as_deref().unwrap_or("") {
        "" | "1" | "web" => InitShape::Web,
        "2" | "tui" => InitShape::Tui,
        "3" | "cli" => InitShape::Cli,
        "4" | "server" => InitShape::Server,
        "5" | "script" => InitShape::Script,
        other => {
            return Err(CliError::UsageOwned(format!(
                "ipe init: unknown shape `{other}` — expected 1-5 or one of: \
                 web, tui, cli, server, script"
            )));
        }
    };
    Ok(shape)
}

/// Read one trimmed line from stdin, or `None` on EOF / read error.
fn read_line_trimmed() -> Option<String> {
    let mut buf = String::new();
    match std::io::stdin().read_line(&mut buf) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(buf.trim_end_matches(['\n', '\r']).to_owned()),
    }
}

// ── scaffold helpers ─────────────────────────────────────────────────────────

/// Run the scaffold: fresh → write all; existing → reconcile.
fn run_scaffold(
    target_arg: &str,
    target_dir: &Path,
    project_name: &str,
    files: &[ManagedFile],
    force: bool,
    lib: bool,
) -> Result<(), CliError> {
    let fresh = is_fresh_target(target_dir)?;
    if fresh || force {
        scaffold(target_dir, files)?;
        let is_tty = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
        let interactive = should_offer_health_check(is_tty, force);
        if lib {
            print_next_steps_lib(target_arg, project_name);
        } else {
            print_next_steps(target_arg, project_name, interactive);
        }
        if interactive && prompt_yes_no("Verify your toolchain now?", true) {
            let _ = health::run_health_inline();
        }
    } else {
        reconcile_existing(target_dir, files)?;
    }
    Ok(())
}

/// The complete set of files `init` writes for an application project.
///
/// `shape` selects which `Main.ipe` and `package.ipe` are scaffolded; all
/// other files are shape-independent.
fn managed_files(project_name: &str, shape: InitShape) -> Vec<ManagedFile> {
    vec![
        ManagedFile {
            rel: PathBuf::from("package.ipe"),
            content: shape.package_ipe().replace("{name}", project_name),
        },
        ManagedFile {
            rel: PathBuf::from("src").join("Main.ipe"),
            content: shape.main_ipe().to_owned(),
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

/// The complete set of files `ipe init --lib` writes.
fn library_files(project_name: &str) -> Vec<ManagedFile> {
    let module = module_name_for(project_name);
    // `{name}` / `{module}` are template placeholders substituted by `.replace`,
    // not `format!` arguments — the formatting-args lint's heuristic misreads the
    // `{module}` literal here.
    #[allow(clippy::literal_string_with_formatting_args)]
    let fill = |template: &str| {
        template
            .replace("{name}", project_name)
            .replace("{module}", &module)
    };
    vec![
        ManagedFile {
            rel: PathBuf::from("package.ipe"),
            content: fill(PACKAGE_LIB_IPE),
        },
        ManagedFile {
            rel: PathBuf::from("src").join(format!("{module}.ipe")),
            content: fill(LIB_IPE),
        },
        ManagedFile {
            rel: PathBuf::from("README.md"),
            content: fill(README_LIB_MD),
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

/// Derive a valid single-segment Ipê module name from a project name.
fn module_name_for(project_name: &str) -> String {
    let mut out = String::new();
    for word in project_name.split(|c: char| !c.is_ascii_alphanumeric()) {
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            out.push(first.to_ascii_uppercase());
            out.extend(chars);
        }
    }
    match out.chars().next() {
        Some(c) if c.is_ascii_uppercase() => out,
        _ => "Lib".to_owned(),
    }
}

/// Whether the target holds none of `init`'s footprint: no manifest
/// (`package.ipe` or a legacy `ipe.toml`) and no non-empty `src/`.
fn is_fresh_target(target_dir: &Path) -> Result<bool, CliError> {
    if target_dir.join("package.ipe").exists() || target_dir.join("ipe.toml").exists() {
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

/// Reconcile an already-populated target against the managed set, one file at
/// a time. Interactive runs prompt per file; non-interactive runs write only
/// missing files and report the existing ones left untouched.
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

/// Decide what to do with one managed file. Missing files default to being
/// restored; existing ones default to being left alone.
fn decide_action(rel: &Path, exists: bool, interactive: bool) -> FileAction {
    if !exists {
        if !interactive || prompt_yes_no(&format!("Restore {}?", rel.display()), true) {
            return FileAction::Write;
        }
        return FileAction::Skip;
    }
    if interactive && prompt_yes_no(&format!("Overwrite {}?", rel.display()), false) {
        FileAction::Write
    } else {
        FileAction::Skip
    }
}

/// Ask a `[Y/n]` / `[y/N]` question and read the answer.
fn prompt_yes_no(question: &str, default: bool) -> bool {
    let hint = if default { "[Y/n]" } else { "[y/N]" };
    print!("{}", style::gutter(&format!("{question} {hint} ")));
    let _ = std::io::stdout().flush();
    crate::read_yes_no_default(default)
}

/// Report what `reconcile_existing` did.
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

/// Derive the project name from the last path component of the resolved target.
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

/// Write every managed file into a fresh (or force-overwritten) target.
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

/// Whether to offer the interactive `ipe health` check after scaffolding.
const fn should_offer_health_check(is_tty: bool, force: bool) -> bool {
    is_tty && !force
}

/// Print the friendly next-steps message.
fn print_next_steps(target_arg: &str, project_name: &str, interactive: bool) {
    let run_cmd = if target_arg == "." {
        "    ipe run".to_owned()
    } else {
        format!("    cd {target_arg} && ipe run")
    };
    let health_tip = if interactive {
        String::new()
    } else {
        "\nTip: run  ipe health  to tune your toolchain for faster builds.\n".to_owned()
    };
    let body = format!(
        "Created Ipê project `{project_name}`.\n\
         \n\
         Next steps:\n\
         {run_cmd}\n\
         \n\
         Then open http://localhost:8000 and click the counter buttons.\
         {health_tip}"
    );
    print!("{}", style::frame(&style::gutter(&body)));
}

/// Print the next-steps message for a freshly scaffolded library.
fn print_next_steps_lib(target_arg: &str, project_name: &str) {
    let build_cmd = if target_arg == "." {
        "    ipe build".to_owned()
    } else {
        format!("    cd {target_arg} && ipe build")
    };
    let body = format!(
        "Created Ipê library `{project_name}`.\n\
         \n\
         Next steps:\n\
         {build_cmd}\n\
         \n\
         Add public modules under src/ and list each in package.ipe's exposedModules.\n"
    );
    print!("{}", style::frame(&style::gutter(&body)));
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: write a minimal src/Main.ipe so the manifest reader's src-root check passes.
    fn write_stub_src(root: &Path) {
        let src = root.join("src");
        std::fs::create_dir_all(&src).expect("create src/");
        std::fs::write(
            src.join("Main.ipe"),
            "module Main exposing (main)\nmain = 0\n",
        )
        .expect("write Main.ipe");
    }

    #[test]
    fn scaffolded_web_manifest_is_package_ipe_that_re_parses() {
        let files = managed_files("demo-app", InitShape::Web);
        let manifest = files
            .iter()
            .find(|f| f.rel == Path::new("package.ipe"))
            .expect("init writes a package.ipe");

        let root = std::env::temp_dir().join("ipe_init_web_roundtrip");
        let _ = std::fs::remove_dir_all(&root);
        write_stub_src(&root);
        let path = root.join("package.ipe");
        std::fs::write(&path, &manifest.content).expect("write scaffolded package.ipe");

        let parsed = crate::project::parse_manifest(&path).expect("scaffolded manifest re-parses");
        assert_eq!(parsed.name, "demo-app");
        assert_eq!(parsed.delivery.desktop.width, 1024);
        assert_eq!(parsed.delivery.desktop.height, 768);
        assert_eq!(parsed.delivery.browser.base_path, "/");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scaffolded_tui_manifest_re_parses() {
        let files = managed_files("tui-app", InitShape::Tui);
        let manifest = files
            .iter()
            .find(|f| f.rel == Path::new("package.ipe"))
            .expect("init writes a package.ipe");

        let root = std::env::temp_dir().join("ipe_init_tui_roundtrip");
        let _ = std::fs::remove_dir_all(&root);
        write_stub_src(&root);
        let path = root.join("package.ipe");
        std::fs::write(&path, &manifest.content).expect("write scaffolded package.ipe");

        let parsed =
            crate::project::parse_manifest(&path).expect("scaffolded tui manifest re-parses");
        assert_eq!(parsed.name, "tui-app");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scaffolded_script_manifest_re_parses() {
        let files = managed_files("my-script", InitShape::Script);
        let manifest = files
            .iter()
            .find(|f| f.rel == Path::new("package.ipe"))
            .expect("init writes a package.ipe");

        let root = std::env::temp_dir().join("ipe_init_script_roundtrip");
        let _ = std::fs::remove_dir_all(&root);
        write_stub_src(&root);
        let path = root.join("package.ipe");
        std::fs::write(&path, &manifest.content).expect("write scaffolded package.ipe");

        let parsed =
            crate::project::parse_manifest(&path).expect("scaffolded script manifest re-parses");
        assert_eq!(parsed.name, "my-script");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn each_shape_scaffolds_a_distinct_main_ipe() {
        let web = managed_files("x", InitShape::Web);
        let tui = managed_files("x", InitShape::Tui);
        let cli = managed_files("x", InitShape::Cli);
        let server = managed_files("x", InitShape::Server);
        let script = managed_files("x", InitShape::Script);

        let main = |files: &[ManagedFile]| {
            files
                .iter()
                .find(|f| f.rel == Path::new("src/Main.ipe"))
                .map(|f| f.content.clone())
                .expect("a Main.ipe is scaffolded")
        };

        let web_main = main(&web);
        let tui_main = main(&tui);
        let cli_main = main(&cli);
        let server_main = main(&server);
        let script_main = main(&script);

        // Each shape uses a different entry point.
        assert!(
            web_main.contains("Web.app") || web_main.contains("app"),
            "web uses Web.app"
        );
        assert!(
            tui_main.contains("Tui.app") || tui_main.contains("Tui"),
            "tui uses Tui.app"
        );
        assert!(
            cli_main.contains("Cli.app") || cli_main.contains("Cli"),
            "cli uses Cli.app"
        );
        assert!(
            server_main.contains("Server.listen"),
            "server uses Server.listen"
        );
        assert!(script_main.contains("Task"), "script uses Task");

        // All five are distinct.
        let mains = [&web_main, &tui_main, &cli_main, &server_main, &script_main];
        for (i, a) in mains.iter().enumerate() {
            for (j, b) in mains.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "shapes {i} and {j} produce identical Main.ipe");
                }
            }
        }
    }

    #[test]
    fn shape_parse_round_trips() {
        for (token, expected) in [
            ("web", InitShape::Web),
            ("tui", InitShape::Tui),
            ("cli", InitShape::Cli),
            ("server", InitShape::Server),
            ("script", InitShape::Script),
        ] {
            assert_eq!(InitShape::parse(token), Some(expected), "parse {token}");
            assert_eq!(expected.label(), token, "label {token}");
        }
        assert_eq!(InitShape::parse("unknown"), None, "unknown shape is None");
    }

    #[test]
    fn delivery_defaults_in_scaffolded_manifest() {
        let files = managed_files("proj", InitShape::Web);
        let manifest = files
            .iter()
            .find(|f| f.rel == Path::new("package.ipe"))
            .expect("package.ipe present");

        let root = std::env::temp_dir().join("ipe_init_delivery_defaults");
        let _ = std::fs::remove_dir_all(&root);
        write_stub_src(&root);
        let path = root.join("package.ipe");
        std::fs::write(&path, &manifest.content).unwrap();

        let parsed = crate::project::parse_manifest(&path).expect("re-parses");
        // desktop defaults
        assert_eq!(parsed.delivery.desktop.width, 1024);
        assert_eq!(parsed.delivery.desktop.height, 768);
        assert_eq!(parsed.delivery.desktop.title, "proj");
        // browser default
        assert_eq!(parsed.delivery.browser.base_path, "/");
        // mobile default orientation
        assert_eq!(
            parsed.delivery.mobile.orientation,
            crate::project::ScreenOrientation::Portrait
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn library_scaffold_manifest_declares_exposed_modules_and_re_parses() {
        let files = library_files("my-cool-lib");
        let manifest = files
            .iter()
            .find(|f| f.rel == Path::new("package.ipe"))
            .expect("init --lib writes a package.ipe");

        assert!(
            files
                .iter()
                .any(|f| f.rel == Path::new("src").join("MyCoolLib.ipe")),
            "the public module src/MyCoolLib.ipe is scaffolded"
        );
        assert!(
            !files
                .iter()
                .any(|f| f.rel == Path::new("src").join("Main.ipe")),
            "a library does not scaffold a runnable src/Main.ipe"
        );

        let root = std::env::temp_dir().join("ipe_init_lib_roundtrip");
        let _ = std::fs::remove_dir_all(&root);
        let src = root.join("src");
        std::fs::create_dir_all(&src).expect("create src/");
        let path = root.join("package.ipe");
        std::fs::write(&path, &manifest.content).expect("write scaffolded package.ipe");

        let parsed =
            crate::project::parse_manifest(&path).expect("scaffolded lib manifest re-parses");
        assert_eq!(parsed.name, "my-cool-lib");
        assert_eq!(parsed.exposed_modules, vec!["MyCoolLib".to_owned()]);
        assert!(
            parsed.programs.is_empty(),
            "a library declares no runnable programs"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn module_name_derivation_produces_a_valid_segment() {
        assert_eq!(module_name_for("my-cool-lib"), "MyCoolLib");
        assert_eq!(module_name_for("json"), "Json");
        assert_eq!(module_name_for("ipe_http"), "IpeHttp");
        assert_eq!(module_name_for("Already"), "Already");
        assert_eq!(module_name_for("123"), "Lib");
        assert_eq!(module_name_for("---"), "Lib");
    }

    #[test]
    fn managed_set_omits_legacy_ipe_toml() {
        let files = managed_files("x", InitShape::default());
        assert!(
            !files.iter().any(|f| f.rel == Path::new("ipe.toml")),
            "init must not scaffold a legacy ipe.toml"
        );
    }

    #[test]
    fn health_offer_requires_tty() {
        assert!(!should_offer_health_check(false, false));
        assert!(!should_offer_health_check(false, true));
    }

    #[test]
    fn health_offer_suppressed_by_force() {
        assert!(!should_offer_health_check(true, true));
    }

    #[test]
    fn health_offer_on_interactive_non_forced() {
        assert!(should_offer_health_check(true, false));
    }

    #[test]
    fn all_shape_package_templates_contain_name_hole() {
        for shape in [
            InitShape::Web,
            InitShape::Tui,
            InitShape::Cli,
            InitShape::Server,
            InitShape::Script,
        ] {
            assert!(
                shape.package_ipe().contains("{name}"),
                "package template for shape '{}' is missing the {{name}} hole",
                shape.label()
            );
        }
    }
}
