//! The `ipe` help system — a data-driven renderer for the top-level help
//! screen and every per-command `--help` page.
//!
//! One table ([`COMMANDS`]) holds each command's synopsis, argument description,
//! and option list; the top-level screen highlights a few in [`MOST_USED`] and
//! groups all of them into [`SECTIONS`]. Both the overview and the per-command
//! pages render from that one source, so a command or flag is described once.
//!
//! Colour is opt-in per output stream: ANSI escapes are emitted only when the
//! destination is a terminal and `NO_COLOR` is unset. Piped or redirected
//! output — and any run under `NO_COLOR` — is clean, aligned plain text.

use std::fmt::Write as _;
use std::io::IsTerminal;

use crate::style::{Palette, REPO_URL, gutter, report_bugs_footer};

/// A single command option: the flag form (e.g. `[--out <dir>]`, keeping its
/// `[]` optional syntax) and a one-line description.
struct Opt {
    /// The flag as it appears in a synopsis, including its `[]` and any value
    /// placeholder.
    flag: &'static str,
    /// A one-line description of what the flag does.
    desc: &'static str,
}

/// A command's help entry: the name, its positional-argument tail, a plain
/// description of that argument, and the optional flags.
struct Command {
    /// The subcommand name (e.g. `build`).
    name: &'static str,
    /// A one-line description of the command, shown both under `Most used
    /// commands` and at the top of the command's own `--help` page.
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
        summary: "Scaffold a new Ipê project.",
        args: "[<name>]",
        args_desc: "The directory to create the project in (`.` for the current directory).",
        options: &[Opt {
            flag: "[--force]",
            desc: "overwrite a non-empty target directory",
        }],
    },
    Command {
        name: "build",
        summary: "Compile a program to a native or WebAssembly artifact.",
        args: "[<path>]",
        args_desc: "A source file, a project directory, or an ipe.toml. Defaults to the current project.",
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
        ],
    },
    Command {
        name: "run",
        summary: "Compile a program and run the resulting binary.",
        args: "[<path>]",
        args_desc: "A source file, a project directory, or an ipe.toml. Defaults to the current project.",
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
                flag: "[-- <args>...]",
                desc: "forward <args> to the compiled program",
            },
        ],
    },
    Command {
        name: "watch",
        summary: "Rebuild and re-run a program on every source change.",
        args: "[<path>]",
        args_desc: "A source file, a project directory, or an ipe.toml. Defaults to the current project.",
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
        summary: "Format Ipê source files.",
        args: "[<path>]",
        args_desc: "A file or directory to format (`.` for the current directory).",
        options: &[Opt {
            flag: "[--check]",
            desc: "report unformatted files without rewriting them",
        }],
    },
    Command {
        name: "add",
        summary: "Add an Ipê package dependency (resolution ships with the index).",
        args: "<package>",
        args_desc: "The package name, optionally `@version`.",
        options: &[],
    },
    Command {
        name: "remove",
        summary: "Remove an Ipê package dependency.",
        args: "<package>",
        args_desc: "The package name.",
        options: &[],
    },
    Command {
        name: "rust",
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
        ],
    },
    Command {
        name: "package",
        summary: "Audit a package against the Tier-1 quality gate before publishing.",
        args: "audit [<path>]",
        args_desc: "The subcommand (audit) and the project directory or ipe.toml to audit \
                    (defaults to the current project).",
        options: &[Opt {
            flag: "[--index <dir>]",
            desc: "audit: read the previous published version from this index checkout",
        }],
    },
    Command {
        name: "explain",
        summary: "Explain a diagnostic code, or list every code with no argument.",
        args: "[<code>]",
        args_desc: "A diagnostic code such as IPE-L0131. Omit to list every code.",
        options: &[
            Opt {
                flag: "[--plain]",
                desc: "list codes flush-left, tab-separated (code<TAB>title), for grep/awk",
            },
            Opt {
                flag: "[--json]",
                desc: "list codes as {\"codes\":[{\"code\",\"title\"}]} for jq",
            },
        ],
    },
    Command {
        name: "capabilities",
        summary: "Report the security capabilities a program exercises, inferred from its code.",
        args: "[<path>]",
        args_desc: "A source file, a project directory, or an ipe.toml. Defaults to the current project.",
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
        summary: "Compare two package versions' public APIs and report the required semver bump.",
        args: "<old> <new>",
        args_desc: "The two package paths to compare — the old version first, then the new.",
        options: &[
            Opt {
                flag: "[--check <old-version> <new-version>]",
                desc: "reject a new version that does not clear the required bump",
            },
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
        name: "lsp",
        summary: "Run the language server over stdio.",
        args: "",
        args_desc: "",
        options: &[],
    },
    Command {
        name: "version",
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

/// The handful of commands a newcomer reaches for first, highlighted with a
/// one-line description above the full sectioned list.
const MOST_USED: &[&str] = &["init", "run", "watch"];

/// The top-level screen's command groups, in display order. Every command
/// appears in exactly one section (the `MOST_USED` highlights repeat here).
const SECTIONS: &[Section] = &[
    Section {
        title: "Development",
        commands: &["init", "build", "run", "watch", "fix", "fmt"],
    },
    Section {
        title: "Using external packages",
        commands: &["add", "remove"],
    },
    Section {
        title: "Package authoring",
        commands: &["package"],
    },
    Section {
        title: "Foreign-function interface (FFI)",
        commands: &["rust"],
    },
    Section {
        title: "Tools",
        commands: &["explain", "capabilities", "diff", "lsp", "version"],
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

/// Render the top-level overview: the header, a highlighted `Most used
/// commands` block, then every command grouped by section.
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

    // Most-used commands: each name with a one-line description below it.
    out.push('\n');
    let _ = writeln!(out, "{}Most used commands:{}", p.bold, p.reset);
    for &name in MOST_USED {
        let Some(cmd) = find(name) else { continue };
        let _ = writeln!(out, "  {}ipe {}{}", p.yellow, cmd.name, p.reset);
        let _ = writeln!(out, "{}      {}{}", p.dim, cmd.summary, p.reset);
    }

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
    fn most_used_commands_appear_with_their_summaries() {
        let plain = render_top_level(&Palette::PLAIN);
        for &name in MOST_USED {
            let cmd = find(name).expect("most-used command exists");
            assert!(plain.contains(cmd.summary), "missing summary for {name}");
        }
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
}
