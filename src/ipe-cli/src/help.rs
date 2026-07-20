//! The `ipe` help system — a data-driven renderer for the top-level help
//! screen and every per-command `--help` page.
//!
//! One table ([`COMMANDS`]) holds each command's synopsis and option list; the
//! top-level screen groups those commands into [`SECTIONS`]. Both the sectioned
//! overview and the single-command pages render from that one source, so a new
//! command or flag is described in exactly one place.
//!
//! Colour is opt-in per output stream: ANSI escapes are emitted only when the
//! destination is a terminal and `NO_COLOR` is unset. Piped or redirected
//! output — and any run under `NO_COLOR` — is clean, aligned plain text.

use std::fmt::Write as _;
use std::io::IsTerminal;

/// The repository home, shown in the header and footer.
const REPO_URL: &str = "https://github.com/arthurmaciel/ipe-lang";

/// A single command option: the flag form (e.g. `[--out <dir>]`, keeping its
/// `[]` optional syntax) and a one-line description.
struct Opt {
    /// The flag as it appears in a synopsis, including its `[]` and any value
    /// placeholder.
    flag: &'static str,
    /// A one-line description of what the flag does.
    desc: &'static str,
}

/// A command's help entry: the name, the mandatory-argument tail rendered
/// inline after the name, and the optional flags shown separately.
struct Command {
    /// The subcommand name (e.g. `build`).
    name: &'static str,
    /// A one-line description of the command, shown at the top of its own
    /// `--help` page.
    summary: &'static str,
    /// The mandatory positional arguments, rendered inline on the command line
    /// (e.g. `<crate-name>`). Empty when the command takes none.
    args: &'static str,
    /// The optional flags, each rendered dim and indented below the command
    /// line, and listed with descriptions on the command's `--help` page.
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
        summary: "Scaffold a new Ipê project in a directory.",
        args: "[<name>|.]",
        options: &[Opt {
            flag: "[--force]",
            desc: "overwrite a non-empty target directory",
        }],
    },
    Command {
        name: "build",
        summary: "Compile an Ipê program to a native or WebAssembly artifact.",
        args: "[<entry.ipe|project-dir|ipe.toml>]",
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
        summary: "Compile an Ipê program and run the resulting binary.",
        args: "[<entry.ipe|project-dir|ipe.toml>]",
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
        summary: "Rebuild and serve an Ipê program on every source change.",
        args: "[<entry.ipe|project-dir|ipe.toml>]",
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
        args: "<entry.ipe>",
        options: &[Opt {
            flag: "[--yes]",
            desc: "apply every fix without per-edit confirmation",
        }],
    },
    Command {
        name: "fmt",
        summary: "Format Ipê source files.",
        args: "[<path>|.]",
        options: &[Opt {
            flag: "[--check]",
            desc: "report unformatted files without rewriting them",
        }],
    },
    Command {
        name: "add",
        summary: "Add a Rust crate as a foreign-function dependency.",
        args: "<crate-name>[@<version>]",
        options: &[
            Opt {
                flag: "[--features <a,b>]",
                desc: "enable the listed crate features",
            },
            Opt {
                flag: "[--yes]",
                desc: "skip the trust-summary confirmation prompt",
            },
            Opt {
                flag: "[--allow-build-scripts]",
                desc: "permit the crate's build scripts to run",
            },
        ],
    },
    Command {
        name: "remove",
        summary: "Remove a foreign-function crate dependency.",
        args: "<crate-name>",
        options: &[],
    },
    Command {
        name: "install",
        summary: "(Re)inspect every foreign-function crate in the project's ipe.toml.",
        args: "",
        options: &[
            Opt {
                flag: "[--yes]",
                desc: "skip each trust-summary confirmation prompt",
            },
            Opt {
                flag: "[--allow-build-scripts]",
                desc: "permit the crates' build scripts to run",
            },
        ],
    },
    Command {
        name: "explain",
        summary: "Explain a diagnostic code, or list every code with no argument.",
        args: "[<CODE>]",
        options: &[],
    },
    Command {
        name: "lsp",
        summary: "Run the language server over stdio.",
        args: "",
        options: &[],
    },
    Command {
        name: "version",
        summary: "Print the ipe version.",
        args: "",
        options: &[],
    },
];

/// The top-level screen's command groups, in display order.
const SECTIONS: &[Section] = &[
    Section {
        title: "Development",
        commands: &["init", "build", "run", "watch", "fix", "fmt"],
    },
    Section {
        title: "Foreign-function interface (FFI)",
        commands: &["add", "remove", "install"],
    },
    Section {
        title: "Tools",
        commands: &["explain", "lsp", "version"],
    },
];

/// The ANSI palette, resolved once against the destination stream. When colour
/// is off every field is the empty string, so the same format code produces
/// clean plain text.
struct Palette {
    /// The Ipê-amarelo golden yellow, for the product name and command names.
    yellow: &'static str,
    /// A dim grey, for optional flags, descriptions, and the footer.
    dim: &'static str,
    /// A bold weight, for section titles.
    bold: &'static str,
    /// Resets all attributes.
    reset: &'static str,
}

impl Palette {
    /// The coloured palette: the golden Ipê-amarelo (256-colour 220) for names,
    /// a mid grey (244) for dim text.
    const COLOR: Self = Self {
        yellow: "\x1b[38;5;220m",
        dim: "\x1b[38;5;244m",
        bold: "\x1b[1m",
        reset: "\x1b[0m",
    };

    /// The plain palette: every escape is empty, yielding aligned plain text.
    const PLAIN: Self = Self {
        yellow: "",
        dim: "",
        bold: "",
        reset: "",
    };

    /// Select the coloured palette when `color` is on, else the plain one.
    const fn select(color: bool) -> &'static Self {
        if color { &Self::COLOR } else { &Self::PLAIN }
    }
}

/// Whether to emit ANSI to `stream`: only when it is a terminal and `NO_COLOR`
/// is unset (per <https://no-color.org>).
fn use_color(stream: &impl IsTerminal) -> bool {
    stream.is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

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
    render_top_level(Palette::select(use_color(stream)))
}

/// Render a single command's `--help` page, or `None` if `name` is unknown.
#[must_use]
pub fn command(name: &str, stream: &impl IsTerminal) -> Option<String> {
    let p = Palette::select(use_color(stream));
    find(name).map(|cmd| render_command(cmd, p))
}

/// The command line for `cmd`: `ipe <name>` in yellow, then its mandatory args
/// inline in plain text.
fn command_line(cmd: &Command, p: &Palette) -> String {
    let mut line = format!("{}ipe {}{}", p.yellow, cmd.name, p.reset);
    if !cmd.args.is_empty() {
        line.push(' ');
        line.push_str(cmd.args);
    }
    line
}

/// Render the sectioned top-level overview.
fn render_top_level(p: &Palette) -> String {
    let version = env!("CARGO_PKG_VERSION");
    let mut out = String::new();

    // Header: the product name in golden yellow, the URL dimmed.
    out.push('\n');
    let _ = writeln!(
        out,
        "{}Ipê language{} - v{version} - {}{REPO_URL}{}",
        p.yellow, p.reset, p.dim, p.reset
    );

    for section in SECTIONS {
        out.push('\n');
        let _ = writeln!(out, "{}{}{}", p.bold, section.title, p.reset);
        for &name in section.commands {
            let Some(cmd) = find(name) else { continue };
            out.push_str("  ");
            out.push_str(&command_line(cmd, p));
            out.push('\n');
            // Optional flags sit dim and indented below the command line, so the
            // mandatory form reads on the command line and the flags are
            // visually separate.
            for flag_line in wrap_flags(cmd.options) {
                let _ = writeln!(out, "{}      {}{}", p.dim, flag_line, p.reset);
            }
        }
    }

    // Footer.
    out.push('\n');
    let _ = writeln!(
        out,
        "{}Run `ipe <command> --help` for a command's options.\n{REPO_URL}{}",
        p.dim, p.reset
    );
    out
}

/// Render one command's `--help` page: summary, synopsis, then each option with
/// its description.
fn render_command(cmd: &Command, p: &Palette) -> String {
    let mut out = String::new();
    out.push('\n');
    let _ = writeln!(out, "{}{}{}", p.dim, cmd.summary, p.reset);
    out.push('\n');
    out.push_str(&command_line(cmd, p));
    out.push('\n');
    if !cmd.options.is_empty() {
        out.push('\n');
        out.push_str("Options:\n");
        let width = cmd.options.iter().map(|o| o.flag.len()).max().unwrap_or(0);
        for opt in cmd.options {
            let _ = writeln!(
                out,
                "  {}{:<width$}{}  {}{}{}",
                p.dim, opt.flag, p.reset, p.dim, opt.desc, p.reset
            );
        }
    }
    out
}

/// Group the flag forms into indentation-friendly lines, wrapping so no single
/// line runs too wide. Each returned line is a space-joined run of flags.
fn wrap_flags(options: &[Opt]) -> Vec<String> {
    /// The soft width at which a flag run wraps to the next indented line.
    const WRAP_AT: usize = 66;
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for opt in options {
        if !current.is_empty() && current.len() + 1 + opt.flag.len() > WRAP_AT {
            lines.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(opt.flag);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
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
    fn sections_reference_only_known_commands() {
        for section in SECTIONS {
            for &name in section.commands {
                assert!(is_command(name), "section lists unknown command {name}");
            }
        }
    }
}
