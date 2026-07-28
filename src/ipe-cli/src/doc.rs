//! `ipe doc` — API documentation generation.
//!
//! Generates reference documentation for an Ipê package from its own source: the
//! public API a consumer sees, each entry carrying its checker-inferred type
//! signature, its `-- |` doc-comment, and a stable source location.
//!
//! The command surface is a closed [`DocMode`] parsed at the CLI boundary, so an
//! invalid flag combination is unrepresentable downstream (parse, don't validate;
//! make-invalid-states-unrepresentable):
//!
//! * `ipe doc [PATH] [--out DIR]` — generate `docs.json` + per-module Markdown.
//! * `ipe doc check [PATH]` — a coverage gate that writes nothing and exits
//!   non-zero when an exposed binding lacks a doc-comment.
//!
//! The machine-readable [`docs.json`](DocsJson) is the source of truth — one
//! record per exposed module, with the module's doc-comment and its exposed
//! unions and values (name + type + comment). The Markdown rendering is a pure
//! view over that same in-memory model. The schema is versioned
//! ([`DOCS_JSON_VERSION`]) so a downstream consumer can rely on it.
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
//! What this first cut does not do (tracked as follow-ups): a self-contained HTML
//! site, cross-reference links between entries, and a local `serve` preview.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::CliError;
use crate::api_surface::{ModuleApi, ModulePath, PublicApi, UnionApi, extract_tree, read_tree};

/// The `docs.json` schema version. Bumped only on an incompatible shape change,
/// so a consumer can refuse a document it does not understand rather than
/// mis-reading it.
pub const DOCS_JSON_VERSION: u32 = 1;

/// What `ipe doc` was asked to do — a closed set.
///
/// No code past the parser can hold an invalid mix
/// (make-invalid-states-unrepresentable). `Generate` carries the one flag it
/// accepts (`--out`); `Check` carries none, so `ipe doc check --out X` has no
/// representation to construct.
#[derive(Debug, PartialEq, Eq)]
pub enum DocMode {
    /// Write `docs.json` and the per-module Markdown to `out`.
    Generate {
        /// The package to document — a directory or a single `.ipe` file.
        path: PathBuf,
        /// Where the rendered documentation is written.
        out: PathBuf,
    },
    /// Verify every exposed binding is documented; write nothing.
    Check {
        /// The package to check.
        path: PathBuf,
    },
}

/// The default output directory when `--out` is omitted, mirroring Elm's `doc/`.
const DEFAULT_OUT: &str = "doc";

/// The default package path when a positional is omitted — the current project.
const DEFAULT_PATH: &str = ".";

/// Parse `ipe doc`'s argument tail into a [`DocMode`].
///
/// The bare form is `generate`; a leading `check` selects the coverage gate.
/// `--out` is a `generate`-only flag, so it is rejected under `check` at this
/// boundary rather than silently ignored downstream.
///
/// # Errors
/// [`CliError::Usage`] / [`CliError::UsageOwned`] naming the exact problem: an
/// unknown flag, a `generate`-only flag under `check`, a repeated `--out`, a
/// missing `--out` value, or a second positional path.
pub fn parse_doc(rest: &[String]) -> Result<DocMode, CliError> {
    let mut it = rest.iter().peekable();

    // A leading `check` selects the check subcommand; anything else is a
    // positional path (or a flag) of the bare `generate` form.
    let is_check = matches!(it.peek(), Some(first) if first.as_str() == "check");
    if is_check {
        it.next();
    }

    let mut path: Option<String> = None;
    let mut out: Option<String> = None;
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--out" if is_check => {
                return Err(CliError::Usage(
                    "ipe doc check writes nothing, so it takes no --out; run `ipe doc` to \
                     generate files",
                ));
            }
            "--out" => {
                let value = it
                    .next()
                    .cloned()
                    .ok_or(CliError::Usage("ipe doc: --out needs a directory"))?;
                if out.is_some() {
                    return Err(CliError::Usage("ipe doc: --out given more than once"));
                }
                out = Some(value);
            }
            flag if flag.starts_with('-') => {
                return Err(CliError::UsageOwned(format!(
                    "ipe doc: unknown flag `{flag}`"
                )));
            }
            positional => {
                if path.is_some() {
                    return Err(CliError::Usage(
                        "ipe doc: expected a single <path> argument",
                    ));
                }
                path = Some(positional.to_owned());
            }
        }
    }

    let path = PathBuf::from(path.as_deref().unwrap_or(DEFAULT_PATH));
    if is_check {
        Ok(DocMode::Check { path })
    } else {
        Ok(DocMode::Generate {
            path,
            out: PathBuf::from(out.as_deref().unwrap_or(DEFAULT_OUT)),
        })
    }
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
        DocMode::Generate { path, out } => generate(&path, &out),
        DocMode::Check { path } => check(&path),
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

/// One exposed module's documentation: its doc-comment plus its exposed unions
/// and values.
#[derive(Debug, PartialEq, Eq)]
pub struct ModuleDoc {
    /// The dotted module name (`Ipe.String`).
    pub name: String,
    /// The module's own `-- |` header doc-comment, empty when it has none.
    pub comment: String,
    /// Exposed union types, in name order.
    pub unions: Vec<UnionDoc>,
    /// Exposed values, in name order.
    pub values: Vec<ValueDoc>,
}

/// One exposed value's documentation.
#[derive(Debug, PartialEq, Eq)]
pub struct ValueDoc {
    /// The value name.
    pub name: String,
    /// Its checker-inferred, α-canonicalised type signature.
    pub signature: String,
    /// Its `-- |` doc-comment, empty when it has none.
    pub comment: String,
}

/// One exposed union type's documentation.
#[derive(Debug, PartialEq, Eq)]
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
#[derive(Debug, PartialEq, Eq)]
pub struct CtorDoc {
    /// The constructor name.
    pub name: String,
    /// Its argument signatures, in declaration order.
    pub args: Vec<String>,
}

/// A binding whose doc-comment is missing, reported by [`check`].
#[derive(Debug, PartialEq, Eq)]
struct Undocumented {
    module: String,
    /// The binding name (a value, or a union type name).
    name: String,
}

/// Build the in-memory [`DocsJson`] for the package at `path`.
///
/// Joins the checker-provided public API (signatures + union shapes) with the
/// source-scanned doc-comments, by binding name per module.
fn build_docs(path: &Path) -> Result<DocsJson, CliError> {
    let api: PublicApi = extract_tree(path).map_err(CliError::from)?;
    let sources = read_tree(path).map_err(CliError::from)?;

    let mut modules = Vec::with_capacity(api.modules.len());
    for (module_path, module_api) in &api.modules {
        let comments = sources
            .get(module_path)
            .map(|(_, src)| scan_doc_comments(src))
            .unwrap_or_default();
        modules.push(module_doc(module_path, module_api, &comments));
    }
    Ok(DocsJson {
        version: DOCS_JSON_VERSION,
        modules,
    })
}

/// Assemble one [`ModuleDoc`] from its checked API surface and its scanned
/// doc-comments.
fn module_doc(module_path: &ModulePath, api: &ModuleApi, comments: &DocComments) -> ModuleDoc {
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
            comment: comments.get(name).unwrap_or_default(),
        })
        .collect();
    ModuleDoc {
        name: module_path.join("."),
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
fn plain_comment_text(line: &str) -> &str {
    line.strip_prefix("--").unwrap_or(line).trim_start()
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

/// Generate `docs.json` and per-module Markdown for the package at `path`,
/// writing them under `out`.
///
/// # Errors
/// As [`build_docs`], plus [`CliError::Io`] on a write failure.
fn generate(path: &Path, out: &Path) -> Result<(), CliError> {
    let docs = build_docs(path)?;

    std::fs::create_dir_all(out).map_err(|e| crate::io_err(out, e))?;

    let json_path = out.join("docs.json");
    std::fs::write(&json_path, render_json(&docs)).map_err(|e| crate::io_err(&json_path, e))?;

    for module in &docs.modules {
        let md_name = format!("{}.md", module.name.replace('.', "-"));
        let md_path = out.join(&md_name);
        std::fs::write(&md_path, render_markdown(module))
            .map_err(|e| crate::io_err(&md_path, e))?;
    }

    println!(
        "documented {} module{} to {}",
        docs.modules.len(),
        if docs.modules.len() == 1 { "" } else { "s" },
        out.display()
    );
    Ok(())
}

/// Verify every exposed binding in the package at `path` carries a doc-comment.
///
/// Writes nothing; exits non-zero (a [`CliError::Usage`] carrying the report)
/// when any exposed value or union type lacks a `-- |` comment. A CI-gateable
/// coverage check.
///
/// # Errors
/// As [`build_docs`], plus [`CliError::DocCoverage`] listing every undocumented
/// binding when coverage is incomplete.
fn check(path: &Path) -> Result<(), CliError> {
    let docs = build_docs(path)?;

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
        println!("all {exposed} exposed binding(s) are documented");
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

/// Render [`DocsJson`] as JSON.
///
/// A small hand-written serializer (the driver has no `serde` dependency) that
/// emits the versioned, stable schema: `{ "version", "modules": [ … ] }`. The
/// key order is fixed and the whole document is a deterministic function of the
/// model, so a consumer diffing two runs sees only real API changes.
fn render_json(docs: &DocsJson) -> String {
    let mut out = String::new();
    out.push_str("{\n");
    let _ = writeln!(out, "  \"version\": {},", docs.version);
    out.push_str("  \"modules\": [\n");
    for (i, module) in docs.modules.iter().enumerate() {
        render_module_json(&mut out, module);
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
fn render_module_json(out: &mut String, module: &ModuleDoc) {
    out.push_str("    {\n");
    let _ = writeln!(out, "      \"name\": {},", json_string(&module.name));
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
            "          \"comment\": {}",
            json_string(&value.comment)
        );
        out.push_str("        }");
    }
    out.push_str(if module.values.is_empty() {
        "]\n"
    } else {
        "\n      ]\n"
    });
    out.push_str("    }");
}

/// Encode a string as a JSON string literal, escaping the characters JSON
/// requires (`"`, `\`, and the C0 control set, with the short escapes for the
/// common ones).
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Render one module's documentation as Markdown — a pure view over its
/// [`ModuleDoc`].
fn render_markdown(module: &ModuleDoc) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# {}\n", module.name);
    if !module.comment.is_empty() {
        let _ = writeln!(out, "{}\n", module.comment);
    }

    if !module.unions.is_empty() {
        out.push_str("## Types\n\n");
        for union in &module.unions {
            let params = if union.params == 0 {
                String::new()
            } else {
                let names: Vec<String> = (0..union.params)
                    .map(|i| ipe_types::letters(u32::try_from(i).unwrap_or(u32::MAX)).to_string())
                    .collect();
                format!(" {}", names.join(" "))
            };
            let _ = writeln!(out, "### `{}{}`\n", union.name, params);
            if !union.comment.is_empty() {
                let _ = writeln!(out, "{}\n", union.comment);
            }
            for ctor in &union.ctors {
                let rendered = if ctor.args.is_empty() {
                    ctor.name.clone()
                } else {
                    format!("{} {}", ctor.name, ctor.args.join(" "))
                };
                let _ = writeln!(out, "- `{rendered}`");
            }
            out.push('\n');
        }
    }

    if !module.values.is_empty() {
        out.push_str("## Values\n\n");
        for value in &module.values {
            let _ = writeln!(out, "### `{} : {}`\n", value.name, value.signature);
            if !value.comment.is_empty() {
                let _ = writeln!(out, "{}\n", value.comment);
            }
        }
    }
    out
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
            }
        );
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
            Err(CliError::Usage(_))
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

    #[test]
    fn markdown_renders_module_values_and_types() {
        let module = ModuleDoc {
            name: "M".to_owned(),
            comment: "A module.".to_owned(),
            unions: vec![UnionDoc {
                name: "Color".to_owned(),
                params: 0,
                ctors: vec![CtorDoc {
                    name: "Red".to_owned(),
                    args: vec![],
                }],
                comment: "A color.".to_owned(),
            }],
            values: vec![ValueDoc {
                name: "foo".to_owned(),
                signature: "Int".to_owned(),
                comment: "The foo.".to_owned(),
            }],
        };
        let md = render_markdown(&module);
        assert!(md.contains("# M"));
        assert!(md.contains("A module."));
        assert!(md.contains("### `Color`"));
        assert!(md.contains("- `Red`"));
        assert!(md.contains("### `foo : Int`"));
        assert!(md.contains("The foo."));
    }
}
