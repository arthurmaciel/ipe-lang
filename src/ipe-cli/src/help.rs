//! The `ipe` help system — a data-driven renderer for the top-level help
//! screen and every per-command `--help` page.
//!
//! One table ([`COMMANDS`]) holds each command's synopsis, argument description,
//! and option list; the top-level screen groups them into [`SECTIONS`]. Both the
//! overview and the per-command pages render from that one source, so a command
//! or flag is described once.
//!
//! Colour is opt-in per output stream: ANSI escapes are emitted only when the
//! destination is a terminal and `NO_COLOR` is unset. Piped or redirected
//! output — and any run under `NO_COLOR` — is clean, aligned plain text.

use std::fmt::Write as _;
use std::io::IsTerminal;

use crate::CliError;
use crate::style::{Palette, REPO_URL, gutter, report_bugs_footer};

/// A command's dispatch handler: it receives the arguments after the command
/// name and runs the command.
pub(crate) type Handler = fn(&[String]) -> Result<(), CliError>;

/// A single command option: the flag form (e.g. `[--out <dir>]`, keeping its
/// `[]` optional syntax) and a one-line description.
struct Opt {
    /// The flag as it appears in a synopsis, including its `[]` and any value
    /// placeholder.
    flag: &'static str,
    /// A one-line description of what the flag does.
    desc: &'static str,
}

/// A command's registry entry: the single source of truth binding a command's
/// name and help metadata to the handler that runs it. Because the dispatcher
/// and the help renderer both read this one table, a command that is dispatched
/// but undescribed — or described but undispatched — cannot exist.
pub(crate) struct Command {
    /// The subcommand name (e.g. `build`).
    name: &'static str,
    /// The handler that runs the command, given the arguments after its name.
    run: Handler,
    /// A one-line description of the command, shown at the top of the command's
    /// own `--help` page.
    summary: &'static str,
    /// The positional arguments, rendered inline after the name on the synopsis
    /// line (e.g. `[<path>]`). Empty when the command takes none.
    args: &'static str,
    /// A one-line, plain-English description of the positional argument, shown
    /// under `Arguments:` on the command's `--help` page. Empty when there is no
    /// positional argument to explain.
    args_desc: &'static str,
    /// The optional flags, listed with descriptions on the command's `--help`
    /// page.
    options: &'static [Opt],
}

/// A titled group of commands on the top-level screen.
struct Section {
    /// The section heading (e.g. `Development`).
    title: &'static str,
    /// The command names in this section, in display order.
    commands: &'static [&'static str],
}

/// Every `ipe` command, each described exactly once.
const COMMANDS: &[Command] = &[
    Command {
        name: "init",
        run: crate::init::run_init,
        summary: "Scaffold a new Ipê project.",
        args: "[<name>]",
        args_desc: "The directory to create the project in (`.` for the current directory).",
        options: &[
            Opt {
                flag: "[--force]",
                desc: "overwrite a non-empty target directory",
            },
            Opt {
                flag: "[--lib]",
                desc: "scaffold a library package (exposedModules) instead of an application",
            },
        ],
    },
    Command {
        name: "build",
        run: crate::run_build,
        summary: "Compile a program to a native or WebAssembly artifact.",
        args: "[<path>]",
        args_desc: "A source file, a project directory, or a package.ipe. Defaults to the current project.",
        options: &[
            Opt {
                flag: "[--out <dir>]",
                desc: "write the emitted project to <dir>",
            },
            Opt {
                flag: "[--runtime <dir>]",
                desc: "vendor the Ipê runtime from <dir>",
            },
            Opt {
                flag: "[--emit-ir]",
                desc: "also emit the intermediate representation",
            },
            Opt {
                flag: "[--fix]",
                desc: "apply machine-applicable fixes before building",
            },
            Opt {
                flag: "[--accept-risks]",
                desc: "accept every disclosed .Unsafe escape-hatch import and proceed without prompting",
            },
            Opt {
                flag: "[--static]",
                desc: "produce a statically linked binary",
            },
            Opt {
                flag: "[--target <triple|wasm>]",
                desc: "cross-compile to <triple>, or build for the browser with wasm",
            },
            Opt {
                flag: "[--allocator <auto|system|dlmalloc|talc|mimalloc>]",
                desc: "select the global allocator (default: auto)",
            },
            Opt {
                flag: "[--allow-slow-allocator]",
                desc: "permit an allocator known to be slow for the target",
            },
            Opt {
                flag: "[--json]",
                desc: "emit each diagnostic as a stable JSON object (one per line) instead of the human layout",
            },
        ],
    },
    Command {
        name: "eject",
        run: crate::run_eject,
        summary: "Emit a self-contained Rust project with a tree-shaken runtime.",
        args: "[<path>]",
        args_desc: "A source file, a project directory, or a package.ipe. Defaults to the current project.",
        options: &[
            Opt {
                flag: "--out <dir>",
                desc: "write the standalone project to <dir> (required)",
            },
            Opt {
                flag: "[--runtime <dir>]",
                desc: "vendor the Ipê runtime source from <dir>",
            },
        ],
    },
    Command {
        name: "release",
        run: crate::run_release,
        summary: "Build the production artifact — optimised, Debug.* gated. Native-bearing apps get a jailed bundle; pure-native apps get a plain optimised binary; `--target wasm` produces a production browser bundle.",
        args: "[<path>]",
        args_desc: "A source file, a project directory, or a package.ipe. Defaults to the current project.",
        options: &[
            Opt {
                flag: "[--out <dir>]",
                desc: "write the artifact to <dir> (default: release/)",
            },
            Opt {
                flag: "[--target wasm|<triple>]",
                desc: "produce a browser bundle (`wasm`) or a musl-static binary for <triple> (default: x86_64-unknown-linux-musl)",
            },
            Opt {
                flag: "[--runtime <dir>]",
                desc: "vendor the Ipê runtime source from <dir>",
            },
            Opt {
                flag: "[--bundle]",
                desc: "native-bearing only: multi-file opt-out — wrapper + app + profile as siblings (app binary can be run directly, bypassing the sandbox)",
            },
            Opt {
                flag: "[--embed]",
                desc: "native-bearing only: default single self-jailing binary (app + profile fused into wrapper)",
            },
            Opt {
                flag: "[--capabilities] [--plain|--json]",
                desc: "print the inferred capability model for the app without building",
            },
        ],
    },
    Command {
        name: "type-check",
        run: crate::run_type_check,
        summary: "Type-check a program without building or running it.",
        args: "[<path>]",
        args_desc: "A source file, a project directory, or a package.ipe. Defaults to the current project.",
        options: &[Opt {
            flag: "[--json]",
            desc: "emit each diagnostic as a stable JSON object (one per line) on stderr; \
                   success is {\"status\":\"ok\"} on stdout",
        }],
    },
    Command {
        name: "test",
        run: crate::run_test,
        summary: "Build and run the project's tests/Main.ipe, reporting pass/fail.",
        args: "[<path>]",
        args_desc: "A source file, a project directory, or a package.ipe. Defaults to the current project.",
        options: &[Opt {
            flag: "[--json]",
            desc: "emit a compact {\"result\":…} verdict on stdout (non-zero exit on a failing case)",
        }],
    },
    Command {
        name: "verify",
        run: crate::run_verify,
        summary: "Run the whole project gate: format, type-check, build, then test.",
        args: "[<path>]",
        args_desc: "A source file, a project directory, or a package.ipe. Defaults to the current project.",
        options: &[Opt {
            flag: "[--json]",
            desc: "emit a compact gate verdict on stdout ({\"result\":…}; non-zero exit at the first failing stage)",
        }],
    },
    Command {
        name: "run",
        run: crate::run_run,
        summary: "Compile a program and run the resulting binary.",
        args: "[<path>]",
        args_desc: "A source file, a project directory, or a package.ipe. Defaults to the current project.",
        options: &[
            Opt {
                flag: "[--out <dir>]",
                desc: "write the emitted project to <dir>",
            },
            Opt {
                flag: "[--runtime <dir>]",
                desc: "vendor the Ipê runtime from <dir>",
            },
            Opt {
                flag: "[--static]",
                desc: "produce a statically linked binary",
            },
            Opt {
                flag: "[--target <triple>]",
                desc: "cross-compile to <triple>",
            },
            Opt {
                flag: "[--allocator <auto|system|dlmalloc|talc|mimalloc>]",
                desc: "select the global allocator (default: auto)",
            },
            Opt {
                flag: "[--allow-slow-allocator]",
                desc: "permit an allocator known to be slow for the target",
            },
            Opt {
                flag: "[--accept-risks]",
                desc: "accept every disclosed .Unsafe escape-hatch import and proceed without prompting",
            },
            Opt {
                flag: "[--json]",
                desc: "emit each diagnostic as a stable JSON object (one per line) instead of the human layout",
            },
            Opt {
                flag: "[-- <args>...]",
                desc: "forward <args> to the compiled program",
            },
        ],
    },
    Command {
        name: "exec",
        run: crate::run_exec,
        summary: "Run a built artifact, jailing native-bearing code to its embedded capability floor.",
        args: "[<artifact-dir>]",
        args_desc: "The build output directory to run (defaults to out/rust). A native-bearing \
                    artifact is confined to its embedded capability floor; a pure Ipê artifact runs \
                    directly.",
        options: &[Opt {
            flag: "[-- <args>...]",
            desc: "forward <args> to the artifact",
        }],
    },
    Command {
        name: "watch",
        run: crate::run_watch,
        summary: "Rebuild and re-run a program on every source change.",
        args: "[<path>]",
        args_desc: "A source file, a project directory, or a package.ipe. Defaults to the current project.",
        options: &[
            Opt {
                flag: "[--out <dir>]",
                desc: "write the emitted project to <dir>",
            },
            Opt {
                flag: "[--runtime <dir>]",
                desc: "vendor the Ipê runtime from <dir>",
            },
            Opt {
                flag: "[--port <n>]",
                desc: "serve on port <n> (default: 8000)",
            },
        ],
    },
    Command {
        name: "fix",
        run: crate::run_fix,
        summary: "Apply the compiler's machine-applicable fixes to a source file.",
        args: "<path>",
        args_desc: "The source file to fix.",
        options: &[Opt {
            flag: "[--yes]",
            desc: "apply every fix without per-edit confirmation",
        }],
    },
    Command {
        name: "fmt",
        run: crate::fmt::run_fmt,
        summary: "Format Ipê source files.",
        args: "[<path>]",
        args_desc: "A file or directory to format (`.` for the current directory).",
        options: &[
            Opt {
                flag: "[--check]",
                desc: "report unformatted files without rewriting them",
            },
            Opt {
                flag: "[--check --json|--plain]",
                desc: "with --check, emit the unformatted file list as JSON ({\"unformatted\":[…]}) or one path per line",
            },
            Opt {
                flag: "[--stdin]",
                desc: "format stdin to stdout (for editors and pipes); excludes <path>",
            },
        ],
    },
    Command {
        name: "clean",
        run: crate::clean::run_clean,
        summary: "Remove the project's build-generated output (out/, target/, .ipe/).",
        args: "",
        args_desc: "",
        options: &[],
    },
    Command {
        name: "add",
        run: crate::pkg::run_add,
        summary: "Add an Ipê package dependency (resolution ships with the index).",
        args: "<package>",
        args_desc: "The package name, optionally `@version`.",
        options: &[],
    },
    Command {
        name: "remove",
        run: crate::pkg::run_remove,
        summary: "Remove an Ipê package dependency.",
        args: "<package>",
        args_desc: "The package name.",
        options: &[],
    },
    Command {
        name: "rust",
        run: crate::ffi::run_rust,
        summary: "Manage Rust crates as foreign-function dependencies.",
        args: "<add|remove|install> [<args>...]",
        args_desc: "The action to run (add / remove / install) and its arguments.",
        options: &[
            Opt {
                flag: "[--features <a,b>]",
                desc: "add: enable the listed crate features",
            },
            Opt {
                flag: "[--yes]",
                desc: "add/install: skip the trust-summary confirmation prompt",
            },
            Opt {
                flag: "[--allow-build-scripts]",
                desc: "add/install: permit the crates' build scripts to run",
            },
            Opt {
                flag: "[--verbose]",
                desc: "add/install: show the full raw inspector log on failure",
            },
        ],
    },
    Command {
        name: "package",
        run: crate::run_package,
        summary: "Audit a package against the Tier-1 quality gate, publish it to the index, \
                  validate an index entry file, or run the index CI's authoritative \
                  receiving gate on a submitted entry.",
        args: "<audit|audit-entry|publish|validate-entry> [<path>]",
        args_desc: "The subcommand and its path: `audit`/`publish` take the project directory \
                    or `package.ipe` (defaults to the current project); `validate-entry` takes a \
                    `packages/<name>.toml` entry file (schema check only); `audit-entry` takes \
                    the same entry file and runs the full index CI receiving gate: schema, \
                    fetch+integrity-verify, and the complete Tier-1 (+ Tier-2) audit for every \
                    new version.",
        options: &[
            Opt {
                flag: "[--index <dir|repo>]",
                desc: "audit: read the previous published version from this index checkout; \
                       publish: the index repo the PR targets",
            },
            Opt {
                flag: "[--json|--plain]",
                desc: "audit: emit a compact certify verdict on stdout ({\"package\":…,\"certified\":…}; \
                       non-zero exit on a failing audit)",
            },
            Opt {
                flag: "[--dry-run]",
                desc: "publish: print the computed entry and intended PR, touch no network",
            },
            Opt {
                flag: "[--source <url>]",
                desc: "publish: the source URL to pin (overrides the git remote)",
            },
            Opt {
                flag: "[--rev <sha>]",
                desc: "publish: the revision to pin (overrides the committed HEAD)",
            },
            Opt {
                flag: "[--fork <owner>]",
                desc: "publish: the owner of your index fork to push to (defaults to the source \
                       owner)",
            },
        ],
    },
    Command {
        name: "login",
        run: crate::login::run_login,
        summary: "Authorize ipe with GitHub (device flow) and store a publish token.",
        args: "",
        args_desc: "",
        options: &[
            Opt {
                flag: "[--status]",
                desc: "report whether a token is stored",
            },
            Opt {
                flag: "[--logout]",
                desc: "remove the stored token",
            },
        ],
    },
    Command {
        name: "capabilities",
        run: crate::run_capabilities,
        summary: "Report the security capabilities a program exercises, inferred from its code.",
        args: "[<path>]",
        args_desc: "A source file, a project directory, or a package.ipe. Defaults to the current project.",
        options: &[
            Opt {
                flag: "[--plain]",
                desc: "print the bare capability names, one per line, flush-left",
            },
            Opt {
                flag: "[--json]",
                desc: "print {\"capabilities\":[…]} for jq",
            },
        ],
    },
    Command {
        name: "diff",
        run: crate::diff::run_diff,
        summary: "Compare two package versions' public APIs and report the required semver bump.",
        args: "<old> <new>  |  check <old> <new> <old-version> <new-version>",
        args_desc: "The two package paths to compare — the old version first, then the new. \
                    `check`: also reject a new version that does not clear the required bump \
                    (`--check <old-version> <new-version>` is a deprecated alias).",
        options: &[
            Opt {
                flag: "[--plain]",
                desc: "print flush-left change / bump records for grep/awk",
            },
            Opt {
                flag: "[--json]",
                desc: "print the report as a stable JSON object for jq",
            },
        ],
    },
    Command {
        name: "doc",
        run: crate::doc::run_doc,
        summary: "Look up documentation, generate API docs (docs.json + Markdown + HTML), query the stdlib, list modules, preview, or check coverage.",
        args: "[list | serve | check | <key> | <Module.Name>] [<path>]",
        args_desc: "Without a subcommand: generate docs.json + renderings for the project and stdlib. \
                    `<key>`: look up any entity by key — a diagnostic code (IPE-L0107), symbol (List.map), \
                    module (List), language construct (case), or CLI command (version). \
                    `list`: list all stdlib + project modules (one per line; `--list` is a deprecated alias). \
                    `serve`: build the HTML site and preview it on loopback. \
                    `check`: verify doc-comment coverage for project modules (stdlib is exempt). \
                    `<Module.Name>`: show one module's types and values with signatures (e.g. `ipe doc Ipe.List`).",
        options: &[
            Opt {
                flag: "[--out <dir>]",
                desc: "write the documentation to <dir> (default: doc/); generate only",
            },
            Opt {
                flag: "[--write-format markdown|json|html|all]",
                desc: "which renderings to write beside docs.json (default: all); generate only",
            },
            Opt {
                flag: "[--port <n>]",
                desc: "pin the serve port (default: an auto-selected free one); serve only",
            },
            Opt {
                flag: "[--plain]",
                desc: "bare output, one entry per line; list and <module> only",
            },
            Opt {
                flag: "[--json]",
                desc: "machine-readable JSON output; list and <module> only",
            },
        ],
    },
    Command {
        name: "lsp",
        run: crate::lsp::run_lsp,
        summary: "Run the language server over stdio.",
        args: "",
        args_desc: "",
        options: &[],
    },
    Command {
        name: "upgrade",
        run: crate::run_upgrade,
        summary: "Self-update ipe to the latest release (re-runs the installer).",
        args: "",
        args_desc: "",
        options: &[
            Opt {
                flag: "[--check]",
                desc: "report whether an upgrade is available, never install",
            },
            Opt {
                flag: "[--check --exit-code]",
                desc: "exit 10 = available, 0 = up-to-date, 2 = feed unreachable",
            },
            Opt {
                flag: "[--yes|-y]",
                desc: "skip the confirmation prompt (implied by non-TTY stdout)",
            },
            Opt {
                flag: "[--dry-run]",
                desc: "print the installer command without running it",
            },
            Opt {
                flag: "[--plain]",
                desc: "print one terse status line, flush-left (never prompts)",
            },
            Opt {
                flag: "[--json]",
                desc: "print the status as JSON (never prompts)",
            },
        ],
    },
    Command {
        name: "health",
        run: crate::health::run_health,
        summary: "Diagnose the build environment and offer consent-gated setup.",
        args: "",
        args_desc: "",
        options: &[
            Opt {
                flag: "[--yes|-y]",
                desc: "apply every suggested fix without prompting (for CI / provisioning)",
            },
            Opt {
                flag: "[--plain]",
                desc: "print one status record per line, flush-left (never mutates)",
            },
            Opt {
                flag: "[--json]",
                desc: "print the report as JSON for jq (never mutates)",
            },
        ],
    },
    Command {
        name: "version",
        run: crate::run_version,
        summary: "Print the ipe version.",
        args: "",
        args_desc: "",
        options: &[
            Opt {
                flag: "[--plain]",
                desc: "print the bare version string, flush-left",
            },
            Opt {
                flag: "[--json]",
                desc: "print {\"version\":\"…\"} for jq",
            },
        ],
    },
];

/// The top-level screen's command groups, in display order. Every command
/// appears in exactly one section.
const SECTIONS: &[Section] = &[
    Section {
        title: "Development",
        commands: &["init", "build", "run", "exec", "release", "watch"],
    },
    Section {
        title: "Quality",
        commands: &["type-check", "test", "verify"],
    },
    Section {
        title: "Using external packages",
        commands: &["add", "remove"],
    },
    Section {
        title: "Package authoring",
        commands: &["login", "package"],
    },
    Section {
        title: "Foreign-function interface (FFI)",
        commands: &["rust"],
    },
    Section {
        title: "Tools",
        commands: &[
            "doc",
            "fmt",
            "lsp",
            "clean",
            "health",
            "capabilities",
            "diff",
            "fix",
            "eject",
            "upgrade",
            "version",
        ],
    },
];

/// Look up a command's help entry by name.
fn find(name: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|c| c.name == name)
}

/// Whether `name` is a known command (drives `--help` interception in the
/// dispatcher).
#[must_use]
pub fn is_command(name: &str) -> bool {
    find(name).is_some()
}

/// Every known command name, in table order — the candidate set for suggesting
/// a near-miss when an unknown command is typed.
#[must_use]
pub fn command_names() -> Vec<&'static str> {
    COMMANDS.iter().map(|c| c.name).collect()
}

/// The canonical static name and handler that run `name`, or `None` when `name`
/// is not a known command. The static name is what misuse output keys its
/// `--help` page on.
///
/// Dispatch and help share this one table, so a command is dispatchable exactly
/// when it is described — the two can never drift apart.
pub(crate) fn handler(name: &str) -> Option<(&'static str, Handler)> {
    find(name).map(|c| (c.name, c.run))
}

/// The one-line summary for `name`, or `None` when `name` is not a known command.
///
/// Used to inject command metadata into the documentation index so that
/// `ipe doc <command>` resolves from the same SSOT as `ipe <command> --help`.
#[must_use]
pub fn command_summary(name: &str) -> Option<&'static str> {
    find(name).map(|c| c.summary)
}

/// Render the top-level help screen for the given output stream.
#[must_use]
pub fn top_level(stream: &impl IsTerminal) -> String {
    render_top_level(Palette::for_stream(stream))
}

/// Render a single command's `--help` page, or `None` if `name` is unknown.
#[must_use]
pub fn command(name: &str, stream: &impl IsTerminal) -> Option<String> {
    let p = Palette::for_stream(stream);
    find(name).map(|cmd| render_command(cmd, p))
}

/// The synopsis line for `cmd`: `ipe <name>` in yellow, then its arguments
/// inline in plain text.
fn command_line(cmd: &Command, p: &Palette) -> String {
    let mut line = format!("{}ipe {}{}", p.yellow, cmd.name, p.reset);
    if !cmd.args.is_empty() {
        line.push(' ');
        line.push_str(cmd.args);
    }
    line
}

/// Render the top-level overview: the header, then every command grouped by
/// section.
fn render_top_level(p: &Palette) -> String {
    let version = env!("CARGO_PKG_VERSION");
    let mut out = String::new();

    // Header: the product name in soft yellow, the URL dimmed.
    out.push('\n');
    let _ = writeln!(
        out,
        "{}Ipê language{} - v{version} - {}{REPO_URL}{}",
        p.yellow, p.reset, p.dim, p.reset
    );

    // The full list: every command by section, each shown as a ready-to-run
    // `ipe <command> --help`. The `--help` suffix aligns into one column within
    // the section (names padded to the section's widest) so the lines read as a
    // tidy block the reader can copy verbatim.
    for section in SECTIONS {
        out.push('\n');
        let _ = writeln!(out, "{}{}{}", p.bold, section.title, p.reset);
        let name_w = section
            .commands
            .iter()
            .filter_map(|n| find(n))
            .map(|c| c.name.len())
            .max()
            .unwrap_or(0);
        for &name in section.commands {
            let Some(cmd) = find(name) else { continue };
            let pad = name_w - cmd.name.len();
            let _ = writeln!(
                out,
                "  {}ipe {}{}{:pad$}  {}--help{}",
                p.yellow, cmd.name, p.reset, "", p.dim, p.reset,
            );
        }
    }

    // Footer: where to report bugs. The repository link already sits in the
    // header, so it is not repeated here.
    out.push('\n');
    let _ = writeln!(out, "{}{}{}", p.dim, report_bugs_footer(), p.reset);
    out.push('\n');
    gutter(&out)
}

/// Render one command's `--help` page: summary, synopsis, the positional
/// argument, then each option with its description.
///
/// The page is built flush-left, then indented once by the shared [`gutter`]
/// so every human line — this page IS a command's misuse output — sits off the
/// terminal edge at the one SSOT width. Within the gutter, `Arguments:` /
/// `Options:` bodies carry a further two-space indent so they read as nested
/// under their heading.
fn render_command(cmd: &Command, p: &Palette) -> String {
    let mut out = String::new();
    out.push('\n');
    let _ = writeln!(out, "{}{}{}", p.dim, cmd.summary, p.reset);
    out.push('\n');
    out.push_str(&command_line(cmd, p));
    out.push('\n');
    if !cmd.args_desc.is_empty() {
        out.push('\n');
        out.push_str("Arguments:\n");
        let _ = writeln!(out, "  {}{}{}", p.dim, cmd.args_desc, p.reset);
    }
    if !cmd.options.is_empty() {
        out.push('\n');
        out.push_str("Options:\n");
        let width = cmd.options.iter().map(|o| o.flag.len()).max().unwrap_or(0);
        for opt in cmd.options {
            let _ = writeln!(
                out,
                "  {:<width$}  {}{}{}",
                opt.flag, p.dim, opt.desc, p.reset
            );
        }
    }
    out.push('\n');
    gutter(&out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_top_level_names_every_command_and_section() {
        let plain = render_top_level(&Palette::PLAIN);
        for section in SECTIONS {
            assert!(
                plain.contains(section.title),
                "missing section {}",
                section.title
            );
        }
        for cmd in COMMANDS {
            assert!(
                plain.contains(&format!("ipe {}", cmd.name)),
                "missing command {}",
                cmd.name
            );
        }
        assert!(!plain.contains('\x1b'), "plain output must carry no ANSI");
    }

    #[test]
    fn colored_palette_emits_ansi_plain_does_not() {
        // The colour vs plain rendering is what we assert here; the TTY /
        // NO_COLOR gating that chooses between them is exercised by the
        // integration tests, which own a real process environment.
        assert!(
            render_top_level(&Palette::COLOR).contains('\x1b'),
            "colour must carry ANSI"
        );
        assert!(
            !render_top_level(&Palette::PLAIN).contains('\x1b'),
            "plain must not"
        );
    }

    #[test]
    fn every_command_has_help_page() {
        for cmd in COMMANDS {
            let page = render_command(cmd, &Palette::PLAIN);
            assert!(page.contains(&format!("ipe {}", cmd.name)));
            assert!(page.contains(cmd.summary));
        }
    }

    #[test]
    fn commands_with_an_argument_describe_it() {
        for cmd in COMMANDS {
            if cmd.args_desc.is_empty() {
                continue;
            }
            let page = render_command(cmd, &Palette::PLAIN);
            assert!(
                page.contains("Arguments:") && page.contains(cmd.args_desc),
                "missing argument description for {}",
                cmd.name
            );
        }
    }

    #[test]
    fn section_commands_show_an_aligned_help_suffix() {
        let plain = render_top_level(&Palette::PLAIN);
        for section in SECTIONS {
            // Every command line in a section carries a copy-pasteable `--help`,
            // and within the section the `--help` column is vertically aligned.
            let mut help_columns = Vec::new();
            for &name in section.commands {
                let needle = format!("ipe {name} ");
                let col = plain
                    .lines()
                    .find(|l| l.trim_start().starts_with(&needle))
                    .and_then(|l| l.find("--help"));
                assert!(
                    col.is_some(),
                    "`ipe {name}` in {} must render a copy-pasteable --help suffix",
                    section.title
                );
                help_columns.extend(col);
            }
            assert!(
                help_columns
                    .windows(2)
                    .all(|w| matches!(w, [a, b] if a == b)),
                "`--help` column misaligned in section {}: {help_columns:?}",
                section.title
            );
        }
    }

    #[test]
    fn sections_reference_only_known_commands() {
        for section in SECTIONS {
            for &name in section.commands {
                assert!(is_command(name), "section lists unknown command {name}");
            }
        }
    }

    #[test]
    fn every_command_appears_in_exactly_one_section() {
        for cmd in COMMANDS {
            let count = SECTIONS
                .iter()
                .flat_map(|s| s.commands)
                .filter(|&&n| n == cmd.name)
                .count();
            assert_eq!(
                count, 1,
                "command {} must appear in exactly one section, found {count}",
                cmd.name
            );
        }
    }

    // The single-source-of-truth invariant: dispatch and advertisement are the
    // same table, so every advertised command is dispatchable and vice versa.
    // A command described but unhandled — or handled but undescribed — is not
    // representable, and this pins that the registry is the sole driver.
    #[test]
    fn every_advertised_command_is_dispatchable() {
        for name in command_names() {
            assert!(
                handler(name).is_some(),
                "advertised command {name} has no dispatch handler"
            );
        }
    }

    #[test]
    fn exec_is_both_advertised_and_dispatchable() {
        assert!(is_command("exec"), "exec must be an advertised command");
        assert!(
            handler("exec").is_some(),
            "exec must resolve to a dispatch handler"
        );
    }
}
