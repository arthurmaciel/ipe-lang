//! `ipe doc` — API documentation generation.
//!
//! Generates reference documentation for an Ipê package from its own source: the
//! public API a consumer sees, each entry carrying its checker-inferred type
//! signature, its `-- |` doc-comment, and a stable source location.
//!
//! ## Output layout
//!
//! Each rendering writes into its own subfolder of the output base (`doc/` by
//! default, overridden with `--out DIR`):
//!
//! - `<base>/json/docs.json` — the machine-readable source of truth.
//! - `<base>/markdown/<Module>.md` and `<base>/markdown/index.md` — Markdown views.
//! - `<base>/html/index.html`, `<base>/html/<Module>.html`, `<base>/html/style.css`
//!   — the self-contained HTML site.
//!
//! Cross-links within each format are relative to that format's subfolder. The
//! logical anchor scheme (`Module#Name`) is format-neutral and identical across
//! all three renderings.
//!
//! ## Stdlib documentation without a project
//!
//! The command succeeds in any directory. When no project source is reachable at
//! `PATH`, it falls back to stdlib-only documentation rather than erroring. Running
//! `ipe doc --write-format html` in an empty directory writes the full stdlib
//! reference to `doc/html/`.
//!
//! ## Module index hierarchy
//!
//! The module listing in HTML and Markdown is a namespace tree — `Ipe.Db.Codec`
//! nests under `Ipe.Db`, not in a flat alphabetical list. A prefix that has no
//! module at its exact dotted path renders as a non-link section header.
//!
//! ## Command surface
//!
//! The command surface is a closed [`DocMode`] parsed at the CLI boundary, so an
//! invalid flag combination is unrepresentable downstream (parse, don't validate;
//! make-invalid-states-unrepresentable):
//!
//! * `ipe doc [PATH] [--out DIR] [--write-format markdown|json|html|all]` — generate
//!   renderings under `<out>/{json,markdown,html}/`. Works without a project.
//!   Includes all stdlib modules (both compiled-source and kernel-backed) alongside
//!   any project modules found at `PATH`.
//! * `ipe doc list [--plain|--json]` — list all stdlib modules (and project modules),
//!   one per line. Default: guttered human output; `--plain`: bare names; `--json`:
//!   `{"modules":[…]}`. The former `--list` flag is a deprecated alias that still
//!   works and prints a notice pointing at `list`.
//! * `ipe doc <MODULE> [--plain|--json]` — dump one module's exposed types, values, and
//!   functions with their type signatures (e.g. `ipe doc Ipe.List`). Default: human;
//!   `--plain`: flush-left; `--json`: stable structured record.
//! * `ipe doc serve [PATH] [--port N]` — build the HTML site and preview it
//!   read-only on `http://127.0.0.1:<port>` (loopback only; the port defaults to
//!   an auto-selected free one).
//! * `ipe doc check [PATH]` — a coverage gate that writes nothing and exits
//!   non-zero when an exposed binding lacks a doc-comment. Stdlib modules are exempt.
//! * `ipe doc --check-examples` — extract every fenced ` ```ipe ` block from every
//!   `{-| … -}` doc-string in the standard library and type-check each one. A block
//!   that carries `-->` result annotations is also compiled and run (when `IPE_E2E=1`
//!   is set) and the printed output is asserted against the annotation. Exits non-zero
//!   when any block fails its expectation.
//!
//! The machine-readable [`docs.json`](DocsJson) is the source of truth — one
//! record per exposed module, with the module's doc-comment and its exposed
//! unions and values (name + type + comment + resolved cross-references). Both
//! the Markdown and the self-contained HTML site are pure views over that same
//! in-memory model. The schema is versioned ([`DOCS_JSON_VERSION`]) so a
//! downstream consumer can rely on it.
//!
//! ## Cross-references
//!
//! Every type name in a rendered signature that resolves — via the
//! canonicaliser's already-computed [`TyDoc`] identity, never a text guess — to a
//! type documented in this package becomes a link to that entry's stable anchor
//! (`Module#Name`, identical across json / Markdown / HTML). A built-in with no
//! in-package definition (e.g. `Int`) renders as plain text.
//!
//! ## Two provenances, joined by name
//!
//! Type signatures come from the type checker — this module reuses
//! [`crate::api_surface::extract_tree`], the same projection `ipe diff` uses, so
//! a documented type is exactly the checked type and is never re-parsed. Ipê's
//! lexer discards every `--` comment before the AST exists, so the `-- |`
//! doc-comments are recovered here at the driver boundary by a source scan
//! ([`scan_doc_comments`]) that is joined to the checked surface by binding name.
//! Neither pass runs the emit tier — `ipe doc` needs types, not code.
//!
//! ## Stdlib coverage
//!
//! Two kinds of stdlib module exist:
//!
//! * **Compiled-source** (`ipe_stdlib::COMPILED_STD_MODULES`): modules with embedded Ipê
//!   source (e.g. `Ipe.Css`, `Ipe.Test`). These are type-checked as embedded stdlib
//!   (so they may declare the reserved `Ipe.*` namespace a user module may not); their
//!   doc-comments are scanned from the embedded source.
//! * **Kernel-qualifier** (`ipe_canon::STDLIB_MODULE_QUALIFIERS`): modules backed by the
//!   kernel registry (e.g. `Ipe.List`, `Ipe.String`). Type signatures come from
//!   [`ipe_types::kernel_type_table`]; doc-comments are absent (signatures are the
//!   contract).
//!
//! `ipe doc check` exempts stdlib modules from the coverage gate — signatures are
//! sufficient; prose is optional for compiler-internal stdlib.
//!
//! Still out of scope (tracked separately): inter-*package* linking (needs the
//! package index), full-text search, and remote hosting. `serve` is a local
//! read-only preview of the static site, never a publish path.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use ipe_diagnostics::{TyDoc, render_ty};
use ipe_docs::{CommandInfo, Index};
use ipe_intern::Interner;
use ipe_types::{VarNamer, kernel_type_table, ty_to_doc};

use crate::CliError;
use crate::api_surface::{ModuleApi, ModulePath, PublicApi, UnionApi, extract_tree, read_tree};
use crate::cli_args::OutputFormat;

/// The `docs.json` schema version. Bumped only on an incompatible shape change,
/// so a consumer can refuse a document it does not understand rather than
/// mis-reading it.
pub const DOCS_JSON_VERSION: u32 = 1;

/// Which renderings `ipe doc` writes — a closed set, so an unknown `--write-format`
/// value is rejected at the CLI boundary rather than carried downstream.
///
/// `docs.json` (the machine-readable source of truth) is always written; a
/// [`WriteFormat`] selects which human-facing view(s) are written beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteFormat {
    /// `docs.json` only — the machine-readable source of truth.
    Json,
    /// `docs.json` + per-module Markdown.
    Markdown,
    /// `docs.json` + the self-contained HTML site.
    Html,
    /// `docs.json` + Markdown + HTML (the default).
    All,
}

impl WriteFormat {
    /// Whether this format writes the per-module Markdown.
    const fn wants_markdown(self) -> bool {
        matches!(self, Self::Markdown | Self::All)
    }

    /// Whether this format writes the self-contained HTML site.
    const fn wants_html(self) -> bool {
        matches!(self, Self::Html | Self::All)
    }
}

/// What `ipe doc` was asked to do — a closed set.
///
/// No code past the parser can hold an invalid mix
/// (make-invalid-states-unrepresentable). `Generate` carries only its own flags
/// (`--out`, `--write-format`); `Serve` carries only `--port`; `Check` carries
/// none; `List` and `Query` carry only `--plain`/`--json` — so mixing flags
/// across subcommands has no representation to construct.
#[derive(Debug, PartialEq, Eq)]
pub enum DocMode {
    /// Write `docs.json` and the selected renderings to `out`.
    Generate {
        /// The package to document — a directory or a single `.ipe` file.
        path: PathBuf,
        /// Where the rendered documentation is written.
        out: PathBuf,
        /// Which human-facing renderings are written beside `docs.json`.
        write_format: WriteFormat,
    },
    /// Build the HTML site and serve it read-only on loopback.
    Serve {
        /// The package to document — a directory or a single `.ipe` file.
        path: PathBuf,
        /// The loopback port to bind. `None` auto-selects a free one (bind
        /// `127.0.0.1:0`, let the OS assign); `Some(n)` pins it and errors if it
        /// is taken.
        port: Option<u16>,
    },
    /// Verify every exposed binding is documented; write nothing.
    Check {
        /// The package to check (only project modules, never stdlib).
        path: PathBuf,
    },
    /// List all stdlib + project module names.
    List {
        /// The package whose modules are listed alongside stdlib (defaults to `.`).
        path: PathBuf,
        /// How to render the list.
        format: OutputFormat,
    },
    /// Print one module's exposed types and values with their type signatures.
    Query {
        /// The dotted module name to look up (e.g. `Ipe.List`).
        module: String,
        /// How to render the result.
        format: OutputFormat,
    },
    /// Type-check every fenced ` ```ipe ` block in every `{-| … -}` doc-string
    /// across the standard library. When `IPE_E2E=1` is set, blocks with `-->`
    /// result annotations are also compiled, run, and their output asserted.
    /// Exits non-zero when any block fails its expectation.
    CheckExamples,
    /// Look up any entity by key via the `ipe_docs` index.
    ///
    /// Accepts symbols (`List.map`), modules (`List`), diagnostic codes
    /// (`IPE-L0107`), language constructs (`case`), and CLI commands (`version`).
    Lookup {
        /// The documentation key to resolve.
        key: String,
        /// How to render the result.
        format: OutputFormat,
    },
}

/// The default output directory when `--out` is omitted, mirroring Elm's `doc/`.
const DEFAULT_OUT: &str = "doc";

/// The default package path when a positional is omitted — the current project.
const DEFAULT_PATH: &str = ".";

/// The `ipe doc` subcommand a leading token selects. The bare form (no leading
/// subcommand) is `generate`.
enum Sub {
    Generate,
    Serve,
    Check,
    List,
    Query(String),
    CheckExamples,
    Lookup(String),
}

/// Accumulated flag values while parsing `ipe doc`'s argument tail.
#[derive(Default)]
struct ParsedFlags {
    path: Option<String>,
    out: Option<String>,
    write_format: Option<WriteFormat>,
    port: Option<u16>,
    output_format: Option<OutputFormat>,
}

/// Parse `ipe doc`'s argument tail into a [`DocMode`].
///
/// The bare form is `generate`; a leading bare word — `serve`, `check`, or
/// `list` — selects that mode, and a leading non-flag positional that looks like
/// a dotted module name (starts with an uppercase letter) selects `query`. The
/// legacy `--list` flag is a deprecated alias for the `list` mode, kept
/// dispatchable so it does not break existing invocations. Each mode accepts
/// ONLY its own flags, so a flag meaningless for the chosen mode is rejected
/// here rather than carried into an unrepresentable [`DocMode`].
///
/// # Errors
/// [`CliError::Usage`] / [`CliError::UsageOwned`] naming the exact problem.
pub fn parse_doc(rest: &[String]) -> Result<DocMode, CliError> {
    parse_doc_with(rest, &mut |msg| eprintln!("{msg}"))
}

/// The stderr notice emitted when the deprecated `--list` alias is used.
const LIST_DEPRECATION_NOTICE: &str =
    "note: `ipe doc --list` is deprecated; use `ipe doc list` instead";

/// [`parse_doc`] with the deprecation-notice sink injected, so a test can
/// observe the alias notice without inspecting a process's stderr.
fn parse_doc_with(rest: &[String], notice: &mut dyn FnMut(&str)) -> Result<DocMode, CliError> {
    let mut it = rest.iter().peekable();

    // `--check-examples` selects the doc-test gate; detected before other flags.
    let has_check_examples = rest.iter().any(|s| s == "--check-examples");

    // The deprecated `--list` flag still selects the `list` mode. It is a
    // flag-style alias for the bare `list` word, detected before the positional
    // scan so it works in any position.
    let has_list_flag = rest.iter().any(|s| s == "--list");
    if has_list_flag {
        notice(LIST_DEPRECATION_NOTICE);
    }

    let sub = if has_check_examples {
        Sub::CheckExamples
    } else if has_list_flag {
        // Consume any `--list` flag found; the rest are positional/format flags.
        Sub::List
    } else {
        match it.peek().map(|s| s.as_str()) {
            Some("serve") => {
                it.next();
                Sub::Serve
            }
            Some("check") => {
                it.next();
                Sub::Check
            }
            Some("list") => {
                it.next();
                Sub::List
            }
            // A diagnostic code (`IPE-X0000`) or a symbol key (`List.map`) routes
            // to the content index. A diagnostic code always starts uppercase and
            // contains a `-`; a symbol key starts uppercase and contains a `.`
            // followed by a lowercase letter.
            Some(first)
                if !first.starts_with('-')
                    && first.chars().next().is_some_and(|c| c.is_ascii_uppercase())
                    && (first.contains('-') || is_symbol_key(first)) =>
            {
                let key = (*first).to_owned();
                it.next();
                Sub::Lookup(key)
            }
            // A module-path positional (uppercase, no `-` or symbol pattern) is a
            // module API query.
            Some(first)
                if !first.starts_with('-')
                    && first.chars().next().is_some_and(|c| c.is_ascii_uppercase()) =>
            {
                let name = (*first).to_owned();
                it.next();
                Sub::Query(name)
            }
            // A lowercase bare word is a content-index lookup key only when no
            // generate-specific flags (`--out`, `--write-format`) appear in the
            // remaining arguments — those flags are unambiguous signals that the
            // word is a project path for the `generate` subcommand.
            Some(first)
                if !first.starts_with('-')
                    && !first.is_empty()
                    && !rest.iter().any(|a| a == "--out" || a == "--write-format") =>
            {
                let key = (*first).to_owned();
                it.next();
                Sub::Lookup(key)
            }
            _ => Sub::Generate,
        }
    };

    let mut flags = ParsedFlags::default();
    parse_doc_flags(&mut it, &sub, &mut flags)?;

    let path = PathBuf::from(flags.path.as_deref().unwrap_or(DEFAULT_PATH));
    let format = flags.output_format.unwrap_or_default();
    Ok(match sub {
        Sub::Generate => DocMode::Generate {
            path,
            out: PathBuf::from(flags.out.as_deref().unwrap_or(DEFAULT_OUT)),
            write_format: flags.write_format.unwrap_or(WriteFormat::All),
        },
        Sub::Serve => DocMode::Serve {
            path,
            port: flags.port,
        },
        Sub::Check => DocMode::Check { path },
        Sub::List => DocMode::List { path, format },
        Sub::Query(module) => DocMode::Query { module, format },
        Sub::CheckExamples => DocMode::CheckExamples,
        Sub::Lookup(key) => DocMode::Lookup { key, format },
    })
}

/// Parse the flag portion of `ipe doc`'s argument tail into [`ParsedFlags`].
///
/// Called from [`parse_doc`] after the leading subcommand token has been
/// consumed. Rejects any flag that does not belong to `sub`.
///
/// # Errors
/// [`CliError::Usage`] / [`CliError::UsageOwned`] for an unknown or misplaced flag.
fn parse_doc_flags(
    it: &mut std::iter::Peekable<std::slice::Iter<'_, String>>,
    sub: &Sub,
    flags: &mut ParsedFlags,
) -> Result<(), CliError> {
    while let Some(arg) = it.next() {
        match arg.as_str() {
            // Skip flags already handled by the caller.
            "--list" | "--check-examples" => {}
            "--out" | "--write-format" if !matches!(sub, Sub::Generate) => {
                return Err(CliError::UsageOwned(format!(
                    "ipe doc {}: {arg} is a generate-only flag; run `ipe doc` to write files",
                    sub_name(sub)
                )));
            }
            "--port" if !matches!(sub, Sub::Serve) => {
                return Err(CliError::UsageOwned(format!(
                    "ipe doc {}: --port applies only to `ipe doc serve`",
                    sub_name(sub)
                )));
            }
            "--plain" | "--json" if !matches!(sub, Sub::List | Sub::Query(_) | Sub::Lookup(_)) => {
                return Err(CliError::UsageOwned(format!(
                    "ipe doc {}: {arg} applies only to `list`, `<module>` queries, and `<key>` lookups",
                    sub_name(sub)
                )));
            }
            "--out" => {
                let value = it
                    .next()
                    .cloned()
                    .ok_or(CliError::Usage("ipe doc: --out needs a directory"))?;
                if flags.out.is_some() {
                    return Err(CliError::Usage("ipe doc: --out given more than once"));
                }
                flags.out = Some(value);
            }
            "--write-format" => {
                let value = it
                    .next()
                    .ok_or(CliError::Usage("ipe doc: --write-format needs a value"))?;
                if flags.write_format.is_some() {
                    return Err(CliError::Usage(
                        "ipe doc: --write-format given more than once",
                    ));
                }
                flags.write_format = Some(parse_write_format(value)?);
            }
            "--port" => {
                let value = it
                    .next()
                    .ok_or(CliError::Usage("ipe doc serve: --port needs a number"))?;
                if flags.port.is_some() {
                    return Err(CliError::Usage(
                        "ipe doc serve: --port given more than once",
                    ));
                }
                flags.port = Some(parse_port(value)?);
            }
            "--plain" => {
                if flags.output_format.is_some() {
                    return Err(CliError::Usage(
                        "ipe doc: --plain and --json are mutually exclusive",
                    ));
                }
                flags.output_format = Some(OutputFormat::Plain);
            }
            "--json" => {
                if flags.output_format.is_some() {
                    return Err(CliError::Usage(
                        "ipe doc: --plain and --json are mutually exclusive",
                    ));
                }
                flags.output_format = Some(OutputFormat::Json);
            }
            flag if flag.starts_with('-') => {
                return Err(CliError::UsageOwned(format!(
                    "ipe doc: unknown flag `{flag}`"
                )));
            }
            positional => {
                if matches!(sub, Sub::Query(_) | Sub::Lookup(_)) {
                    return Err(CliError::Usage("ipe doc: expected a single <key> argument"));
                }
                if flags.path.is_some() {
                    return Err(CliError::Usage(
                        "ipe doc: expected a single <path> argument",
                    ));
                }
                flags.path = Some(positional.to_owned());
            }
        }
    }
    Ok(())
}

/// The subcommand's name for an error message.
const fn sub_name(sub: &Sub) -> &'static str {
    match sub {
        Sub::Generate => "generate",
        Sub::Serve => "serve",
        Sub::Check => "check",
        Sub::List => "list",
        Sub::Query(_) => "<module>",
        Sub::CheckExamples => "--check-examples",
        Sub::Lookup(_) => "<key>",
    }
}

/// Parse a `--write-format` value into a [`WriteFormat`], rejecting an unknown spelling.
fn parse_write_format(value: &str) -> Result<WriteFormat, CliError> {
    match value {
        "json" => Ok(WriteFormat::Json),
        "markdown" => Ok(WriteFormat::Markdown),
        "html" => Ok(WriteFormat::Html),
        "all" => Ok(WriteFormat::All),
        other => Err(CliError::UsageOwned(format!(
            "ipe doc: unknown --write-format `{other}` (want markdown | json | html | all)"
        ))),
    }
}

/// Parse a `--port` value into a `u16`, rejecting a non-numeric or out-of-range
/// value (and `0`, which would silently auto-select — omit `--port` for that).
fn parse_port(value: &str) -> Result<u16, CliError> {
    match value.parse::<u16>() {
        Ok(0) => Err(CliError::Usage(
            "ipe doc serve: --port 0 is not a real port; omit --port to auto-select a free one",
        )),
        Ok(p) => Ok(p),
        Err(_) => Err(CliError::UsageOwned(format!(
            "ipe doc serve: --port `{value}` is not a port number (1-65535)"
        ))),
    }
}

/// Language construct pages embedded at compile time from `docs/content/`.
///
/// Each entry is `(key, markdown_text)`. The key becomes the index entry key
/// (e.g. `"case"` resolves via `ipe doc case`). Adding a new construct page
/// requires a new `docs/content/<name>.md` and a new entry here.
static CONTENT_PAGES: &[(&str, &str)] = &[
    ("case", include_str!("../../../docs/content/case.md")),
    ("do", include_str!("../../../docs/content/do.md")),
];

/// Return `true` when `s` looks like a symbol key: it starts uppercase, contains
/// a `.`, and the character after the first `.` is lowercase — e.g. `List.map`,
/// `Ipe.List.map`. This distinguishes a symbol lookup from a plain module name
/// (`Ipe.List`, `List`) which is routed to the API query instead.
fn is_symbol_key(s: &str) -> bool {
    let Some(dot_pos) = s.find('.') else {
        return false;
    };
    s.get(dot_pos + 1..)
        .and_then(|after| after.chars().next())
        .is_some_and(|c| c.is_ascii_lowercase())
}

/// Build the `ipe_docs` index, wiring in the CLI command registry.
///
/// Diagnostics are indexed from the compile-time embedded explain pages (same
/// source as `ipe explain`) — no filesystem lookup required. Language constructs
/// are indexed from the embedded `docs/content/*.md` files via a compile-time
/// static table. Commands are injected from `help.rs`'s `COMMANDS` registry.
fn build_index() -> Result<Index, CliError> {
    use ipe_docs::IndexBuilder;
    let mut builder = IndexBuilder::new();

    builder
        .add_stdlib()
        .map_err(|e| CliError::UsageOwned(format!("ipe doc: stdlib index failed: {e}")))?;

    // Diagnostics: indexed from the compile-time embedded explain pages.
    for code in ipe_diagnostics::ALL_CODES {
        let text = ipe_diagnostics::explain_page(*code)
            .unwrap_or("")
            .to_owned();
        builder.insert(
            code.as_str().to_owned(),
            ipe_docs::Entry {
                kind: ipe_docs::EntryKind::Diagnostic,
                source_key: code.as_str().to_owned(),
                text,
            },
        );
    }

    // Constructs: compile-time embedded content pages.
    for (key, text) in CONTENT_PAGES {
        builder.insert(
            (*key).to_owned(),
            ipe_docs::Entry {
                kind: ipe_docs::EntryKind::Construct,
                source_key: (*key).to_owned(),
                text: (*text).to_owned(),
            },
        );
    }

    // Commands: sourced from the COMMANDS registry so the index never drifts.
    let commands: Vec<CommandInfo> = crate::help::command_names()
        .into_iter()
        .filter_map(|name| {
            crate::help::command_summary(name).map(|summary| CommandInfo { name, summary })
        })
        .collect();
    builder.add_commands(&commands);

    Ok(builder.finish())
}

/// `ipe doc <key>` — look up any entity by key and render it.
///
/// Resolves symbols, modules, diagnostic codes, constructs, and CLI commands
/// via the `ipe_docs` content index. Renders rich by default, `--plain` for
/// terse output, and `--json` for machine consumers.
fn run_doc_lookup(key: &str, format: OutputFormat) -> Result<(), CliError> {
    let index = build_index()?;

    let Some(entry) = index.resolve(key) else {
        return Err(CliError::UsageOwned(format!(
            "ipe doc: no documentation found for `{key}`\n\
             \n\
             Try `ipe doc list` to browse available entries."
        )));
    };

    let stdout = std::io::stdout();
    match format {
        OutputFormat::Plain => {
            print!("{}", render_doc_entry_plain(entry));
        }
        OutputFormat::Json => {
            print!("{}", render_doc_entry_json(entry));
        }
        OutputFormat::Human => {
            let p = crate::style::Palette::for_stream(&stdout);
            print!(
                "{}",
                crate::style::frame(&crate::style::gutter(&render_doc_entry_human(entry, p)))
            );
        }
    }
    Ok(())
}

/// Plain (terse) rendering of a documentation entry: signature + example.
fn render_doc_entry_plain(entry: &ipe_docs::Entry) -> String {
    let mut out = entry.text.clone();
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// JSON rendering of a documentation entry.
fn render_doc_entry_json(entry: &ipe_docs::Entry) -> String {
    let kind = match entry.kind {
        ipe_docs::EntryKind::Symbol => "symbol",
        ipe_docs::EntryKind::Module => "module",
        ipe_docs::EntryKind::Diagnostic => "diagnostic",
        ipe_docs::EntryKind::Construct => "construct",
        ipe_docs::EntryKind::Command => "command",
    };
    format!(
        "{{\"kind\":{},\"key\":{},\"text\":{}}}\n",
        doc_json_str(kind),
        doc_json_str(&entry.source_key),
        doc_json_str(&entry.text),
    )
}

/// Human (rich) rendering of a documentation entry with ANSI colour.
fn render_doc_entry_human(entry: &ipe_docs::Entry, p: &crate::style::Palette) -> String {
    let kind_label = match entry.kind {
        ipe_docs::EntryKind::Symbol => "symbol",
        ipe_docs::EntryKind::Module => "module",
        ipe_docs::EntryKind::Diagnostic => "diagnostic",
        ipe_docs::EntryKind::Construct => "construct",
        ipe_docs::EntryKind::Command => "command",
    };
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{}{}{}  {}[{}]{}",
        p.bold, entry.source_key, p.reset, p.dim, kind_label, p.reset
    );
    if !entry.text.is_empty() {
        out.push('\n');
        out.push_str(&entry.text);
        if !entry.text.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

/// Minimal JSON string escaping for the doc lookup renderer (no serde dependency).
fn doc_json_str(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Run `ipe doc` for the parsed [`DocMode`].
///
/// # Errors
/// [`CliError::Usage`] for a misuse the parser could not have caught, [`CliError`]
/// wrapping a [`crate::api_surface::DiffError`] when the package cannot be typed,
/// [`CliError::Io`] on a write failure, and [`CliError::Usage`] carrying the
/// coverage report when `check` finds an undocumented binding.
pub fn run_doc(rest: &[String]) -> Result<(), CliError> {
    match parse_doc(rest)? {
        DocMode::Generate {
            path,
            out,
            write_format,
        } => generate(&path, &out, write_format),
        DocMode::Serve { path, port } => serve(&path, port),
        DocMode::Check { path } => check(&path),
        DocMode::List { path, format } => {
            list_modules(&path, format);
            Ok(())
        }
        DocMode::Query { module, format } => query_module(&module, format),
        DocMode::CheckExamples => check_examples(),
        DocMode::Lookup { key, format } => run_doc_lookup(&key, format),
    }
}

/// The complete documentation of one package — the in-memory model every
/// rendering (JSON, Markdown) is a pure view over.
#[derive(Debug, PartialEq, Eq)]
pub struct DocsJson {
    /// The schema version this document was produced under.
    pub version: u32,
    /// One record per exposed module, in module-path order.
    pub modules: Vec<ModuleDoc>,
}

/// Which group a documented module belongs to.
///
/// The doc listing presents `Local` modules first, under their own labelled
/// section, so a reader sees their own API before the standard library.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleKind {
    /// A module from the package being documented.
    Local,
    /// A bundled standard-library module.
    Stdlib,
}

/// The section label for the user's own project modules, shared by every
/// rendering (console listing and generated HTML) so the phrasing lives once.
const LABEL_PROJECT: &str = "Project modules";

/// The section label for the bundled standard library, shared by every
/// rendering so the phrasing lives once.
const LABEL_STDLIB: &str = "Standard library";

/// The stable machine tag a `docs.json` consumer reads to group a module.
const fn module_kind_tag(kind: ModuleKind) -> &'static str {
    match kind {
        ModuleKind::Local => "local",
        ModuleKind::Stdlib => "stdlib",
    }
}

/// One exposed module's documentation: its doc-comment plus its exposed unions
/// and values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleDoc {
    /// The dotted module name (`Ipe.String`).
    pub name: String,
    /// Whether this is a project module or a bundled stdlib module.
    pub kind: ModuleKind,
    /// The module's own `-- |` header doc-comment, empty when it has none.
    pub comment: String,
    /// Exposed union types, in name order.
    pub unions: Vec<UnionDoc>,
    /// Exposed values, in name order.
    pub values: Vec<ValueDoc>,
}

/// One exposed value's documentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueDoc {
    /// The value name.
    pub name: String,
    /// Its checker-inferred, α-canonicalised type signature.
    pub signature: String,
    /// The same signature in its resolved [`TyDoc`] form, whose type-constructor
    /// nodes carry the canonicaliser's resolved module + name. Cross-references
    /// are computed from this — never from the flat string — so a link is a real
    /// resolved identity, not a text match.
    pub signature_ty: TyDoc,
    /// Its `-- |` doc-comment, empty when it has none.
    pub comment: String,
}

/// One exposed union type's documentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnionDoc {
    /// The union type name.
    pub name: String,
    /// Its type-parameter arity (`Maybe a` → 1).
    pub params: usize,
    /// Constructor name → argument signatures, in declaration order.
    pub ctors: Vec<CtorDoc>,
    /// The union's `-- |` doc-comment, empty when it has none.
    pub comment: String,
}

/// One constructor's rendered shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CtorDoc {
    /// The constructor name.
    pub name: String,
    /// Its argument signatures, in declaration order.
    pub args: Vec<String>,
    /// Its arguments' resolved type documents, in declaration order (parallel to
    /// [`Self::args`]), for cross-reference linking.
    pub arg_types: Vec<TyDoc>,
}

/// A binding whose doc-comment is missing, reported by [`check`].
#[derive(Debug, PartialEq, Eq)]
struct Undocumented {
    module: String,
    /// The binding name (a value, or a union type name).
    name: String,
}

/// Build the in-memory [`DocsJson`] for the package at `path`, including both
/// project modules and all stdlib modules (compiled-source + kernel-backed).
///
/// Project modules go through the existing `extract_tree` + `read_tree` pipeline.
/// Compiled-source stdlib modules go through the same type-checker path.
/// Kernel-qualifier stdlib modules use [`kernel_type_table`] for signatures.
///
/// Modules are listed in name order: stdlib first (alphabetically), then project.
fn build_docs(path: &Path) -> Result<DocsJson, CliError> {
    let api: PublicApi = extract_tree(path).map_err(CliError::from)?;
    let sources = read_tree(path).map_err(CliError::from)?;

    // Collect project modules.
    let mut project_modules: Vec<ModuleDoc> = Vec::with_capacity(api.modules.len());
    for (module_path, module_api) in &api.modules {
        let comments = sources
            .get(module_path)
            .map(|(_, src)| scan_doc_comments(src))
            .unwrap_or_default();
        project_modules.push(module_doc(
            module_path,
            module_api,
            &comments,
            ModuleKind::Local,
        ));
    }
    project_modules.sort_by(|a, b| a.name.cmp(&b.name));

    // Collect stdlib modules (both compiled-source and kernel-backed).
    let mut stdlib = build_stdlib_docs();
    stdlib.sort_by(|a, b| a.name.cmp(&b.name));

    // A project module shadows a stdlib module of the same name — project wins.
    let project_names: BTreeSet<&str> = project_modules.iter().map(|m| m.name.as_str()).collect();
    stdlib.retain(|m| !project_names.contains(m.name.as_str()));

    // Present the user's own modules first, then the standard library.
    let mut modules = project_modules;
    modules.extend(stdlib);

    Ok(DocsJson {
        version: DOCS_JSON_VERSION,
        modules,
    })
}

/// Build the in-memory [`DocsJson`] for the package at `path`, project modules only.
///
/// Used by `check` — stdlib modules are exempt from the coverage gate.
fn build_project_docs(path: &Path) -> Result<DocsJson, CliError> {
    let api: PublicApi = extract_tree(path).map_err(CliError::from)?;
    let sources = read_tree(path).map_err(CliError::from)?;

    let mut modules = Vec::with_capacity(api.modules.len());
    for (module_path, module_api) in &api.modules {
        let comments = sources
            .get(module_path)
            .map(|(_, src)| scan_doc_comments(src))
            .unwrap_or_default();
        modules.push(module_doc(
            module_path,
            module_api,
            &comments,
            ModuleKind::Local,
        ));
    }
    Ok(DocsJson {
        version: DOCS_JSON_VERSION,
        modules,
    })
}

/// Enumerate every stdlib module name (both compiled-source and kernel-qualifier),
/// sorted alphabetically.
fn stdlib_module_names() -> Vec<String> {
    let mut names: BTreeSet<String> = BTreeSet::new();

    // Compiled-source modules (Ipe.Css, Ipe.Test, …).
    for m in ipe_stdlib::COMPILED_STD_MODULES {
        names.insert(m.dotted.to_owned());
    }

    // Kernel-qualifier modules (Ipe.List, Ipe.String, …).
    for (segments, _qualifier) in ipe_canon::STDLIB_MODULE_QUALIFIERS {
        names.insert(segments.join("."));
    }

    names.into_iter().collect()
}

/// Build [`ModuleDoc`]s for every stdlib module.
///
/// Compiled-source modules are type-checked using the same pipeline as project
/// modules (injecting each individually as a minimal package). Kernel-qualifier
/// modules use [`kernel_type_table`] for type signatures; doc-comments are empty
/// (signatures are the contract for kernel-backed stdlib).
fn build_stdlib_docs() -> Vec<ModuleDoc> {
    let mut modules: BTreeMap<String, ModuleDoc> = BTreeMap::new();

    // ── Compiled-source stdlib modules ────────────────────────────────────────
    // Each compiled-source module is type-checked as embedded stdlib (so it may
    // declare the reserved `Ipe.*` namespace); signatures and unions come from the
    // type checker, doc-comments from the embedded source. Every module yields a
    // ModuleDoc — a type-check failure degrades to a doc-comment-only entry rather
    // than being dropped, so every `--list` name is queryable (see the invariant
    // enforced by `stdlib_module_names`, reconciled below).
    for csm in ipe_stdlib::COMPILED_STD_MODULES {
        let segments: Vec<String> = csm.dotted.split('.').map(str::to_owned).collect();
        modules.insert(
            csm.dotted.to_owned(),
            build_compiled_std_module_doc(&segments, csm.source),
        );
    }

    // ── Kernel-qualifier stdlib modules ───────────────────────────────────────
    // Signatures come from kernel_type_table; doc-comments are absent for these
    // compiler-internal modules (signatures are the contract). If the kernel
    // type table cannot be constructed, kernel modules are omitted (names still
    // appear in --list via stdlib_module_names).
    if let Ok(kernel_docs) = build_kernel_module_docs() {
        for (name, doc) in kernel_docs {
            modules.entry(name).or_insert(doc);
        }
    }

    // ── SSOT reconciliation ───────────────────────────────────────────────────
    // `stdlib_module_names` is the single source of truth for which stdlib modules
    // exist — the same set `--list` advertises. Guarantee a queryable ModuleDoc
    // for every one of those names, so a listed-but-unqueryable module (a `--list`
    // entry that 404s on `ipe doc <name>`) is unrepresentable.
    for name in stdlib_module_names() {
        modules
            .entry(name.clone())
            .or_insert_with(|| empty_stdlib_module_doc(&name));
    }

    modules.into_values().collect()
}

/// A signature-less [`ModuleDoc`] carrying only the module name — the last-resort
/// entry for a listed stdlib module the richer paths could not document, so a
/// `--list` name is never unqueryable.
fn empty_stdlib_module_doc(name: &str) -> ModuleDoc {
    ModuleDoc {
        name: name.to_owned(),
        kind: ModuleKind::Stdlib,
        comment: String::new(),
        unions: Vec::new(),
        values: Vec::new(),
    }
}

/// Build a [`ModuleDoc`] for one compiled-source stdlib module.
///
/// Type-checks the embedded source as embedded stdlib (via
/// [`crate::api_surface::extract_stdlib_module`], which grants the reserved-`Ipe.*`
/// declaration a project's injected closure already grants it) to recover full
/// signatures and unions, joining them to the doc-comments scanned from the
/// source. Always yields a [`ModuleDoc`]: if the module cannot be type-checked,
/// the result degrades to the scanned doc-comments over an empty API rather than
/// disappearing, so the module stays queryable and `--list`/query agree.
fn build_compiled_std_module_doc(segments: &[String], source: &str) -> ModuleDoc {
    use crate::api_surface::extract_stdlib_module;

    let comments = scan_doc_comments(source);
    let module_api = extract_stdlib_module(segments, source)
        .ok()
        .and_then(|api| api.modules.get(segments).cloned())
        .unwrap_or_default();
    let segments_vec: Vec<String> = segments.to_vec();
    module_doc(&segments_vec, &module_api, &comments, ModuleKind::Stdlib)
}

/// Build [`ModuleDoc`]s for every kernel-qualifier stdlib module.
///
/// The canonical qualifier table ([`ipe_canon::STDLIB_MODULE_QUALIFIERS`]) maps
/// import path → short qualifier. The kernel type table maps
/// [`ipe_kernels::StdlibKernel`] → Ipê type; each kernel's [`StdlibDecl`] gives
/// the qualifier and member name. These three tables are joined here to produce
/// per-module value lists with checker-inferred type signatures.
fn build_kernel_module_docs() -> Result<BTreeMap<String, ModuleDoc>, CliError> {
    // Build a reverse map from qualifier short name → full dotted module path.
    let qualifier_to_path: BTreeMap<String, String> = ipe_canon::STDLIB_MODULE_QUALIFIERS
        .iter()
        .map(|(segments, qualifier)| ((*qualifier).to_owned(), segments.join(".")))
        .collect();

    // Get the full type table for all kernel functions.
    let mut interner = Interner::new();
    let type_table = kernel_type_table(&mut interner)
        .map_err(|d| CliError::UsageOwned(format!("ipe doc: kernel type table error: {d:?}")))?;

    // Group by module path and build ValueDoc for each kernel.
    let mut by_module: BTreeMap<String, Vec<ValueDoc>> = BTreeMap::new();
    for (kernel, ty) in type_table {
        let decl = kernel.decl();
        let Some(module_path) = qualifier_to_path.get(decl.qualifier) else {
            continue;
        };
        let mut namer = VarNamer::new();
        let Ok(ty_doc) = ty_to_doc(&ty, &interner, &mut namer) else {
            continue; // skip kernels whose type fails to render
        };
        let signature = render_ty(&ty_doc);
        by_module
            .entry(module_path.clone())
            .or_default()
            .push(ValueDoc {
                name: decl.name.to_owned(),
                signature,
                signature_ty: ty_doc,
                comment: String::new(),
            });
    }

    // Assemble ModuleDoc for each module, values in name order.
    let mut docs: BTreeMap<String, ModuleDoc> = BTreeMap::new();
    for (module_path, mut values) in by_module {
        values.sort_by(|a, b| a.name.cmp(&b.name));
        docs.insert(
            module_path.clone(),
            ModuleDoc {
                name: module_path,
                kind: ModuleKind::Stdlib,
                comment: String::new(),
                unions: Vec::new(),
                values,
            },
        );
    }
    Ok(docs)
}

/// List all stdlib + project modules, rendered per `format`.
///
/// `--list` output:
/// * Human (default): guttered module names with a header.
/// * `--plain`: one bare module name per line, no framing.
/// * `--json`: `{"modules":["Ipe.List","Ipe.String",…]}`.
fn list_modules(path: &Path, format: OutputFormat) {
    use crate::style::{GUTTER, frame};

    // Collect stdlib names.
    let stdlib: Vec<String> = stdlib_module_names();

    // Collect project module names (best-effort; an unresolvable project is skipped
    // so `--list` always succeeds for stdlib).
    let project: Vec<String> = read_tree(path).map_or_else(
        |_| Vec::new(),
        |sources| {
            let mut names: Vec<String> = sources.keys().map(|p| p.join(".")).collect();
            names.sort();
            names
        },
    );

    // A project module shadows a stdlib module of the same name.
    let mut project_names: Vec<String> = project;
    project_names.sort();
    let project_set: BTreeSet<&str> = project_names.iter().map(String::as_str).collect();
    let stdlib_names: Vec<String> = stdlib
        .into_iter()
        .filter(|n| !project_set.contains(n.as_str()))
        .collect();

    // Present the user's own modules first, then the standard library.
    let ordered: Vec<&String> = project_names.iter().chain(stdlib_names.iter()).collect();

    match format {
        OutputFormat::Plain => {
            // One queryable module name per line (project first, then stdlib).
            // Kept flat — every line must resolve on `ipe doc <name>`; the
            // namespace hierarchy is presented in the human listing, the
            // Markdown index, and the HTML nav (which can show pure-prefix
            // headers a plain, machine-readable list must not).
            for name in &ordered {
                println!("{name}");
            }
        }
        OutputFormat::Json => {
            let names: Vec<&str> = ordered.iter().map(|n| n.as_str()).collect();
            println!(
                "{}",
                crate::cli_args::json::object(&[(
                    "modules",
                    crate::cli_args::json::string_array(&names),
                )])
            );
        }
        OutputFormat::Human => {
            let mut body = String::new();
            let _ = writeln!(body, "{GUTTER}{LABEL_PROJECT} ({}):\n", project_names.len());
            if project_names.is_empty() {
                let _ = writeln!(body, "{GUTTER}  (none)\n");
            } else {
                let proj_refs: Vec<&str> = project_names.iter().map(String::as_str).collect();
                let tree = build_namespace_tree(&proj_refs);
                let mut tree_out = String::new();
                render_plain_tree(&tree, 0, &mut tree_out);
                for line in tree_out.lines() {
                    let _ = writeln!(body, "{GUTTER}  {line}");
                }
                body.push('\n');
            }
            let _ = writeln!(body, "{GUTTER}{LABEL_STDLIB} ({}):\n", stdlib_names.len());
            let stdlib_refs: Vec<&str> = stdlib_names.iter().map(String::as_str).collect();
            let tree = build_namespace_tree(&stdlib_refs);
            let mut tree_out = String::new();
            render_plain_tree(&tree, 0, &mut tree_out);
            for line in tree_out.lines() {
                let _ = writeln!(body, "{GUTTER}  {line}");
            }
            print!("{}", frame(body.trim_end_matches('\n')));
        }
    }
}

/// Collect project [`ModuleDoc`]s from the current directory (best-effort).
///
/// Returns an empty vec when the project cannot be read or typed, so `query`
/// can still serve stdlib modules when no project is present.
fn query_project_modules() -> Vec<ModuleDoc> {
    read_tree(Path::new(DEFAULT_PATH)).map_or_else(
        |_| Vec::new(),
        |sources| {
            let Ok(api) = extract_tree(Path::new(DEFAULT_PATH)) else {
                return Vec::new();
            };
            api.modules
                .iter()
                .map(|(module_path, module_api)| {
                    let comments = sources
                        .get(module_path)
                        .map(|(_, src)| scan_doc_comments(src))
                        .unwrap_or_default();
                    module_doc(module_path, module_api, &comments, ModuleKind::Local)
                })
                .collect()
        },
    )
}

/// Render a single [`ModuleDoc`] in human-readable guttered form.
fn render_module_human(module: &ModuleDoc, index: &AnchorIndex) {
    use crate::style::{GUTTER, frame};

    let md = module.comment.as_str();
    let mut body = format!("{GUTTER}{}\n\n", module.name);
    if !md.is_empty() {
        let _ = writeln!(body, "{GUTTER}{md}\n");
    }
    for union in &module.unions {
        let _ = writeln!(
            body,
            "{GUTTER}  type {}{}",
            union.name,
            union_params(union.params)
        );
        if !union.comment.is_empty() {
            let _ = writeln!(body, "{GUTTER}    {}", union.comment);
        }
    }
    for value in &module.values {
        let sig = {
            let pieces = signature_pieces(&value.signature_ty, index);
            let mut s = String::new();
            for p in &pieces {
                match p {
                    SigPiece::Text(t) | SigPiece::Link { text: t, .. } => s.push_str(t),
                }
            }
            s
        };
        let _ = writeln!(body, "{GUTTER}  {} : {}", value.name, sig);
        if !value.comment.is_empty() {
            let _ = writeln!(body, "{GUTTER}    {}", value.comment);
        }
    }
    print!("{}", frame(body.trim_end_matches('\n')));
}

/// Query one module's API and render it per `format`.
///
/// Resolves `module_name` against stdlib + project (project overrides stdlib on
/// a name collision). Errors with a typed message on an unknown module.
fn query_module(module_name: &str, format: OutputFormat) -> Result<(), CliError> {
    // Build stdlib docs; also try to find project docs from `.` (best-effort).
    let stdlib = build_stdlib_docs();
    let project = query_project_modules();

    // Project wins over stdlib on name collision.
    let found: Option<&ModuleDoc> = project
        .iter()
        .find(|m| m.name == module_name)
        .or_else(|| stdlib.iter().find(|m| m.name == module_name));

    let Some(module) = found else {
        return Err(CliError::UsageOwned(format!(
            "ipe doc: unknown module `{module_name}` (IPE-N0004)\n\
             Run `ipe doc list` to see all available modules."
        )));
    };

    // Build a single-module DocsJson for the anchor index (cross-reference
    // resolution within this module's own types).
    let docs = DocsJson {
        version: DOCS_JSON_VERSION,
        modules: vec![module.clone()],
    };
    let index = AnchorIndex::build(&docs);

    match format {
        OutputFormat::Plain => {
            // Flush-left: one entry per line, `name : signature`.
            for union in &module.unions {
                println!("type {}{}", union.name, union_params(union.params));
            }
            for value in &module.values {
                println!("{} : {}", value.name, value.signature);
            }
        }
        OutputFormat::Json => {
            let mut out = String::new();
            render_module_json(&mut out, module, &index);
            println!("{out}");
        }
        OutputFormat::Human => {
            render_module_human(module, &index);
        }
    }
    Ok(())
}

/// Assemble one [`ModuleDoc`] from its checked API surface and its scanned
/// doc-comments.
fn module_doc(
    module_path: &ModulePath,
    api: &ModuleApi,
    comments: &DocComments,
    kind: ModuleKind,
) -> ModuleDoc {
    let unions = api
        .unions
        .iter()
        .map(|(name, union)| union_doc(name, union, comments))
        .collect();
    let values = api
        .values
        .iter()
        .map(|(name, signature)| ValueDoc {
            name: name.clone(),
            signature: signature.clone(),
            signature_ty: api.value_types.get(name).cloned().unwrap_or(TyDoc::Unit),
            comment: comments.get(name).unwrap_or_default(),
        })
        .collect();
    ModuleDoc {
        name: module_path.join("."),
        kind,
        comment: comments.module.clone(),
        unions,
        values,
    }
}

/// Assemble one [`UnionDoc`] from its checked shape and its scanned comment.
fn union_doc(name: &str, union: &UnionApi, comments: &DocComments) -> UnionDoc {
    let ctors = union
        .ctors
        .iter()
        .map(|(ctor_name, args)| CtorDoc {
            name: ctor_name.clone(),
            args: args.clone(),
            arg_types: union.ctor_types.get(ctor_name).cloned().unwrap_or_default(),
        })
        .collect();
    UnionDoc {
        name: name.to_owned(),
        params: union.params,
        ctors,
        comment: comments.get(name).unwrap_or_default(),
    }
}

/// The doc-comments scanned out of one module's source: the module header's own
/// comment and a per-binding-name map.
#[derive(Debug, Default, PartialEq, Eq)]
struct DocComments {
    /// The `-- |` block immediately above the `module` header, empty when absent.
    module: String,
    /// Binding name → its `-- |` block. A binding is a top-level value or a union
    /// type; a doc-comment attaches to the name on the first non-comment line
    /// below it.
    bindings: BTreeMap<String, String>,
}

impl DocComments {
    /// The doc-comment for `name`, or an empty string when it has none.
    fn get(&self, name: &str) -> Option<String> {
        self.bindings.get(name).cloned()
    }
}

/// Scan one module's source for `-- |` doc-comments and attach each to the
/// binding it precedes.
///
/// A doc-comment is a `-- |` line optionally followed by plain `--` continuation
/// lines; it attaches to the identifier that opens the next non-comment,
/// non-blank line. This is the driver-boundary recovery of a convention the lexer
/// erases — a value binding is recognised by `name :` or `name … =`, a union by
/// `type Name`, and the module header by `module`.
fn scan_doc_comments(src: &str) -> DocComments {
    let mut result = DocComments::default();
    let mut pending: Option<String> = None;

    for line in src.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = doc_comment_text(trimmed) {
            // Start of, or continuation of, a doc block.
            match &mut pending {
                Some(block) => {
                    block.push('\n');
                    block.push_str(rest);
                }
                None => pending = Some(rest.to_owned()),
            }
            continue;
        }
        if is_plain_comment(trimmed) {
            // A plain `--` line continues an open doc block (the stdlib writes
            // multi-line comments with only the first line marked `-- |`), and is
            // otherwise ignored.
            if let Some(block) = &mut pending {
                block.push('\n');
                block.push_str(plain_comment_text(trimmed));
            }
            continue;
        }
        if trimmed.is_empty() {
            // A blank line does not break an open block; a doc-comment separated
            // from its binding by blank lines still attaches to it.
            continue;
        }

        // A code line: the pending block, if any, attaches to the binding it
        // opens. A line with no recognised binding drops the block.
        if let Some(block) = pending.take() {
            let block = block.trim().to_owned();
            if let Some(target) = binding_target(trimmed) {
                match target {
                    BindingTarget::Module => result.module = block,
                    BindingTarget::Named(name) => {
                        result.bindings.entry(name).or_insert(block);
                    }
                }
            }
        }
    }
    result
}

/// The text of a `-- |` doc-comment line (the marker stripped), or `None` when
/// `line` is not a doc-comment opener.
fn doc_comment_text(line: &str) -> Option<&str> {
    line.strip_prefix("-- |")
        .map(str::trim_start)
        .or_else(|| line.strip_prefix("--|").map(str::trim_start))
}

/// Whether `line` is any `--` line comment (doc or plain).
fn is_plain_comment(line: &str) -> bool {
    line.starts_with("--")
}

/// The text of a plain `--` comment line, marker stripped.
///
/// Strips the `-- ` prefix (with a single space) so that any indentation
/// following the space is preserved in the returned text — allowing indented
/// code examples embedded in doc-comments to carry their leading spaces into
/// the accumulated block and, from there, into the HTML code-block renderer.
/// A bare `--` with no following space (a blank continuation line) strips only
/// the `--`, returning an empty string.
fn plain_comment_text(line: &str) -> &str {
    line.strip_prefix("-- ")
        .or_else(|| line.strip_prefix("--"))
        .unwrap_or(line)
}

/// What binding a code line opens, for attaching a preceding doc-comment.
enum BindingTarget {
    /// The `module` header.
    Module,
    /// A named top-level value or union type.
    Named(String),
}

/// Recognise the binding a code line opens: the `module` header, a `type Name`
/// union, or a top-level `name` value (`name :` signature or `name … =`
/// definition). Returns `None` for any other line (an `import`, an expression
/// continuation, a `)` closing an exposing list), so a doc block above such a
/// line is dropped rather than misattached.
fn binding_target(line: &str) -> Option<BindingTarget> {
    let mut words = line.split_whitespace();
    let first = words.next()?;
    if first == "module" {
        return Some(BindingTarget::Module);
    }
    if first == "type" {
        // `type Name …` or `type alias Name …`.
        let name = match words.next() {
            Some("alias") => words.next()?,
            Some(other) => other,
            None => return None,
        };
        return Some(BindingTarget::Named(name.to_owned()));
    }
    // A top-level value: an identifier that begins a signature (`name :`) or a
    // definition (`name arg… =`). A binding name starts with a lowercase letter;
    // this excludes keywords like `import`, `exposing`, and closing punctuation.
    let name = first.trim_end_matches(':');
    if is_value_name(name) {
        return Some(BindingTarget::Named(name.to_owned()));
    }
    None
}

/// Whether `word` is a lowercase-initial identifier — a top-level value name, as
/// distinct from a `Type`, a keyword, or punctuation.
fn is_value_name(word: &str) -> bool {
    let mut chars = word.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {
            chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '\'')
        }
        _ => false,
    }
}

/// The set of type constructors this package documents, so a reference in a
/// signature can be resolved to an in-package definition (and only then linked).
///
/// A `(module, type-name)` pair is present exactly when that module exposes that
/// union type. A `TyDoc::Con` whose resolved `(module, name)` is absent — a
/// built-in like `Int`, or a type from another package — is left as plain text,
/// never a dangling link.
struct AnchorIndex {
    types: BTreeSet<(String, String)>,
}

impl AnchorIndex {
    /// Build the index from every module's exposed union types.
    fn build(docs: &DocsJson) -> Self {
        let mut types = BTreeSet::new();
        for module in &docs.modules {
            for union in &module.unions {
                types.insert((module.name.clone(), union.name.clone()));
            }
        }
        Self { types }
    }

    /// The link target for a type constructor resolved to `(module, name)`, or
    /// `None` when it is not documented in this package.
    fn type_ref(&self, module: &str, name: &str) -> Option<TypeRef> {
        if self.types.contains(&(module.to_owned(), name.to_owned())) {
            Some(TypeRef {
                module: module.to_owned(),
                name: name.to_owned(),
            })
        } else {
            None
        }
    }
}

/// A resolved, in-package type reference — the address a cross-reference links
/// to. The anchor is `Module#Name`, identical across json, Markdown, and HTML.
#[derive(Clone, Debug, PartialEq, Eq)]
struct TypeRef {
    module: String,
    name: String,
}

impl TypeRef {
    /// The stable logical anchor `Module#Name`, shared by every rendering — the
    /// address a `docs.json` consumer records.
    fn anchor(&self) -> String {
        format!("{}#{}", self.module, self.name)
    }

    /// The physical link to this entry within a rendered site of extension `ext`
    /// (`html` or `md`): `Module-stem.ext#Name`. Deterministic from the logical
    /// anchor, so json / Markdown / HTML all point at the one entry.
    fn href(&self, ext: &str) -> String {
        format!("{}.{ext}#{}", module_stem(&self.module), self.name)
    }
}

/// The stem of a module's page filename (`Ipe.String` → `Ipe-String`), shared by
/// its `.md` and `.html` pages so an anchor address is the same in both.
fn module_stem(module: &str) -> String {
    module.replace('.', "-")
}

/// One piece of a rendered signature: either plain text, or an in-package type
/// reference to link. A [`TyDoc`] renders to a flat sequence of these, so a
/// renderer emits links (HTML `<a>`, Markdown `[…](…)`) without re-parsing the
/// string.
enum SigPiece {
    /// Literal text (punctuation, arrows, variables, built-in type names).
    Text(String),
    /// An in-package type name that links to its definition.
    Link { text: String, target: TypeRef },
}

/// Render a value's signature [`TyDoc`] into a flat piece sequence, linking every
/// type constructor that resolves to an in-package definition.
fn signature_pieces(ty: &TyDoc, index: &AnchorIndex) -> Vec<SigPiece> {
    let mut pieces = Vec::new();
    push_ty(ty, index, Prec::Top, &mut pieces);
    pieces
}

/// Precedence context for parenthesising, mirroring [`render_ty`]'s own rules so
/// the linked rendering reads identically to the flat string.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Prec {
    /// Top level — no parentheses added.
    Top,
    /// Left of an arrow — a nested arrow is parenthesised.
    FunLhs,
    /// A constructor argument — a nested application or arrow is parenthesised.
    Arg,
}

/// Append literal text to the piece sequence, coalescing with a trailing text
/// piece so links stay whole tokens.
fn push_text(pieces: &mut Vec<SigPiece>, text: &str) {
    if let Some(SigPiece::Text(last)) = pieces.last_mut() {
        last.push_str(text);
    } else {
        pieces.push(SigPiece::Text(text.to_owned()));
    }
}

/// Walk a [`TyDoc`] at the given precedence, emitting text and link pieces.
fn push_ty(ty: &TyDoc, index: &AnchorIndex, prec: Prec, pieces: &mut Vec<SigPiece>) {
    match ty {
        TyDoc::Unit => push_text(pieces, "()"),
        TyDoc::Var(v) => push_text(pieces, v),
        TyDoc::Con { module, name, args } => {
            let parens = prec == Prec::Arg && !args.is_empty();
            if parens {
                push_text(pieces, "(");
            }
            let head = if module.is_empty() {
                name.to_string()
            } else {
                format!("{module}.{name}")
            };
            match index.type_ref(module, name) {
                Some(target) => pieces.push(SigPiece::Link { text: head, target }),
                None => push_text(pieces, &head),
            }
            for arg in args {
                push_text(pieces, " ");
                push_ty(arg, index, Prec::Arg, pieces);
            }
            if parens {
                push_text(pieces, ")");
            }
        }
        TyDoc::Fun(a, b) => {
            let parens = prec != Prec::Top;
            if parens {
                push_text(pieces, "(");
            }
            push_ty(a, index, Prec::FunLhs, pieces);
            push_text(pieces, " -> ");
            push_ty(b, index, Prec::Top, pieces);
            if parens {
                push_text(pieces, ")");
            }
        }
        TyDoc::Tuple(elems) => {
            push_text(pieces, "(");
            for (i, e) in elems.iter().enumerate() {
                if i > 0 {
                    push_text(pieces, ", ");
                }
                push_ty(e, index, Prec::Top, pieces);
            }
            push_text(pieces, ")");
        }
        TyDoc::Record(fields) => {
            if fields.is_empty() {
                push_text(pieces, "{}");
                return;
            }
            push_text(pieces, "{ ");
            for (i, (fname, fty)) in fields.iter().enumerate() {
                if i > 0 {
                    push_text(pieces, ", ");
                }
                push_text(pieces, &format!("{fname} : "));
                push_ty(fty, index, Prec::Top, pieces);
            }
            push_text(pieces, " }");
        }
    }
}

/// The distinct in-package type references a signature resolves to, in
/// first-seen order — the structured cross-references `docs.json` records.
fn signature_references(ty: &TyDoc, index: &AnchorIndex) -> Vec<TypeRef> {
    let mut refs = Vec::new();
    for piece in signature_pieces(ty, index) {
        if let SigPiece::Link { target, .. } = piece
            && !refs.contains(&target)
        {
            refs.push(target);
        }
    }
    refs
}

/// The renderings for one `ipe doc` generate call, split by format subdirectory.
///
/// Returns three maps keyed by bare filename (no subfolder prefix):
/// - `json_files`: the `docs.json` source of truth
/// - `markdown_files`: per-module `.md` pages + the Markdown index
/// - `html_files`: the self-contained HTML site (`index.html`, per-module
///   pages, `style.css`)
///
/// The caller writes each map into its matching `<base>/json/`,
/// `<base>/markdown/`, or `<base>/html/` subfolder. Cross-links within each
/// format are relative to that subfolder (e.g. HTML `<a href="Ipe-List.html">`
/// resolves within `html/`; Markdown `[…](Ipe-List.md)` within `markdown/`).
/// The `docs.json` cross-reference anchors are format-neutral logical
/// addresses, identical across all three renderings.
fn render_site_split(
    docs: &DocsJson,
    write_format: WriteFormat,
) -> (
    BTreeMap<String, String>,
    BTreeMap<String, String>,
    BTreeMap<String, String>,
) {
    let index = AnchorIndex::build(docs);
    let mut json_files = BTreeMap::new();
    let mut markdown_files = BTreeMap::new();
    let mut html_files = BTreeMap::new();

    json_files.insert("docs.json".to_owned(), render_json(docs));

    if write_format.wants_markdown() {
        markdown_files.insert("index.md".to_owned(), render_markdown_index(docs));
        for module in &docs.modules {
            markdown_files.insert(
                format!("{}.md", module_stem(&module.name)),
                render_markdown(module, &index),
            );
        }
    }

    if write_format.wants_html() {
        html_files.insert("index.html".to_owned(), render_html_index(docs));
        html_files.insert("style.css".to_owned(), STYLE_CSS.to_owned());
        for module in &docs.modules {
            html_files.insert(
                format!("{}.html", module_stem(&module.name)),
                render_html_module(module, &index),
            );
        }
    }

    (json_files, markdown_files, html_files)
}

/// Build the flat file map for the HTTP serve loop (HTML only, no disk writes).
fn render_site_for_serve(docs: &DocsJson) -> BTreeMap<String, String> {
    let index = AnchorIndex::build(docs);
    let mut files = BTreeMap::new();
    files.insert("index.html".to_owned(), render_html_index(docs));
    files.insert("style.css".to_owned(), STYLE_CSS.to_owned());
    for module in &docs.modules {
        files.insert(
            format!("{}.html", module_stem(&module.name)),
            render_html_module(module, &index),
        );
    }
    files
}

/// Generate `docs.json` and the selected renderings for the package at `path`,
/// writing them under `out/{json,markdown,html}/` subfolders.
///
/// Attempts full project + stdlib documentation. When no project source is
/// reachable at `path` (no source files, no valid project), falls back to
/// stdlib-only so the command succeeds in any directory, including an empty
/// scratch dir.
///
/// # Errors
/// [`CliError::Io`] on a write failure. Project extraction errors are silently
/// absorbed by the stdlib-only fallback.
fn generate(path: &Path, out: &Path, write_format: WriteFormat) -> Result<(), CliError> {
    // Attempt full project + stdlib documentation. When no project is reachable
    // at `path`, fall back to stdlib-only so the command succeeds in any dir.
    let docs = build_docs(path).unwrap_or_else(|_| build_stdlib_only_docs());

    let (json_files, markdown_files, html_files) = render_site_split(&docs, write_format);

    write_format_dir(out, "json", &json_files)?;
    if write_format.wants_markdown() {
        write_format_dir(out, "markdown", &markdown_files)?;
    }
    if write_format.wants_html() {
        write_format_dir(out, "html", &html_files)?;
    }

    print!(
        "{}",
        crate::style::frame(&crate::style::gutter(&format!(
            "documented {} module{} to {}",
            docs.modules.len(),
            if docs.modules.len() == 1 { "" } else { "s" },
            out.display()
        )))
    );
    Ok(())
}

/// Write every file in `files` into `<base>/<subdir>/`, creating the directory
/// when absent.
///
/// # Errors
/// [`CliError::Io`] on any filesystem failure.
fn write_format_dir(
    base: &Path,
    subdir: &str,
    files: &BTreeMap<String, String>,
) -> Result<(), CliError> {
    let dir = base.join(subdir);
    std::fs::create_dir_all(&dir).map_err(|e| crate::io_err(&dir, e))?;
    for (name, contents) in files {
        let file_path = dir.join(name);
        std::fs::write(&file_path, contents).map_err(|e| crate::io_err(&file_path, e))?;
    }
    Ok(())
}

/// Build a stdlib-only [`DocsJson`] without accessing any project on disk.
///
/// Used when `ipe doc --write-format <fmt>` is invoked outside a project
/// directory. The project extraction pipeline (`extract_tree`/`read_tree`) is
/// not called; only stdlib modules are enumerated.
fn build_stdlib_only_docs() -> DocsJson {
    let mut stdlib = build_stdlib_docs();
    stdlib.sort_by(|a, b| a.name.cmp(&b.name));
    DocsJson {
        version: DOCS_JSON_VERSION,
        modules: stdlib,
    }
}

/// Verify every exposed binding in the package at `path` carries a doc-comment.
///
/// Writes nothing; exits non-zero (a [`CliError::DocCoverage`] carrying the
/// report) when any exposed value or union type lacks a `-- |` comment.
/// Stdlib modules are exempt — their signatures are the contract, doc-comments
/// are optional.
///
/// # Errors
/// As [`build_project_docs`], plus [`CliError::DocCoverage`] listing every
/// undocumented binding when coverage is incomplete.
fn check(path: &Path) -> Result<(), CliError> {
    let docs = build_project_docs(path)?;

    let mut gaps: Vec<Undocumented> = Vec::new();
    let mut exposed = 0usize;
    for module in &docs.modules {
        for value in &module.values {
            exposed += 1;
            if value.comment.is_empty() {
                gaps.push(Undocumented {
                    module: module.name.clone(),
                    name: value.name.clone(),
                });
            }
        }
        for union in &module.unions {
            exposed += 1;
            if union.comment.is_empty() {
                gaps.push(Undocumented {
                    module: module.name.clone(),
                    name: union.name.clone(),
                });
            }
        }
    }

    if gaps.is_empty() {
        print!(
            "{}",
            crate::style::frame(&crate::style::gutter(&format!(
                "all {exposed} exposed binding(s) are documented"
            )))
        );
        return Ok(());
    }

    let mut report = format!(
        "{} of {} exposed binding(s) lack a doc-comment:\n",
        gaps.len(),
        exposed
    );
    for gap in &gaps {
        let _ = writeln!(report, "  {}.{}", gap.module, gap.name);
    }
    let _ = write!(
        report,
        "add a `-- |` comment above each, or hide it from the module's exposing list"
    );
    Err(CliError::DocCoverage(report))
}

/// One extracted doc-string example awaiting verification.
struct Example {
    /// Human-readable label, e.g. `Ipe.Maybe::withDefault example 1`.
    label: String,
    /// The fenced ` ```ipe ` block body, already stripped of the fence lines.
    body: String,
    /// Expected result lines from `-->` annotations in the body, in order.
    /// Each element is the trimmed right-hand side of a `-->` arrow.
    expected_results: Vec<String>,
}

/// Extract all `{-| … -}` doc-string blocks from an Ipê source text and return
/// every ` ```ipe ` fenced example within them.
///
/// Each returned [`Example`] carries a label (for error reporting), the raw
/// block body, and the `-->` expected-result lines parsed out of the body
/// (for the result-assertion tier).
fn extract_doc_examples(module_name: &str, src: &str) -> Vec<Example> {
    const OPEN: &str = "{-|";
    const CLOSE: &str = "-}";
    const FENCE_OPEN: &str = "```ipe";
    const FENCE_CLOSE: &str = "```";

    let mut examples = Vec::new();
    let mut search_from = 0usize;
    let mut example_idx = 0usize;

    while let Some(open_rel) = src[search_from..].find(OPEN) {
        let block_start = search_from + open_rel + OPEN.len();
        let Some(close_rel) = src[block_start..].find(CLOSE) else {
            break;
        };
        let block_end = block_start + close_rel;
        let block_body = &src[block_start..block_end];

        // Find ` ```ipe ` fences within this doc-string body.
        let mut fence_search = 0usize;
        while let Some(fence_open_rel) = block_body[fence_search..].find(FENCE_OPEN) {
            let after_open = fence_search + fence_open_rel + FENCE_OPEN.len();
            // Skip the rest of the opening fence line.
            let content_start = block_body[after_open..]
                .find('\n')
                .map_or(block_body.len(), |nl| after_open + nl + 1);
            // Find the closing fence.
            let Some(close_fence_rel) = block_body[content_start..].find(FENCE_CLOSE) else {
                fence_search = after_open;
                continue;
            };
            let content_end = content_start + close_fence_rel;
            let raw_body = block_body[content_start..content_end].trim_end_matches('\n');

            example_idx += 1;
            let label = format!("{module_name} example {example_idx}");

            // Parse `-->` result annotations out of the body lines.
            let mut expected_results: Vec<String> = Vec::new();
            for line in raw_body.lines() {
                if let Some(arrow_pos) = line.find("-->") {
                    let result = line[arrow_pos + 3..].trim();
                    if !result.is_empty() {
                        expected_results.push(result.to_owned());
                    }
                }
            }

            examples.push(Example {
                label,
                body: raw_body.to_owned(),
                expected_results,
            });

            fence_search = content_end + FENCE_CLOSE.len();
        }

        search_from = block_end + CLOSE.len();
    }

    examples
}

/// Synthesize a minimal compilable module wrapping `body`.
///
/// If `body` already starts with `module ` it is returned as-is. Otherwise a
/// `module Main exposing (..)` header is prepended. The module that the
/// doc-string belongs to (`source_module`, e.g. `Ipe.Maybe`) is imported with
/// `exposing (..)` so unqualified names from it resolve. Additional imports for
/// commonly-qualified names are injected when the corresponding prefix appears
/// in `body`.
///
/// Lines that contain `-->` are treated as expression/result assertions. The
/// expression (the part before `-->`) is assigned to a fresh top-level binding
/// (`docCheckN = <expr>`) so the type-checker can infer its type without
/// needing a `main` entry point.
fn synthesize_module(body: &str, source_module: &str) -> String {
    if body.trim_start().starts_with("module ") {
        return body.to_owned();
    }

    let mut out = String::from("module Main exposing (..)\n");

    // Import the module whose doc-string this example comes from; its
    // unqualified names are in scope in the example.
    if !source_module.is_empty() {
        let short = source_module
            .split('.')
            .next_back()
            .unwrap_or(source_module);
        out.push('\n');
        let _ = writeln!(out, "import {source_module} as {short} exposing (..)");
    }

    // Auto-inject additional imports for qualified names used in the block.
    for (prefix, import) in &[
        ("Maybe.", "import Ipe.Maybe as Maybe exposing (Maybe(..))"),
        ("List.", "import Ipe.List as List"),
        ("String.", "import Ipe.String as String"),
        ("Dict.", "import Ipe.Dict as Dict"),
        ("Set.", "import Ipe.Set as Set"),
        (
            "Result.",
            "import Ipe.Result as Result exposing (Result(..))",
        ),
        ("Task.", "import Ipe.Task as Task"),
        ("Io.", "import Ipe.Io as Io"),
        ("Debug.", "import Ipe.Debug as Debug"),
        ("Char.", "import Ipe.Char as Char"),
    ] {
        // Only inject if the prefix appears AND the module is not already
        // imported as the source module (avoid a duplicate import).
        let module_for_prefix = import.split_whitespace().nth(1).unwrap_or("");
        if body.contains(prefix) && module_for_prefix != source_module {
            out.push('\n');
            out.push_str(import);
        }
    }

    out.push('\n');

    // Emit body lines. `-->` expression-assertion lines become `docCheckN = <expr>`
    // top-level bindings so the type-checker can reach them.
    let mut check_idx = 0usize;
    for line in body.lines() {
        if let Some(arrow_pos) = line.find("-->") {
            let expr = line[..arrow_pos].trim();
            if !expr.is_empty() {
                check_idx += 1;
                out.push('\n');
                let _ = writeln!(out, "docCheck{check_idx} = {expr}");
            }
        } else {
            out.push('\n');
            out.push_str(line);
        }
    }
    out.push('\n');
    out
}

/// Run the compiled example at `snippet_path` and assert its output matches the
/// `-->` annotated results (one per line, in order).
///
/// Spawns the current binary as `ipe run <snippet_path>` and compares stdout
/// against the expected output. Returns `Err(description)` on a mismatch or
/// subprocess failure; `Ok(())` when the output matches.
fn run_example_and_check(
    snippet_path: &Path,
    label: &str,
    expected: &[String],
) -> Result<(), String> {
    use std::process::Command;

    // Locate this binary (we re-invoke ourselves as `ipe run`).
    let ipe_bin = std::env::current_exe()
        .map_err(|e| format!("{label}: could not locate ipe binary: {e}"))?;

    let out = Command::new(&ipe_bin)
        .arg("run")
        .arg(snippet_path)
        .output()
        .map_err(|e| format!("{label}: ipe run failed to spawn: {e}"))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!("{label}: ipe run exited non-zero\n       {stderr}"));
    }

    let actual = String::from_utf8_lossy(&out.stdout);
    let actual_lines: Vec<&str> = actual.lines().filter(|l| !l.trim().is_empty()).collect();

    if actual_lines.len() != expected.len() {
        return Err(format!(
            "{label}: expected {} output line(s) but got {}\n       expected: {:?}\n       actual:   {:?}",
            expected.len(),
            actual_lines.len(),
            expected,
            actual_lines,
        ));
    }

    for (i, (exp, got)) in expected.iter().zip(actual_lines.iter()).enumerate() {
        if exp.trim() != got.trim() {
            return Err(format!(
                "{label}: result line {} mismatch\n       expected: {exp}\n       actual:   {got}",
                i + 1
            ));
        }
    }

    Ok(())
}

/// Doc-test gate: extract every fenced ` ```ipe ` example from every
/// `{-| … -}` doc-string across the standard library and type-check each one.
///
/// When `IPE_E2E=1` is set in the environment, examples with `-->` result
/// annotations are also compiled, run, and their printed output asserted.
///
/// # Errors
/// [`CliError::DocExamplesFailed`] when any block fails its expectation.
fn check_examples() -> Result<(), CliError> {
    // Collect all stdlib sources: the kernel-typed modules and the compiled-source ones.
    let all_sources: Vec<(&str, &str)> = crate::stdlib::MODULES
        .iter()
        .map(|m| (m.name, m.source))
        .chain(
            crate::stdlib::COMPILED_STD_MODULES
                .iter()
                .map(|m| (m.dotted, m.source)),
        )
        .collect();

    let mut total = 0usize;
    let mut passed = 0usize;
    let mut failed: Vec<String> = Vec::new();

    // Write examples to a temp dir; RAII cleanup on exit.
    let tmp_dir =
        crate::scratch::ScratchDir::new("ipe-doc-examples").map_err(|e| CliError::Io {
            path: std::path::PathBuf::from("ipe-doc-examples"),
            source: e,
        })?;

    for (module_name, src) in &all_sources {
        let examples = extract_doc_examples(module_name, src);
        for ex in &examples {
            total += 1;
            let module_src = synthesize_module(&ex.body, module_name);

            // Write to a temp file.
            let snippet_path = tmp_dir.child("Main.ipe");
            std::fs::write(&snippet_path, &module_src)
                .map_err(|e| crate::io_err(&snippet_path, e))?;

            // Tier 1: type-check the example (always).
            match crate::typecheck_entry_via_graph(&snippet_path) {
                Ok(()) => {
                    eprintln!("  ok   {}", ex.label);
                    passed += 1;
                }
                Err(err) => {
                    eprintln!("  FAIL {}: does not type-check", ex.label);
                    eprintln!("       {err}");
                    failed.push(format!("{}: does not type-check", ex.label));
                    // Skip result-checking for examples that don't even type-check.
                    continue;
                }
            }

            // Tier 2: if the example has `-->` annotations AND IPE_E2E=1 is set,
            // run the example and assert its printed output.
            if !ex.expected_results.is_empty()
                && std::env::var_os("IPE_E2E").is_some_and(|v| v == "1")
            {
                match run_example_and_check(&snippet_path, &ex.label, &ex.expected_results) {
                    Ok(()) => {}
                    Err(msg) => {
                        eprintln!("  FAIL {msg}");
                        failed.push(msg);
                    }
                }
            }
        }
    }

    eprintln!();
    eprintln!("=== doc-example gate: {passed}/{total} passed ===");

    if failed.is_empty() {
        print!(
            "{}",
            crate::style::frame(&crate::style::gutter(&format!(
                "all {total} doc-string example(s) type-check"
            )))
        );
        Ok(())
    } else {
        let mut report = format!(
            "{} of {} doc-string example(s) failed:\n",
            failed.len(),
            total
        );
        for msg in &failed {
            let _ = writeln!(report, "  FAIL: {msg}");
        }
        let _ = write!(
            report,
            "fix each failing example or mark it with ` ```ipe ipe:skip ` to exempt it"
        );
        Err(CliError::DocExamplesFailed(report))
    }
}

/// Render [`DocsJson`] as JSON.
///
/// A small hand-written serializer (the driver has no `serde` dependency) that
/// emits the versioned, stable schema: `{ "version", "modules": [ … ] }`. The
/// key order is fixed and the whole document is a deterministic function of the
/// model, so a consumer diffing two runs sees only real API changes.
fn render_json(docs: &DocsJson) -> String {
    let index = AnchorIndex::build(docs);
    let mut out = String::new();
    out.push_str("{\n");
    let _ = writeln!(out, "  \"version\": {},", docs.version);
    out.push_str("  \"modules\": [\n");
    for (i, module) in docs.modules.iter().enumerate() {
        render_module_json(&mut out, module, &index);
        out.push_str(if i + 1 < docs.modules.len() {
            ",\n"
        } else {
            "\n"
        });
    }
    out.push_str("  ]\n}\n");
    out
}

/// Render one module object into the JSON buffer at a fixed two-space indent.
fn render_module_json(out: &mut String, module: &ModuleDoc, index: &AnchorIndex) {
    out.push_str("    {\n");
    let _ = writeln!(out, "      \"name\": {},", json_string(&module.name));
    let _ = writeln!(
        out,
        "      \"kind\": {},",
        json_string(module_kind_tag(module.kind))
    );
    let _ = writeln!(out, "      \"comment\": {},", json_string(&module.comment));

    out.push_str("      \"unions\": [");
    for (i, union) in module.unions.iter().enumerate() {
        out.push_str(if i == 0 { "\n" } else { ",\n" });
        out.push_str("        {\n");
        let _ = writeln!(out, "          \"name\": {},", json_string(&union.name));
        let _ = writeln!(out, "          \"params\": {},", union.params);
        let _ = writeln!(
            out,
            "          \"comment\": {},",
            json_string(&union.comment)
        );
        out.push_str("          \"constructors\": [");
        for (j, ctor) in union.ctors.iter().enumerate() {
            out.push_str(if j == 0 { "\n" } else { ",\n" });
            out.push_str("            {\n");
            let _ = writeln!(out, "              \"name\": {},", json_string(&ctor.name));
            let args = ctor
                .args
                .iter()
                .map(|a| json_string(a))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(out, "              \"args\": [{args}]");
            out.push_str("            }");
        }
        out.push_str(if union.ctors.is_empty() {
            "]\n"
        } else {
            "\n          ]\n"
        });
        out.push_str("        }");
    }
    out.push_str(if module.unions.is_empty() {
        "],\n"
    } else {
        "\n      ],\n"
    });

    out.push_str("      \"values\": [");
    for (i, value) in module.values.iter().enumerate() {
        out.push_str(if i == 0 { "\n" } else { ",\n" });
        out.push_str("        {\n");
        let _ = writeln!(out, "          \"name\": {},", json_string(&value.name));
        let _ = writeln!(
            out,
            "          \"signature\": {},",
            json_string(&value.signature)
        );
        let _ = writeln!(
            out,
            "          \"comment\": {},",
            json_string(&value.comment)
        );
        // The in-package cross-references this signature resolves to — the
        // structured form a `docs.json` consumer resolves, with the anchor
        // shared by the Markdown and HTML renderings.
        let refs = signature_references(&value.signature_ty, index);
        out.push_str("          \"references\": [");
        for (k, r) in refs.iter().enumerate() {
            out.push_str(if k == 0 { "\n" } else { ",\n" });
            out.push_str("            {\n");
            let _ = writeln!(out, "              \"module\": {},", json_string(&r.module));
            let _ = writeln!(out, "              \"name\": {},", json_string(&r.name));
            let _ = writeln!(
                out,
                "              \"anchor\": {}",
                json_string(&r.anchor())
            );
            out.push_str("            }");
        }
        out.push_str(if refs.is_empty() {
            "]\n"
        } else {
            "\n          ]\n"
        });
        out.push_str("        }");
    }
    out.push_str(if module.values.is_empty() {
        "]\n"
    } else {
        "\n      ]\n"
    });
    out.push_str("    }");
}

/// Encode a string as a JSON string literal via the CLI's one JSON-string SSOT
/// ([`crate::cli_args::json::string`]) so the multi-line `docs.json` and the
/// compact `--json` verdicts escape identically.
fn json_string(s: &str) -> String {
    crate::cli_args::json::string(s)
}

/// The type-parameter suffix a union type displays (`Maybe` at arity 1 → ` a`).
fn union_params(params: usize) -> String {
    if params == 0 {
        return String::new();
    }
    let names: Vec<String> = (0..params)
        .map(|i| ipe_types::letters(u32::try_from(i).unwrap_or(u32::MAX)).to_string())
        .collect();
    format!(" {}", names.join(" "))
}

/// Render a signature's pieces as Markdown: an in-package type is a
/// `[Type](page#anchor)` link, everything else is inline `` `code` ``.
fn markdown_signature(pieces: &[SigPiece]) -> String {
    let mut out = String::new();
    for piece in pieces {
        match piece {
            SigPiece::Text(t) => {
                let _ = write!(out, "`{t}`");
            }
            SigPiece::Link { text, target } => {
                let _ = write!(out, "[`{text}`]({})", target.href("md"));
            }
        }
    }
    out
}

/// Render one module's documentation as Markdown — a pure view over its
/// [`ModuleDoc`], with in-package type references linked via `index`.
fn render_markdown(module: &ModuleDoc, index: &AnchorIndex) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# {}\n", module.name);
    if !module.comment.is_empty() {
        let _ = writeln!(out, "{}\n", module.comment);
    }

    if !module.unions.is_empty() {
        out.push_str("## Types\n\n");
        for union in &module.unions {
            let _ = writeln!(out, "### `{}{}`\n", union.name, union_params(union.params));
            if !union.comment.is_empty() {
                let _ = writeln!(out, "{}\n", union.comment);
            }
            for ctor in &union.ctors {
                if ctor.args.is_empty() {
                    let _ = writeln!(out, "- `{}`", ctor.name);
                } else {
                    let mut line = format!("`{} `", ctor.name);
                    for arg in &ctor.arg_types {
                        line.push_str(&markdown_signature(&signature_pieces(arg, index)));
                        line.push(' ');
                    }
                    let _ = writeln!(out, "- {}", line.trim_end());
                }
            }
            out.push('\n');
        }
    }

    if !module.values.is_empty() {
        out.push_str("## Values\n\n");
        for value in &module.values {
            let sig = markdown_signature(&signature_pieces(&value.signature_ty, index));
            let _ = writeln!(out, "### `{}`\n", value.name);
            let _ = writeln!(out, "`{} :` {}\n", value.name, sig);
            if !value.comment.is_empty() {
                let _ = writeln!(out, "{}\n", value.comment);
            }
        }
    }
    out
}

// ===========================================================================
// Namespace hierarchy — shared by Markdown index, HTML nav, and plain list.
// ===========================================================================

/// A node in the namespace tree built from dotted module names.
///
/// Each node represents one segment (e.g. `Db` within `Ipe.Db`). A node is a
/// *module* when a [`ModuleDoc`] exists at its exact dotted path; it is a
/// *namespace header* when it has children but no module of its own at that
/// exact path. Children are kept in sorted order so every level renders
/// alphabetically.
struct NamespaceNode {
    /// The dotted path to this node (e.g. `Ipe.Db`).
    full_name: String,
    /// Whether a [`ModuleDoc`] exists at this exact path.
    is_module: bool,
    /// Child nodes, sorted alphabetically by their trailing segment.
    children: Vec<Self>,
}

/// Build a namespace tree from a flat, sorted slice of module names.
///
/// Returns the root children (the top-level segments). A pure prefix node
/// that has children but no module of its own at its exact path is marked
/// `is_module = false` and renders as a non-link header.
fn build_namespace_tree(names: &[&str]) -> Vec<NamespaceNode> {
    let mut roots: Vec<NamespaceNode> = Vec::new();
    for name in names {
        insert_into_tree(&mut roots, &name.split('.').collect::<Vec<_>>(), 0);
    }
    roots
}

/// Recursively insert a dotted module name into the tree at `depth`.
fn insert_into_tree(nodes: &mut Vec<NamespaceNode>, segments: &[&str], depth: usize) {
    if depth >= segments.len() {
        return;
    }
    let Some(prefix_segs) = segments.get(..=depth) else {
        return;
    };
    let prefix = prefix_segs.join(".");

    if let Some(node) = nodes.iter_mut().find(|n| n.full_name == prefix) {
        if depth + 1 == segments.len() {
            node.is_module = true;
        } else {
            insert_into_tree(&mut node.children, segments, depth + 1);
        }
    } else {
        let is_module = depth + 1 == segments.len();
        let mut node = NamespaceNode {
            full_name: prefix,
            is_module,
            children: Vec::new(),
        };
        if !is_module {
            insert_into_tree(&mut node.children, segments, depth + 1);
        }
        nodes.push(node);
        nodes.sort_by(|a, b| a.full_name.cmp(&b.full_name));
    }
}

/// Render a namespace tree as indented Markdown lines.
///
/// Modules are links to their `.md` page; pure prefix headers are bold text
/// with no link. Each namespace depth is indented by exactly 2 spaces.
fn render_markdown_tree(nodes: &[NamespaceNode], depth: usize, out: &mut String) {
    let indent = "  ".repeat(depth);
    for node in nodes {
        if node.is_module {
            let stem = module_stem(&node.full_name);
            let _ = writeln!(out, "{indent}- [{name}]({stem}.md)", name = node.full_name);
        } else {
            let _ = writeln!(out, "{indent}- **{}**", node.full_name);
        }
        render_markdown_tree(&node.children, depth + 1, out);
    }
}

/// Render the Markdown index page for the whole doc set.
///
/// Presents project modules under their section first, then the stdlib, each
/// section as its own namespace tree. When only stdlib modules are present the
/// project section is omitted.
fn render_markdown_index(docs: &DocsJson) -> String {
    let mut out = String::from("# API documentation\n\n");

    let project_names: Vec<&str> = docs
        .modules
        .iter()
        .filter(|m| m.kind == ModuleKind::Local)
        .map(|m| m.name.as_str())
        .collect();

    let stdlib_names: Vec<&str> = docs
        .modules
        .iter()
        .filter(|m| m.kind == ModuleKind::Stdlib)
        .map(|m| m.name.as_str())
        .collect();

    if !project_names.is_empty() {
        let _ = writeln!(out, "## {LABEL_PROJECT}\n");
        let tree = build_namespace_tree(&project_names);
        render_markdown_tree(&tree, 0, &mut out);
        out.push('\n');
    }

    if !stdlib_names.is_empty() {
        let _ = writeln!(out, "## {LABEL_STDLIB}\n");
        let tree = build_namespace_tree(&stdlib_names);
        render_markdown_tree(&tree, 0, &mut out);
    }

    out
}

/// Render a namespace tree into a string with 2-space indent per depth.
///
/// Module nodes emit their full dotted name; pure prefix header nodes emit
/// their full dotted name followed by `/` to indicate they are a namespace
/// without a module at that exact path.
fn render_plain_tree(nodes: &[NamespaceNode], depth: usize, out: &mut String) {
    let indent = "  ".repeat(depth);
    for node in nodes {
        if node.is_module {
            let _ = writeln!(out, "{indent}{}", node.full_name);
        } else {
            let _ = writeln!(out, "{indent}{}/", node.full_name);
        }
        render_plain_tree(&node.children, depth + 1, out);
    }
}

/// Render a namespace tree as nested HTML `<ul>/<li>` markup.
///
/// Modules emit an `<a href="…">` link; pure prefix headers emit a plain
/// `<span>`. Children are wrapped in a nested `<ul>`.
fn render_html_tree(nodes: &[NamespaceNode], out: &mut String) {
    out.push_str("<ul class=\"modules\">\n");
    for node in nodes {
        out.push_str("<li class=\"module\"");
        let _ = write!(
            out,
            " data-name=\"{}\"",
            html_escape(&node.full_name.to_lowercase())
        );
        out.push('>');
        if node.is_module {
            let stem = module_stem(&node.full_name);
            let _ = write!(
                out,
                "<a href=\"{stem}.html\">{}</a>",
                html_escape(&node.full_name)
            );
        } else {
            let _ = write!(
                out,
                "<span class=\"ns-header\">{}</span>",
                html_escape(&node.full_name)
            );
        }
        if !node.children.is_empty() {
            out.push('\n');
            render_html_tree(&node.children, out);
        }
        out.push_str("</li>\n");
    }
    out.push_str("</ul>\n");
}

// ===========================================================================
// HTML site — a self-contained static rendering over the same `DocsJson` model.
// ===========================================================================

/// The bundled stylesheet the HTML site links, written once beside the pages so
/// the site is self-contained and opens over `file://` with no network fetch.
const STYLE_CSS: &str = "\
:root {
  color-scheme: dark;
  --bg: #1e232b;
  --surface: #262c36;
  --fg: #d7dbe0;
  --muted: #9aa2ad;
  --accent: #f2e29a;
  --accent-strong: #f7ecb0;
  --border: #3a4150;
}
body {
  font: 16px/1.6 system-ui, -apple-system, Segoe UI, Roboto, sans-serif;
  margin: 0; padding: 2rem; max-width: 60rem; margin-inline: auto;
  background: var(--bg); color: var(--fg);
}
h1 { font-size: 1.6rem; color: var(--accent); }
h2 { font-size: 1.2rem; margin-top: 2rem; color: var(--accent); }
a { color: var(--accent); text-decoration: none; }
a:hover, a:focus { color: var(--accent-strong); text-decoration: underline; }
nav.crumb { margin-bottom: 1.5rem; font-size: 0.9rem; }
input.filter {
  width: 100%; box-sizing: border-box; margin: 0.5rem 0 1.5rem;
  padding: 0.5rem 0.7rem; font-size: 1rem;
  background: var(--surface); color: var(--fg);
  border: 1px solid var(--border); border-radius: 6px;
}
input.filter:focus { outline: 2px solid var(--accent); outline-offset: 1px; }
section.group { margin-top: 1.5rem; }
ul.modules {
  list-style: none; padding: 0; margin: 0.5rem 0 0;
  columns: 3 14rem; column-gap: 2rem;
}
ul.modules li { margin: 0.3rem 0; break-inside: avoid; }
section.entry { border-top: 1px solid var(--border); padding-top: 0.5rem; margin-top: 1.5rem; }
section.entry h3 { margin: 0 0 0.3rem; font-size: 1.05rem; color: var(--accent); }
code, pre { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; }
pre.sig {
  background: var(--surface); padding: 0.4rem 0.6rem;
  border-radius: 4px; overflow-x: auto; border: 1px solid var(--border);
}
p.comment { margin: 0.4rem 0 0; color: var(--muted); }
pre.doc-code {
  background: var(--surface); border: 1px solid var(--border);
  border-radius: 4px; padding: 0.6rem 0.8rem; overflow-x: auto;
  margin: 0.6rem 0 0; color: var(--fg);
}
pre.doc-code code { background: none; padding: 0; }
span.ns-header { color: var(--muted); font-style: italic; }
ul.modules ul { padding-left: 1.2rem; }
@media (max-width: 40rem) { ul.modules { columns: 1; } }
";

/// The inline filter script bundled into the index page. It hides any module
/// list item whose lowercased `data-name` does not contain the (lowercased)
/// query, and collapses a section that ends up with no visible module. No
/// external dependency — a single `input` listener over the static list.
const FILTER_SCRIPT: &str = "\
<script>
(function () {
  var box = document.getElementById('filter');
  if (!box) return;
  var items = Array.prototype.slice.call(document.querySelectorAll('li.module'));
  var groups = Array.prototype.slice.call(document.querySelectorAll('section.group'));
  box.addEventListener('input', function () {
    var q = box.value.trim().toLowerCase();
    items.forEach(function (li) {
      var name = li.getAttribute('data-name') || '';
      li.hidden = q !== '' && name.indexOf(q) === -1;
    });
    groups.forEach(function (sec) {
      var any = sec.querySelectorAll('li.module:not([hidden])').length > 0;
      sec.hidden = !any;
    });
  });
})();
</script>
";

/// Escape the five characters an HTML text/attribute context requires, so a type
/// name or a doc-comment can never inject markup.
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// Render a doc-comment body as structured HTML.
///
/// The output contains block-level tags and must be inserted verbatim into the
/// page body — do not wrap it in an additional `<p>`.
///
/// Code blocks are emitted as `<pre class="doc-code"><code>…</code></pre>` with
/// all content html-escaped and internal whitespace (newlines, leading spaces)
/// preserved exactly as written.  Two detection rules apply:
///
/// * A fenced block is a run of lines bracketed by opening/closing fence lines
///   that consist solely of three backticks (optionally followed by a language
///   tag on the opening fence).  The fence lines themselves are stripped; inner
///   lines are emitted verbatim.
///
/// * An indented block is a run of consecutive non-blank lines that each start
///   with at least four spaces.  The leading four-space marker is stripped from
///   each line; the remaining content (including any additional indentation) is
///   preserved.
///
/// Prose paragraphs are blank-line–separated runs of non-code lines emitted as
/// `<p class="comment">…</p>`.  Within prose, spans delimited by single
/// backticks become `<code>…</code>`; all other text is html-escaped.
fn render_comment_html(comment: &str) -> String {
    /// Render inline backtick spans in prose text as `<code>`.
    fn render_inline(text: &str) -> String {
        let mut out = String::new();
        let mut rest = text;
        while let Some(open) = rest.find('`') {
            out.push_str(&html_escape(&rest[..open]));
            rest = &rest[open + 1..];
            if let Some(close) = rest.find('`') {
                out.push_str("<code>");
                out.push_str(&html_escape(&rest[..close]));
                out.push_str("</code>");
                rest = &rest[close + 1..];
            } else {
                // Unmatched backtick: emit literally.
                out.push('`');
            }
        }
        out.push_str(&html_escape(rest));
        out
    }

    /// Flush a prose paragraph buffer to `out`.
    fn flush_prose(out: &mut String, prose: &mut Vec<String>) {
        if prose.is_empty() {
            return;
        }
        let text = prose.join(" ");
        let _ = writeln!(out, "<p class=\"comment\">{}</p>", render_inline(&text));
        prose.clear();
    }

    /// Flush a code block buffer to `out`, html-escaping the content.
    fn flush_code(out: &mut String, code: &mut Vec<String>) {
        if code.is_empty() {
            return;
        }
        out.push_str("<pre class=\"doc-code\"><code>");
        for (i, line) in code.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(&html_escape(line));
        }
        out.push_str("</code></pre>\n");
        code.clear();
    }

    let mut out = String::new();
    let mut prose: Vec<String> = Vec::new();
    let mut code_buf: Vec<String> = Vec::new();
    let mut fenced = false;

    for line in comment.lines() {
        let trimmed = line.trim_start_matches(' ');
        let fence_delim = trimmed.starts_with("```");

        if fenced {
            if fence_delim {
                // Closing delimiter: flush the accumulated code block.
                fenced = false;
                flush_code(&mut out, &mut code_buf);
            } else {
                code_buf.push(line.to_owned());
            }
        } else if fence_delim {
            // Opening delimiter: flush prose, then enter fenced mode.
            flush_prose(&mut out, &mut prose);
            fenced = true;
            // The opening delimiter line (plus any language tag) is dropped.
        } else if let Some(rest) = line.strip_prefix("    ") {
            // Indented code block: flush prose, strip the four-space marker.
            flush_prose(&mut out, &mut prose);
            code_buf.push(rest.to_owned());
        } else if line.trim().is_empty() {
            // Blank line: close any open paragraph or indented code block.
            flush_prose(&mut out, &mut prose);
            flush_code(&mut out, &mut code_buf);
        } else {
            // Prose line: close any open indented code block first.
            flush_code(&mut out, &mut code_buf);
            prose.push(line.trim().to_owned());
        }
    }

    // Flush any trailing content (an unclosed fence is treated as a code block).
    flush_prose(&mut out, &mut prose);
    flush_code(&mut out, &mut code_buf);

    out
}

/// Render a signature's pieces as HTML: an in-package type is an `<a href>` to
/// its anchor, everything else is escaped text.
fn html_signature(pieces: &[SigPiece]) -> String {
    let mut out = String::new();
    for piece in pieces {
        match piece {
            SigPiece::Text(t) => out.push_str(&html_escape(t)),
            SigPiece::Link { text, target } => {
                let _ = write!(
                    out,
                    "<a href=\"{}\">{}</a>",
                    html_escape(&target.href("html")),
                    html_escape(text)
                );
            }
        }
    }
    out
}

/// Wrap a page body in the shared HTML shell (doctype, `<head>` linking the
/// bundled stylesheet, `<body>`).
fn html_page(title: &str, body: &str) -> String {
    format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>{}</title>\n<link rel=\"stylesheet\" href=\"style.css\">\n</head>\n\
         <body>\n{body}</body>\n</html>\n",
        html_escape(title)
    )
}

/// Render the index page — the package's module list, project modules first
/// under their labelled section, then the standard library. Each section is a
/// hierarchical namespace tree with a client-side filter box.
fn render_html_index(docs: &DocsJson) -> String {
    let mut body = String::from("<h1>API documentation</h1>\n");
    body.push_str(
        "<input type=\"search\" id=\"filter\" class=\"filter\" \
         placeholder=\"Filter modules\u{2026}\" aria-label=\"Filter modules\" \
         autocomplete=\"off\">\n",
    );

    render_html_module_section(&mut body, docs, ModuleKind::Local, LABEL_PROJECT);
    render_html_module_section(&mut body, docs, ModuleKind::Stdlib, LABEL_STDLIB);

    body.push_str(FILTER_SCRIPT);
    html_page("API documentation", &body)
}

/// Append one labelled module section as a hierarchical namespace tree to the
/// index body. A section with no modules is omitted.
fn render_html_module_section(body: &mut String, docs: &DocsJson, kind: ModuleKind, label: &str) {
    let names: Vec<&str> = docs
        .modules
        .iter()
        .filter(|m| m.kind == kind)
        .map(|m| m.name.as_str())
        .collect();
    if names.is_empty() {
        return;
    }
    let _ = writeln!(
        body,
        "<section class=\"group\"><h2>{}</h2>",
        html_escape(label)
    );
    let tree = build_namespace_tree(&names);
    render_html_tree(&tree, body);
    body.push_str("</section>\n");
}

/// Render one module's page — its doc-comment, its exposed types and values,
/// each entry carrying a stable `id` anchor and its cross-linked signature.
fn render_html_module(module: &ModuleDoc, index: &AnchorIndex) -> String {
    let mut body =
        String::from("<nav class=\"crumb\"><a href=\"index.html\">&larr; all modules</a></nav>\n");
    let _ = writeln!(body, "<h1>{}</h1>", html_escape(&module.name));
    if !module.comment.is_empty() {
        body.push_str(&render_comment_html(&module.comment));
    }

    if !module.unions.is_empty() {
        body.push_str("<h2>Types</h2>\n");
        for union in &module.unions {
            let _ = writeln!(
                body,
                "<section class=\"entry\" id=\"{}\">\n<h3><code>{}{}</code></h3>",
                html_escape(&union.name),
                html_escape(&union.name),
                html_escape(&union_params(union.params))
            );
            if !union.comment.is_empty() {
                body.push_str(&render_comment_html(&union.comment));
            }
            for ctor in &union.ctors {
                body.push_str("<pre class=\"sig\">");
                body.push_str(&html_escape(&ctor.name));
                for arg in &ctor.arg_types {
                    body.push(' ');
                    body.push_str(&html_signature(&signature_pieces(arg, index)));
                }
                body.push_str("</pre>\n");
            }
            body.push_str("</section>\n");
        }
    }

    if !module.values.is_empty() {
        body.push_str("<h2>Values</h2>\n");
        for value in &module.values {
            let sig = html_signature(&signature_pieces(&value.signature_ty, index));
            let _ = writeln!(
                body,
                "<section class=\"entry\" id=\"{}\">\n<h3><code>{}</code></h3>\n\
                 <pre class=\"sig\">{} : {sig}</pre>",
                html_escape(&value.name),
                html_escape(&value.name),
                html_escape(&value.name)
            );
            if !value.comment.is_empty() {
                body.push_str(&render_comment_html(&value.comment));
            }
            body.push_str("</section>\n");
        }
    }

    html_page(&module.name, &body)
}

// ===========================================================================
// serve — a read-only, loopback-only preview of the built HTML site.
// ===========================================================================

/// Build the HTML site for the package at `path` and serve it read-only on
/// loopback, blocking until interrupted.
///
/// The site is built once in memory (never written to disk — to keep files, run
/// `ipe doc`), then served from a single-threaded blocking HTTP/1.1 loop over
/// `std::net::TcpListener`. It binds `127.0.0.1` only; `port` `None` lets the OS
/// assign a free port (bind `:0`), `Some(n)` pins one and errors if it is taken.
///
/// The default browser is opened on the served URL; a headless caller sets
/// `IPE_DOC_NO_OPEN` to skip that (the URL is printed either way).
///
/// # Errors
/// [`CliError::Io`] if the loopback port cannot be bound (a pinned port already
/// in use), plus any error from [`build_docs`].
fn serve(path: &Path, port: Option<u16>) -> Result<(), CliError> {
    use std::net::TcpListener;

    let docs = build_docs(path).unwrap_or_else(|_| build_stdlib_only_docs());
    let site = render_site_for_serve(&docs);

    let addr = format!("127.0.0.1:{}", port.unwrap_or(0));
    let listener = TcpListener::bind(&addr).map_err(|e| crate::io_err(Path::new(&addr), e))?;
    let bound = listener
        .local_addr()
        .map_err(|e| crate::io_err(Path::new(&addr), e))?;

    let url = format!("http://{bound}/");
    print!(
        "{}",
        crate::style::frame(&crate::style::gutter(&format!(
            "serving docs at {url} (read-only, loopback; Ctrl-C to stop)"
        )))
    );
    // A headless caller (CI, a test, a remote shell) opts out of the browser pop
    // with `IPE_DOC_NO_OPEN`; the URL is already printed, so the preview stays
    // reachable.
    if std::env::var_os("IPE_DOC_NO_OPEN").is_none() {
        open_in_browser(&url);
    }

    // A single dropped connection must not take the server down; `flatten` skips
    // the `Err`s and serves each accepted stream.
    for mut conn in listener.incoming().flatten() {
        serve_one(&mut conn, &site);
    }
    Ok(())
}

/// Serve one HTTP/1.1 request from the in-memory `site`, read-only.
///
/// Reads the request line, maps its path to a built file (`/` → `index.html`),
/// and writes the file with its content type — or a `404` when the path names no
/// built file. Only `GET`/`HEAD`-shaped requests are honoured; the body, if any,
/// is ignored (nothing here writes or executes).
fn serve_one(conn: &mut std::net::TcpStream, site: &BTreeMap<String, String>) {
    use std::io::{BufRead, BufReader, Write};

    let mut reader = BufReader::new(match conn.try_clone() {
        Ok(c) => c,
        Err(_) => return,
    });
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }

    let path = request_line.split_whitespace().nth(1).unwrap_or("/");
    let name = serve_file_name(path);

    let response = site.get(&name).map_or_else(
        || http_response("404 Not Found", "text/plain; charset=utf-8", "not found\n"),
        |body| http_response("200 OK", content_type(&name), body),
    );
    let _ = conn.write_all(response.as_bytes());
    let _ = conn.flush();
}

/// Map a request path to a built-file key: `/` (or empty) → `index.html`,
/// otherwise the leading `/` is stripped and any `?query`/`#frag` dropped. The
/// result is a bare filename — a `..` or nested path simply misses the flat site
/// map and 404s, so no traversal can escape it.
fn serve_file_name(path: &str) -> String {
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let trimmed = path.trim_start_matches('/');
    if trimmed.is_empty() {
        "index.html".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// The content type for a built file, by extension.
fn content_type(name: &str) -> &'static str {
    match Path::new(name).extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("md") => "text/markdown; charset=utf-8",
        _ => "text/plain; charset=utf-8",
    }
}

/// A complete HTTP/1.1 response with an explicit `Content-Length` and a
/// `Connection: close` (the loop serves one request per connection).
fn http_response(status: &str, content_type: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// Best-effort open of `url` in the default browser. A failure is silent — the
/// URL is already printed, so the preview is reachable regardless.
fn open_in_browser(url: &str) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(opener).arg(url).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_owned()).collect()
    }

    #[test]
    fn parse_bare_is_generate_with_defaults() {
        let m = parse_doc(&[]).expect("empty doc");
        assert_eq!(
            m,
            DocMode::Generate {
                path: PathBuf::from("."),
                out: PathBuf::from("doc"),
                write_format: WriteFormat::All,
            }
        );
    }

    #[test]
    fn parse_generate_takes_path_and_out() {
        let m = parse_doc(&s(&["pkg", "--out", "site"])).expect("generate");
        assert_eq!(
            m,
            DocMode::Generate {
                path: PathBuf::from("pkg"),
                out: PathBuf::from("site"),
                write_format: WriteFormat::All,
            }
        );
    }

    #[test]
    fn parse_write_format_selects_a_single_rendering() {
        let m = parse_doc(&s(&["--write-format", "html"])).expect("write-format html");
        assert_eq!(
            m,
            DocMode::Generate {
                path: PathBuf::from("."),
                out: PathBuf::from("doc"),
                write_format: WriteFormat::Html,
            }
        );
    }

    #[test]
    fn parse_rejects_unknown_write_format() {
        assert!(matches!(
            parse_doc(&s(&["--write-format", "pdf"])),
            Err(CliError::UsageOwned(_))
        ));
    }

    #[test]
    fn parse_list_flag_returns_list_mode() {
        let m = parse_doc(&s(&["--list"])).expect("list");
        assert!(matches!(m, DocMode::List { .. }));
    }

    #[test]
    fn parse_list_flag_with_plain() {
        let m = parse_doc(&s(&["--list", "--plain"])).expect("list plain");
        assert!(matches!(
            m,
            DocMode::List {
                format: OutputFormat::Plain,
                ..
            }
        ));
    }

    #[test]
    fn parse_list_flag_with_json() {
        let m = parse_doc(&s(&["--list", "--json"])).expect("list json");
        assert!(matches!(
            m,
            DocMode::List {
                format: OutputFormat::Json,
                ..
            }
        ));
    }

    #[test]
    fn parse_bare_list_word_returns_list_mode() {
        let m = parse_doc(&s(&["list"])).expect("list");
        assert!(matches!(m, DocMode::List { .. }));
    }

    #[test]
    fn parse_bare_list_word_with_plain_and_json() {
        let plain = parse_doc(&s(&["list", "--plain"])).expect("list plain");
        assert!(matches!(
            plain,
            DocMode::List {
                format: OutputFormat::Plain,
                ..
            }
        ));
        let json = parse_doc(&s(&["list", "--json"])).expect("list json");
        assert!(matches!(
            json,
            DocMode::List {
                format: OutputFormat::Json,
                ..
            }
        ));
    }

    #[test]
    fn deprecated_list_flag_still_dispatches_and_warns() {
        // The `--list` alias must keep selecting `list` (never-break-users) while
        // emitting a one-time notice steering the caller to the bare word.
        let mut notices: Vec<String> = Vec::new();
        let m = parse_doc_with(&s(&["--list", "--plain"]), &mut |msg| {
            notices.push(msg.to_owned());
        })
        .expect("deprecated --list");
        assert!(matches!(
            m,
            DocMode::List {
                format: OutputFormat::Plain,
                ..
            }
        ));
        assert_eq!(
            notices.len(),
            1,
            "exactly one deprecation notice: {notices:?}"
        );
        let note = notices.first().map(String::as_str).unwrap_or_default();
        assert!(
            note.contains("--list") && note.contains("deprecated") && note.contains("doc list"),
            "the notice names the deprecated flag and its replacement: {note}"
        );
    }

    #[test]
    fn bare_list_word_emits_no_deprecation_notice() {
        let mut notices: Vec<String> = Vec::new();
        let _ = parse_doc_with(&s(&["list"]), &mut |msg| notices.push(msg.to_owned()))
            .expect("bare list");
        assert!(
            notices.is_empty(),
            "bare `list` is not deprecated: {notices:?}"
        );
    }

    #[test]
    fn parse_module_query_returns_query_mode() {
        let m = parse_doc(&s(&["Ipe.List"])).expect("query Ipe.List");
        assert_eq!(
            m,
            DocMode::Query {
                module: "Ipe.List".to_owned(),
                format: OutputFormat::Human,
            }
        );
    }

    #[test]
    fn parse_module_query_with_plain() {
        let m = parse_doc(&s(&["Ipe.String", "--plain"])).expect("query plain");
        assert_eq!(
            m,
            DocMode::Query {
                module: "Ipe.String".to_owned(),
                format: OutputFormat::Plain,
            }
        );
    }

    #[test]
    fn parse_module_query_with_json() {
        let m = parse_doc(&s(&["Ipe.Http", "--json"])).expect("query json");
        assert_eq!(
            m,
            DocMode::Query {
                module: "Ipe.Http".to_owned(),
                format: OutputFormat::Json,
            }
        );
    }

    #[test]
    fn parse_serve_auto_selects_port() {
        let m = parse_doc(&s(&["serve", "pkg"])).expect("serve");
        assert_eq!(
            m,
            DocMode::Serve {
                path: PathBuf::from("pkg"),
                port: None,
            }
        );
    }

    #[test]
    fn parse_serve_pins_a_port() {
        let m = parse_doc(&s(&["serve", "--port", "8080"])).expect("serve port");
        assert_eq!(
            m,
            DocMode::Serve {
                path: PathBuf::from("."),
                port: Some(8080),
            }
        );
    }

    #[test]
    fn serve_rejects_generate_flags() {
        // `--write-format`/`--out` are meaningless when serving HTML —
        // unrepresentable in `DocMode::Serve`, so rejected at the boundary.
        assert!(parse_doc(&s(&["serve", "--write-format", "json"])).is_err());
        assert!(parse_doc(&s(&["serve", "--out", "x"])).is_err());
    }

    #[test]
    fn generate_rejects_port_flag() {
        // `--port` has no meaning without a server; unrepresentable in Generate.
        assert!(parse_doc(&s(&["--port", "8080"])).is_err());
    }

    #[test]
    fn serve_rejects_malformed_port() {
        assert!(parse_doc(&s(&["serve", "--port", "nope"])).is_err());
        assert!(parse_doc(&s(&["serve", "--port", "0"])).is_err());
    }

    #[test]
    fn parse_check_takes_path() {
        let m = parse_doc(&s(&["check", "pkg"])).expect("check");
        assert_eq!(
            m,
            DocMode::Check {
                path: PathBuf::from("pkg"),
            }
        );
    }

    #[test]
    fn parse_check_defaults_path_to_cwd() {
        let m = parse_doc(&s(&["check"])).expect("check default");
        assert_eq!(
            m,
            DocMode::Check {
                path: PathBuf::from(".")
            }
        );
    }

    #[test]
    fn check_rejects_out_flag() {
        // `--out` is meaningless under check — and unrepresentable in `DocMode`,
        // so it is rejected at the boundary, not silently ignored.
        assert!(matches!(
            parse_doc(&s(&["check", "--out", "x"])),
            Err(CliError::UsageOwned(_))
        ));
    }

    #[test]
    fn generate_rejects_duplicate_out() {
        assert!(parse_doc(&s(&["--out", "a", "--out", "b"])).is_err());
    }

    #[test]
    fn generate_rejects_missing_out_value() {
        assert!(parse_doc(&s(&["--out"])).is_err());
    }

    #[test]
    fn rejects_unknown_flag() {
        assert!(matches!(
            parse_doc(&s(&["--bogus"])),
            Err(CliError::UsageOwned(_))
        ));
    }

    #[test]
    fn rejects_second_positional() {
        assert!(parse_doc(&s(&["a", "b"])).is_err());
    }

    #[test]
    fn scans_module_and_binding_comments() {
        let src = "-- | The module.\nmodule M exposing (foo)\n\n\
                   -- | The foo value.\n-- more foo.\nfoo : Int\nfoo = 1\n";
        let comments = scan_doc_comments(src);
        assert_eq!(comments.module, "The module.");
        assert_eq!(
            comments.get("foo").as_deref(),
            Some("The foo value.\nmore foo.")
        );
    }

    #[test]
    fn scans_type_and_alias_names() {
        let union = scan_doc_comments("-- | A color.\ntype Color = Red | Blue\n");
        assert_eq!(union.get("Color").as_deref(), Some("A color."));
        let alias = scan_doc_comments("-- | A name.\ntype alias Name = String\n");
        assert_eq!(alias.get("Name").as_deref(), Some("A name."));
    }

    #[test]
    fn undocumented_binding_has_empty_comment() {
        let comments = scan_doc_comments("module M exposing (foo)\nfoo : Int\nfoo = 1\n");
        assert!(comments.get("foo").is_none());
        assert!(comments.module.is_empty());
    }

    #[test]
    fn json_escapes_control_and_quote() {
        assert_eq!(json_string("a\"b\\c\n"), "\"a\\\"b\\\\c\\n\"");
    }

    /// A resolved type-constructor `TyDoc`, e.g. `con("M", "Color", [])`.
    fn con(module: &str, name: &str, args: Vec<TyDoc>) -> TyDoc {
        TyDoc::Con {
            module: module.into(),
            name: name.into(),
            args: args.into_boxed_slice(),
        }
    }

    /// A one-module package documenting a `Color` type and a `paint` value whose
    /// signature mentions both the in-package `Color` and the built-in `Int`.
    fn color_module() -> ModuleDoc {
        ModuleDoc {
            name: "M".to_owned(),
            kind: ModuleKind::Local,
            comment: "A module.".to_owned(),
            unions: vec![UnionDoc {
                name: "Color".to_owned(),
                params: 0,
                ctors: vec![CtorDoc {
                    name: "Red".to_owned(),
                    args: vec![],
                    arg_types: vec![],
                }],
                comment: "A color.".to_owned(),
            }],
            values: vec![ValueDoc {
                name: "paint".to_owned(),
                signature: "M.Color -> Int".to_owned(),
                // `M.Color` is in-package (links); `Int` is a built-in (plain).
                signature_ty: TyDoc::Fun(
                    Box::new(con("M", "Color", vec![])),
                    Box::new(con("", "Int", vec![])),
                ),
                comment: "The paint.".to_owned(),
            }],
        }
    }

    fn one_module_docs(module: ModuleDoc) -> DocsJson {
        DocsJson {
            version: DOCS_JSON_VERSION,
            modules: vec![module],
        }
    }

    #[test]
    fn markdown_renders_module_values_and_types() {
        let module = color_module();
        let docs = one_module_docs(color_module());
        let index = AnchorIndex::build(&docs);
        let md = render_markdown(&module, &index);
        assert!(md.contains("# M"));
        assert!(md.contains("A module."));
        assert!(md.contains("### `Color`"));
        assert!(md.contains("- `Red`"));
        assert!(md.contains("### `paint`"));
        assert!(md.contains("The paint."));
    }

    #[test]
    fn cross_reference_links_in_package_type_and_not_a_builtin() {
        let module = color_module();
        let docs = one_module_docs(color_module());
        let index = AnchorIndex::build(&docs);
        let value = module.values.first().expect("one value");
        let pieces = signature_pieces(&value.signature_ty, &index);

        // `M.Color` resolves in-package → a link to its anchor.
        let linked: Vec<&TypeRef> = pieces
            .iter()
            .filter_map(|p| match p {
                SigPiece::Link { target, .. } => Some(target),
                SigPiece::Text(_) => None,
            })
            .collect();
        assert_eq!(linked.len(), 1, "exactly the in-package type links");
        let only = linked.first().expect("one linked type");
        assert_eq!(only.anchor(), "M#Color");
        assert_eq!(only.href("html"), "M.html#Color");

        // The whole rendered text still reads as the signature, `Int` inline.
        let html = html_signature(&pieces);
        assert!(
            html.contains("<a href=\"M.html#Color\">M.Color</a>"),
            "{html}"
        );
        assert!(
            html.contains("-&gt; Int"),
            "the builtin stays plain: {html}"
        );
        assert!(
            !html.contains(">Int</a>"),
            "no link is emitted for a builtin"
        );
    }

    #[test]
    fn json_records_the_cross_reference() {
        let docs = one_module_docs(color_module());
        let json = render_json(&docs);
        assert!(json.contains("\"anchor\": \"M#Color\""), "{json}");
        // The built-in `Int` produces no reference entry.
        assert!(!json.contains("\"name\": \"Int\""), "{json}");
    }

    #[test]
    fn html_index_and_module_pages_are_self_contained() {
        let module = color_module();
        let docs = one_module_docs(color_module());
        let index = AnchorIndex::build(&docs);

        let idx = render_html_index(&docs);
        assert!(idx.contains("<!DOCTYPE html>"));
        assert!(idx.contains("href=\"style.css\""), "links the bundled CSS");
        assert!(
            idx.contains("href=\"M.html\">M</a>"),
            "lists the module: {idx}"
        );
        // The filter box and its no-dependency inline script are present.
        assert!(
            idx.contains("<input type=\"search\" id=\"filter\""),
            "the index carries a search box: {idx}"
        );
        assert!(
            idx.contains("addEventListener('input'"),
            "the inline filter script is bundled: {idx}"
        );
        // A local module lands under the project-modules section label.
        assert!(
            idx.contains(LABEL_PROJECT),
            "the project section label is present: {idx}"
        );

        let page = render_html_module(&module, &index);
        assert!(
            page.contains("id=\"Color\""),
            "the type has a stable anchor"
        );
        assert!(
            page.contains("id=\"paint\""),
            "the value has a stable anchor"
        );
        assert!(
            page.contains("<a href=\"M.html#Color\">M.Color</a>"),
            "the in-package type links: {page}"
        );
    }

    /// A minimal stdlib-kind module for ordering/labelling tests.
    fn stdlib_module(name: &str) -> ModuleDoc {
        ModuleDoc {
            name: name.to_owned(),
            kind: ModuleKind::Stdlib,
            comment: String::new(),
            unions: Vec::new(),
            values: Vec::new(),
        }
    }

    /// A minimal local-kind module for ordering/labelling tests.
    fn local_module(name: &str) -> ModuleDoc {
        ModuleDoc {
            name: name.to_owned(),
            kind: ModuleKind::Local,
            comment: String::new(),
            unions: Vec::new(),
            values: Vec::new(),
        }
    }

    #[test]
    fn html_index_groups_local_before_stdlib_with_section_labels() {
        // Feed both kinds; the model is already local-first (build_docs order).
        let docs = DocsJson {
            version: DOCS_JSON_VERSION,
            modules: vec![local_module("App"), stdlib_module("Ipe.List")],
        };
        let idx = render_html_index(&docs);

        // Both section labels render.
        assert!(idx.contains(LABEL_PROJECT), "project label: {idx}");
        assert!(idx.contains(LABEL_STDLIB), "stdlib label: {idx}");
        // The project section precedes the standard-library section.
        let p = idx.find(LABEL_PROJECT).expect("project label present");
        let s = idx.find(LABEL_STDLIB).expect("stdlib label present");
        assert!(p < s, "project section comes first: {idx}");
    }

    #[test]
    fn html_style_is_soft_dark_with_accent_and_multicolumn_module_list() {
        // The accent custom property is defined once in :root.
        assert!(STYLE_CSS.contains("--accent:"), "accent var: {STYLE_CSS}");
        // The module index lays out in responsive multi-column form.
        assert!(
            STYLE_CSS.contains("columns:"),
            "multi-column module list: {STYLE_CSS}"
        );
        // A soft dark background, not pure black.
        assert!(
            STYLE_CSS.contains("--bg:") && !STYLE_CSS.contains("--bg: #000"),
            "soft (non-pure-black) background: {STYLE_CSS}"
        );
        // It collapses to one column on a narrow viewport.
        assert!(
            STYLE_CSS.contains("columns: 1"),
            "collapses to one column when narrow: {STYLE_CSS}"
        );
    }

    #[test]
    fn json_records_the_module_kind() {
        let docs = DocsJson {
            version: DOCS_JSON_VERSION,
            modules: vec![local_module("App"), stdlib_module("Ipe.List")],
        };
        let json = render_json(&docs);
        assert!(json.contains("\"kind\": \"local\""), "{json}");
        assert!(json.contains("\"kind\": \"stdlib\""), "{json}");
        // Local precedes stdlib in the serialized order.
        let l = json.find("\"kind\": \"local\"").expect("local kind");
        let s = json.find("\"kind\": \"stdlib\"").expect("stdlib kind");
        assert!(l < s, "the model is serialized local-first: {json}");
    }

    #[test]
    fn html_escapes_markup_in_names_and_comments() {
        assert_eq!(html_escape("a<b>&\"'"), "a&lt;b&gt;&amp;&quot;&#39;");
    }

    #[test]
    fn render_comment_indented_block_preserves_relative_indentation() {
        let comment = "    line_a\n        line_b";
        let html = render_comment_html(comment);
        assert!(html.contains("<pre class=\"doc-code\"><code>"), "{html}");
        assert!(html.contains("line_a\n    line_b"), "{html}");
    }

    #[test]
    fn render_comment_inline_backtick_becomes_code_element() {
        let comment = "Call `foo` here.";
        let html = render_comment_html(comment);
        assert!(html.contains("<code>foo</code>"), "{html}");
        assert!(html.contains("<p class=\"comment\">"), "{html}");
    }

    #[test]
    fn render_comment_fenced_block_renders_in_pre_without_delimiters() {
        let comment = "Example:\n```ipe\nfoo =\n    bar\n```\nEnd.";
        let html = render_comment_html(comment);
        assert!(html.contains("<pre class=\"doc-code\"><code>"), "{html}");
        assert!(html.contains("foo =\n    bar"), "{html}");
        assert!(!html.contains("```"), "{html}");
        assert!(html.contains("End."), "{html}");
    }

    #[test]
    fn serve_file_name_maps_root_and_strips_query() {
        assert_eq!(serve_file_name("/"), "index.html");
        assert_eq!(serve_file_name(""), "index.html");
        assert_eq!(serve_file_name("/M.html"), "M.html");
        assert_eq!(serve_file_name("/style.css?v=1"), "style.css");
        // A traversal attempt is just a filename that misses the flat map.
        assert_eq!(serve_file_name("/../etc/passwd"), "../etc/passwd");
    }

    #[test]
    fn http_response_has_length_and_close() {
        let r = http_response("200 OK", "text/html; charset=utf-8", "hi");
        assert!(r.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(r.contains("Content-Length: 2\r\n"));
        assert!(r.contains("Connection: close\r\n"));
        assert!(r.ends_with("\r\n\r\nhi"));
    }

    // ── Per-format subfolder layout ────────────────────────────────────────

    #[test]
    fn render_site_split_writes_three_separate_maps() {
        let docs = DocsJson {
            version: DOCS_JSON_VERSION,
            modules: vec![local_module("App"), stdlib_module("Ipe.List")],
        };
        let (json, markdown, html) = render_site_split(&docs, WriteFormat::All);
        assert!(json.contains_key("docs.json"), "json map has docs.json");
        assert!(
            markdown.contains_key("index.md"),
            "markdown map has index.md"
        );
        assert!(
            markdown.contains_key("App.md"),
            "markdown map has module page"
        );
        assert!(
            markdown.contains_key("Ipe-List.md"),
            "markdown map has stdlib page"
        );
        assert!(html.contains_key("index.html"), "html map has index.html");
        assert!(html.contains_key("style.css"), "html map has stylesheet");
        assert!(html.contains_key("App.html"), "html map has module page");
    }

    #[test]
    fn render_site_split_json_only_omits_markdown_and_html() {
        let docs = one_module_docs(local_module("App"));
        let (json, markdown, html) = render_site_split(&docs, WriteFormat::Json);
        assert!(json.contains_key("docs.json"));
        assert!(markdown.is_empty(), "no markdown for Json format");
        assert!(html.is_empty(), "no html for Json format");
    }

    #[test]
    fn render_site_split_markdown_only_omits_html() {
        let docs = one_module_docs(local_module("App"));
        let (json, markdown, html) = render_site_split(&docs, WriteFormat::Markdown);
        assert!(json.contains_key("docs.json"));
        assert!(!markdown.is_empty());
        assert!(html.is_empty());
    }

    #[test]
    fn write_format_dir_creates_subfolder_and_writes_files() {
        use std::fs;
        let tmp = std::env::temp_dir().join(format!("ipe-doc-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let mut files = BTreeMap::new();
        files.insert("docs.json".to_owned(), "{\"v\":1}".to_owned());
        write_format_dir(&tmp, "json", &files).expect("write_format_dir");
        let written = tmp.join("json").join("docs.json");
        assert!(written.exists(), "docs.json written under json/");
        let _ = fs::remove_dir_all(&tmp);
    }

    // ── Stdlib docs without a project ─────────────────────────────────────

    #[test]
    fn build_stdlib_only_docs_contains_known_modules() {
        let docs = build_stdlib_only_docs();
        let names: Vec<&str> = docs.modules.iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"Ipe.List"), "Ipe.List in stdlib: {names:?}");
        assert!(
            names.contains(&"Ipe.String"),
            "Ipe.String in stdlib: {names:?}"
        );
        assert!(!docs.modules.is_empty(), "at least one module");
    }

    #[test]
    fn build_stdlib_only_docs_all_modules_are_stdlib_kind() {
        let docs = build_stdlib_only_docs();
        for m in &docs.modules {
            assert_eq!(
                m.kind,
                ModuleKind::Stdlib,
                "{} should be Stdlib kind",
                m.name
            );
        }
    }

    // ── Hierarchical namespace tree ────────────────────────────────────────

    #[test]
    fn namespace_tree_nests_children_under_shared_prefix() {
        let names = ["Ipe.Db", "Ipe.Db.Codec", "Ipe.Db.Store", "Ipe.List"];
        let tree = build_namespace_tree(&names);
        // Top level: Ipe (namespace only, has children)
        assert_eq!(tree.len(), 1, "one top-level node: Ipe");
        let ipe = tree.first().expect("one top-level Ipe node");
        assert_eq!(ipe.full_name, "Ipe");
        assert!(!ipe.is_module, "Ipe itself is not a module here");
        // Children of Ipe: Db, List
        let child_names: Vec<&str> = ipe.children.iter().map(|n| n.full_name.as_str()).collect();
        assert!(
            child_names.contains(&"Ipe.Db"),
            "Ipe.Db is a child: {child_names:?}"
        );
        assert!(
            child_names.contains(&"Ipe.List"),
            "Ipe.List is a child: {child_names:?}"
        );
        // Ipe.Db has children Codec and Store
        let db = ipe
            .children
            .iter()
            .find(|n| n.full_name == "Ipe.Db")
            .expect("Ipe.Db node");
        assert!(db.is_module, "Ipe.Db is itself a module");
        let db_children: Vec<&str> = db.children.iter().map(|n| n.full_name.as_str()).collect();
        assert!(
            db_children.contains(&"Ipe.Db.Codec"),
            "Codec nested: {db_children:?}"
        );
        assert!(
            db_children.contains(&"Ipe.Db.Store"),
            "Store nested: {db_children:?}"
        );
    }

    #[test]
    fn markdown_index_indents_submodule_two_spaces_under_parent() {
        let docs = DocsJson {
            version: DOCS_JSON_VERSION,
            modules: vec![
                stdlib_module("Ipe.Db"),
                stdlib_module("Ipe.Db.Codec"),
                stdlib_module("Ipe.Db.Store"),
                stdlib_module("Ipe.List"),
            ],
        };
        let idx = render_markdown_index(&docs);
        // Ipe.Db.Codec must appear indented under Ipe.Db — two extra spaces.
        // In the tree: Ipe (depth 0) -> Ipe.Db (depth 1) -> Ipe.Db.Codec (depth 2)
        // render_markdown_tree uses depth relative to its call site (0 for roots).
        // Root = Ipe at depth 0 → "- **Ipe**"
        // Ipe.Db at depth 1 → "  - [Ipe.Db](...)"
        // Ipe.Db.Codec at depth 2 → "    - [Ipe.Db.Codec](...)"
        let codec_line = idx
            .lines()
            .find(|l| l.contains("Ipe.Db.Codec"))
            .expect("Ipe.Db.Codec in index");
        let db_line = idx
            .lines()
            .find(|l| {
                l.contains("Ipe.Db)")
                    || (l.contains("Ipe.Db") && !l.contains("Codec") && !l.contains("Store"))
            })
            .expect("Ipe.Db in index");
        // codec_line must have more leading spaces than db_line
        let codec_indent = codec_line.len() - codec_line.trim_start().len();
        let db_indent = db_line.len() - db_line.trim_start().len();
        assert!(
            codec_indent > db_indent,
            "Ipe.Db.Codec is indented more than Ipe.Db: db={db_indent} codec={codec_indent}\n{idx}"
        );
        // The difference should be exactly 2 (one extra depth level)
        assert_eq!(
            codec_indent - db_indent,
            2,
            "exactly 2 extra spaces per depth: {idx}"
        );
    }

    #[test]
    fn html_index_nests_submodules_in_child_ul() {
        let docs = DocsJson {
            version: DOCS_JSON_VERSION,
            modules: vec![
                stdlib_module("Ipe.Db"),
                stdlib_module("Ipe.Db.Codec"),
                stdlib_module("Ipe.List"),
            ],
        };
        let idx = render_html_index(&docs);
        // Ipe.Db.Codec must appear inside a nested <ul> inside Ipe's <li>
        // The simplest proxy: Ipe.Db.Codec's <a> appears after Ipe.Db's <a>,
        // and there must be a nested <ul> between them.
        let db_pos = idx.find("Ipe-Db.html").expect("Ipe.Db link");
        let codec_pos = idx.find("Ipe-Db-Codec.html").expect("Ipe.Db.Codec link");
        assert!(codec_pos > db_pos, "Codec comes after Db in markup");
        // A <ul> must appear between Db and Codec (nesting).
        let between = &idx[db_pos..codec_pos];
        assert!(
            between.contains("<ul"),
            "nested <ul> between Db and Codec: {between}"
        );
    }

    #[test]
    fn pure_prefix_namespace_renders_as_non_link_header_in_html() {
        // "Ipe" has no module of its own; only Ipe.List and Ipe.String exist.
        let docs = DocsJson {
            version: DOCS_JSON_VERSION,
            modules: vec![stdlib_module("Ipe.List"), stdlib_module("Ipe.String")],
        };
        let idx = render_html_index(&docs);
        // "Ipe" should appear as a span.ns-header, not a link.
        assert!(
            idx.contains("class=\"ns-header\">Ipe<"),
            "Ipe prefix is a non-link header: {idx}"
        );
        assert!(
            !idx.contains("href=\"Ipe.html\""),
            "no link for pure prefix node: {idx}"
        );
    }

    #[test]
    fn markdown_index_renders_pure_prefix_as_bold_non_link() {
        let docs = DocsJson {
            version: DOCS_JSON_VERSION,
            modules: vec![stdlib_module("Ipe.List"), stdlib_module("Ipe.String")],
        };
        let idx = render_markdown_index(&docs);
        // "Ipe" prefix has no module → bold non-link header.
        assert!(idx.contains("**Ipe**"), "pure prefix is bold: {idx}");
        assert!(!idx.contains("[Ipe]"), "pure prefix is not linked: {idx}");
    }
}
