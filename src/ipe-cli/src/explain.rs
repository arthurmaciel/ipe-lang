//! `ipe explain` — unified teaching interface over three page kinds.
//!
//! Provides a single queryable surface over error-codes, syntax forms, and
//! topic pages. Three entry shapes are supported:
//!
//! - `ipe explain` — a short friendly overview of what can be queried.
//! - `ipe explain <query>` — exact or fuzzy lookup across all kinds.
//! - `ipe explain list [kind]` — browse the index, optionally filtered.
//!
//! All three accept `--json` and `--plain`.
//!
//! ## Content model
//!
//! - **error-code**: the ~128 `IPE-*.md` pages; virtual kind, no frontmatter,
//!   title and body come from [`ipe_diagnostics`].
//! - **syntax** and **topic**: curated pages under `explain/syntax/` and
//!   `explain/topic/` in this crate, each carrying YAML frontmatter.
//!
//! Adding a new kind requires only authoring pages with that `kind:` value in
//! their frontmatter — no changes to the resolver, list, or JSON code.

use std::fmt::Write as _;

use ipe_diagnostics::{ALL_CODES, explain_page, title};

use crate::cli_args::OutputFormat;
use crate::style;

// ---------------------------------------------------------------------------
// Content model — typed page representation
// ---------------------------------------------------------------------------

/// The kind of a teaching page. Open vocabulary: new kinds ship as new pages
/// with the matching `kind:` frontmatter value.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PageKind {
    /// `IPE-XNNNN` diagnostic code pages — title/body from `ipe_diagnostics`.
    ErrorCode,
    /// Syntax-form pages (`case`, `let`, `if`, `|>`, …).
    Syntax,
    /// Topic pages (`effects`, `state`, `errors`, `shapes`, `main`, …).
    Topic,
    /// An unrecognised `kind:` value — carried forward so future kinds survive
    /// a binary that predates them without panicking.
    Other(String),
}

impl PageKind {
    /// The user-visible label for a kind, used in list output and footers.
    #[must_use]
    pub const fn label(&self) -> &str {
        match self {
            Self::ErrorCode => "error-code",
            Self::Syntax => "syntax",
            Self::Topic => "topic",
            Self::Other(s) => s.as_str(),
        }
    }

    /// Heading shown in `ipe explain list` for a kind group.
    const fn heading(&self) -> &str {
        match self {
            Self::ErrorCode => "Diagnostic error codes",
            Self::Syntax => "Syntax forms",
            Self::Topic => "Topics",
            Self::Other(s) => s.as_str(),
        }
    }

    fn from_str(s: &str) -> Self {
        match s {
            "error-code" => Self::ErrorCode,
            "syntax" => Self::Syntax,
            "topic" => Self::Topic,
            other => Self::Other(other.to_owned()),
        }
    }
}

/// A parsed teaching page, ready to render.
#[derive(Debug, Clone)]
pub struct Page {
    /// Stable identifier used in queries and `--json` output.
    pub id: String,
    pub kind: PageKind,
    pub title: String,
    pub summary: String,
    /// Raw body markdown (after frontmatter).
    pub body: String,
    /// Optional syntax reference (from frontmatter `syntax:`).
    pub syntax: Option<String>,
    /// Whether this page contains idiom rules (from frontmatter `idiom: true`).
    pub idiom: bool,
    /// Optional curated example text (from frontmatter `example:`).
    pub example: Option<String>,
    /// Related page ids.
    pub see_also: Vec<String>,
    /// Alternative query names — the resolver matches these too.
    pub aliases: Vec<String>,
}

impl Page {
    /// Render the page as a `--json` object with the stable schema.
    ///
    /// Schema: `{kind, id, title, summary, syntax, idiom, example,
    ///           see_also[], explain_ref}`.
    #[must_use]
    pub fn to_json(&self) -> String {
        let kind = json_str(self.kind.label());
        let id = json_str(&self.id);
        let title = json_str(&self.title);
        let summary = json_str(&self.summary);
        let syntax = self
            .syntax
            .as_deref()
            .map_or_else(|| "null".to_owned(), json_str);
        let idiom = if self.idiom { "true" } else { "false" };
        let example = self
            .example
            .as_deref()
            .map_or_else(|| "null".to_owned(), json_str);
        let see_also = json_str_array(&self.see_also);
        let explain_ref = json_str(&format!("ipe explain {}", self.id));
        format!(
            "{{\"kind\":{kind},\"id\":{id},\"title\":{title},\
             \"summary\":{summary},\"syntax\":{syntax},\
             \"idiom\":{idiom},\"example\":{example},\
             \"see_also\":{see_also},\"explain_ref\":{explain_ref}}}"
        )
    }

    /// Render a one-line list entry: `id  title  [kind]`.
    #[must_use]
    pub fn list_line_human(&self, p: &style::Palette) -> String {
        format!(
            "  {}{}{} {} {}{}[{}]{}",
            p.yellow,
            self.id,
            p.reset,
            self.title,
            p.dim,
            p.reset,
            self.kind.label(),
            p.reset
        )
    }

    /// Render the full human teaching page with gutter.
    #[must_use]
    pub fn render_human(&self, p: &style::Palette) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "{}{}{}  {}[{}]{}",
            p.bold,
            self.title,
            p.reset,
            p.dim,
            self.kind.label(),
            p.reset
        );
        let _ = writeln!(out, "{}{}{}", p.dim, self.summary, p.reset);
        out.push('\n');
        out.push_str(&self.body);
        if !out.ends_with('\n') {
            out.push('\n');
        }
        // Related pages footer — cross-kind links
        if !self.see_also.is_empty() {
            out.push('\n');
            let _ = write!(out, "{}Related:{} ", p.bold, p.reset);
            let related: Vec<String> = self
                .see_also
                .iter()
                .map(|id| {
                    let kind_tag = all_pages()
                        .into_iter()
                        .find(|pg| &pg.id == id)
                        .map(|pg| pg.kind.label().to_owned())
                        .unwrap_or_default();
                    if kind_tag.is_empty() {
                        format!("{}{}{}", p.yellow, id, p.reset)
                    } else {
                        format!(
                            "{}{}{} {}[{}]{}",
                            p.yellow, id, p.reset, p.dim, kind_tag, p.reset
                        )
                    }
                })
                .collect();
            out.push_str(&related.join(", "));
            out.push('\n');
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Frontmatter parser (parse, don't validate)
// ---------------------------------------------------------------------------

/// Parsed YAML frontmatter from a curated page.
#[derive(Debug, Default)]
struct Frontmatter {
    kind: Option<String>,
    title: Option<String>,
    summary: Option<String>,
    syntax: Option<String>,
    idiom: bool,
    example: Option<String>,
    see_also: Vec<String>,
    aliases: Vec<String>,
}

/// Parse a markdown page with optional YAML frontmatter.
///
/// Returns `(frontmatter, body)` where `body` is everything after the closing
/// `---`. If the page has no frontmatter the struct is default and body is
/// the whole input.
fn parse_frontmatter(source: &str) -> (Frontmatter, &str) {
    let Some(after_first) = source.strip_prefix("---\n") else {
        return (Frontmatter::default(), source);
    };
    let Some(end) = after_first.find("\n---\n") else {
        return (Frontmatter::default(), source);
    };
    let yaml = &after_first[..end];
    let body = &after_first[end + 5..]; // skip "\n---\n"
    (parse_yaml_block(yaml), body)
}

/// Minimal YAML parser for the flat frontmatter subset used in explain pages.
///
/// Handles:
/// - `key: scalar value`
/// - `key: true` / `key: false` (bool)
/// - `key: [item1, item2]` (inline arrays)
/// - `key:` followed by `  - item` lines (block arrays)
fn parse_yaml_block(yaml: &str) -> Frontmatter {
    let mut fm = Frontmatter::default();
    let mut lines = yaml.lines().peekable();
    while let Some(line) = lines.next() {
        let Some((raw_key, raw_val)) = line.split_once(':') else {
            continue;
        };
        let key = raw_key.trim();
        let val = raw_val.trim();

        // Inline array: `key: [a, b, c]`
        if val.starts_with('[') && val.ends_with(']') {
            let items = parse_inline_array(val);
            match key {
                "see_also" => fm.see_also = items,
                "aliases" => fm.aliases = items,
                _ => {}
            }
            continue;
        }

        // Scalar value
        if !val.is_empty() {
            match key {
                "kind" => fm.kind = Some(val.to_owned()),
                "title" => fm.title = Some(unquote(val).to_owned()),
                "summary" => fm.summary = Some(unquote(val).to_owned()),
                "syntax" => fm.syntax = Some(unquote(val).to_owned()),
                "example" => fm.example = Some(unquote(val).to_owned()),
                "idiom" => fm.idiom = val == "true",
                _ => {}
            }
            continue;
        }

        // Block array — collect `  - item` lines that follow
        let mut items: Vec<String> = Vec::new();
        while let Some(&next) = lines.peek() {
            if let Some(rest) = next.trim_start().strip_prefix("- ") {
                items.push(unquote(rest.trim()).to_owned());
                lines.next();
            } else {
                break;
            }
        }
        match key {
            "see_also" => fm.see_also = items,
            "aliases" => fm.aliases = items,
            _ => {}
        }
    }
    fm
}

fn parse_inline_array(s: &str) -> Vec<String> {
    let inner = s.trim_start_matches('[').trim_end_matches(']');
    inner
        .split(',')
        .map(|item| unquote(item.trim()).to_owned())
        .filter(|s| !s.is_empty())
        .collect()
}

fn unquote(s: &str) -> &str {
    let s = s.trim();
    if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

// ---------------------------------------------------------------------------
// Page registry — include_str! at compile time, parsed lazily
// ---------------------------------------------------------------------------

/// All curated (non-error-code) pages, embedded at compile time.
///
/// Each entry is `(id, raw_source)`. Adding a new page = add one entry here.
static CURATED_PAGES_RAW: &[(&str, &str)] = &[
    // Syntax pages
    ("case", include_str!("../explain/syntax/case.md")),
    ("let", include_str!("../explain/syntax/let.md")),
    ("if", include_str!("../explain/syntax/if.md")),
    ("do", include_str!("../explain/syntax/do.md")),
    ("type", include_str!("../explain/syntax/type.md")),
    (
        "type-alias",
        include_str!("../explain/syntax/type-alias.md"),
    ),
    ("record", include_str!("../explain/syntax/record.md")),
    (
        "record-update",
        include_str!("../explain/syntax/record-update.md"),
    ),
    (
        "or-pattern",
        include_str!("../explain/syntax/or-pattern.md"),
    ),
    ("pipe", include_str!("../explain/syntax/pipe.md")),
    ("lambda", include_str!("../explain/syntax/lambda.md")),
    ("module", include_str!("../explain/syntax/module.md")),
    ("import", include_str!("../explain/syntax/import.md")),
    // Topic pages
    ("effects", include_str!("../explain/topic/effects.md")),
    ("state", include_str!("../explain/topic/state.md")),
    ("errors", include_str!("../explain/topic/errors.md")),
    ("shapes", include_str!("../explain/topic/shapes.md")),
    ("main", include_str!("../explain/topic/main.md")),
];

/// Parse one curated raw source into a `Page`. Returns `None` if the source
/// is missing required frontmatter fields — parse, don't validate: we reject
/// at the boundary rather than carrying a half-formed page downstream.
fn parse_curated(id: &str, source: &str) -> Option<Page> {
    let (fm, body) = parse_frontmatter(source);
    let kind = PageKind::from_str(fm.kind.as_deref().unwrap_or(""));
    let title = fm.title?;
    let summary = fm.summary.unwrap_or_default();
    Some(Page {
        id: id.to_owned(),
        kind,
        title,
        summary,
        body: body.to_owned(),
        syntax: fm.syntax,
        idiom: fm.idiom,
        example: fm.example,
        see_also: fm.see_also,
        aliases: fm.aliases,
    })
}

/// Build a `Page` for a diagnostic error-code entry (virtual kind).
fn error_code_page(code: ipe_diagnostics::Code) -> Page {
    let id = code.as_str().to_owned();
    let t = title(code).to_owned();
    let body = explain_page(code).unwrap_or("").to_owned();
    Page {
        kind: PageKind::ErrorCode,
        title: t.clone(),
        summary: t,
        body,
        id,
        syntax: None,
        idiom: false,
        example: None,
        see_also: Vec::new(),
        aliases: Vec::new(),
    }
}

/// All pages across all kinds, in stable order: error-codes first (taxonomy
/// order), then curated pages in registration order.
#[must_use]
pub fn all_pages() -> Vec<Page> {
    let mut pages: Vec<Page> = ALL_CODES.iter().copied().map(error_code_page).collect();
    for &(id, src) in CURATED_PAGES_RAW {
        if let Some(p) = parse_curated(id, src) {
            pages.push(p);
        }
    }
    pages
}

// ---------------------------------------------------------------------------
// Resolver — exact-first, kind-agnostic
// ---------------------------------------------------------------------------

/// A ranked match from the fuzzy resolver.
#[derive(Debug)]
pub struct Match {
    pub id: String,
    pub kind: PageKind,
    pub title: String,
    /// Lower = better. 0 = exact.
    pub score: usize,
}

/// Resolve a query across all page kinds.
///
/// - Exact match (case-insensitive, aliases included) → score 0.
/// - Otherwise ranked by Levenshtein distance.
///
/// Returns `(exact_page, ranked_matches)`. `exact_page` is `Some` only on an
/// exact hit; `ranked_matches` always contains the top candidates (excluding
/// the exact hit when one exists).
#[must_use]
pub fn resolve(query: &str) -> (Option<Page>, Vec<Match>) {
    let canonical = query.trim().to_ascii_lowercase();
    let canonical_upper = query.trim().to_ascii_uppercase();
    let pages = all_pages();

    // Check for exact match: id (case-insensitive) or any alias
    let exact_idx = pages.iter().position(|p| {
        p.id.to_ascii_lowercase() == canonical
            || p.id.to_ascii_uppercase() == canonical_upper
            || p.aliases
                .iter()
                .any(|a| a.to_ascii_lowercase() == canonical)
    });

    if let Some(idx) = exact_idx {
        let exact = pages.get(idx).cloned();
        // Build related matches from remaining pages, ranked
        let mut scored: Vec<Match> = pages
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != idx)
            .map(|(_, p)| {
                let dist = best_distance(&canonical, p);
                Match {
                    id: p.id.clone(),
                    kind: p.kind.clone(),
                    title: p.title.clone(),
                    score: dist,
                }
            })
            .filter(|m| m.score <= 4)
            .collect();
        scored.sort_by(|a, b| a.score.cmp(&b.score).then_with(|| a.id.cmp(&b.id)));
        scored.truncate(5);
        return (exact, scored);
    }

    // No exact match: rank all pages
    let mut scored: Vec<Match> = pages
        .iter()
        .map(|p| {
            let dist = best_distance(&canonical, p);
            Match {
                id: p.id.clone(),
                kind: p.kind.clone(),
                title: p.title.clone(),
                score: dist,
            }
        })
        .collect();
    scored.sort_by(|a, b| a.score.cmp(&b.score).then_with(|| a.id.cmp(&b.id)));
    scored.truncate(8);
    (None, scored)
}

/// The best Levenshtein distance between `query` and any of a page's
/// searchable names (id, aliases, title words).
fn best_distance(query: &str, page: &Page) -> usize {
    let id_lower = page.id.to_ascii_lowercase();
    let mut best = levenshtein(query, &id_lower);
    for alias in &page.aliases {
        best = best.min(levenshtein(query, &alias.to_ascii_lowercase()));
    }
    // Also check title as a whole (lowercased)
    let title_lower = page.title.to_ascii_lowercase();
    best = best.min(levenshtein(query, &title_lower));
    best
}

/// Classic two-row Levenshtein. No slice indexing — uses only push/get/last.
fn levenshtein(a: &str, b: &str) -> usize {
    let b_chars: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b_chars.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur: Vec<usize> = Vec::with_capacity(b_chars.len().saturating_add(1));
        cur.push(i.saturating_add(1));
        for (j, &cb) in b_chars.iter().enumerate() {
            let cost = usize::from(ca != cb);
            let del = prev.get(j.saturating_add(1)).copied().unwrap_or(usize::MAX);
            let ins = cur.get(j).copied().unwrap_or(usize::MAX);
            let sub = prev.get(j).copied().unwrap_or(usize::MAX);
            cur.push(
                del.saturating_add(1)
                    .min(ins.saturating_add(1))
                    .min(sub.saturating_add(cost)),
            );
        }
        prev = cur;
    }
    prev.last().copied().unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Anti-pattern → topic SSOT map (for diagnostic nudges)
// ---------------------------------------------------------------------------

/// The single source of truth mapping a diagnostic anti-pattern to the topic
/// page that teaches the right approach. Used by diagnostic hints that append
/// `-> run 'ipe explain <topic>'`.
///
/// Every topic named here must have a corresponding page in [`CURATED_PAGES_RAW`].
pub const ANTI_PATTERN_TOPICS: &[(&str, &str)] = &[
    // `let _ = task` — discarding a Task effect
    ("let_discard_task", "effects"),
    // bare `Io.println` in multi-step logic without andThen
    ("bare_println_multistep", "effects"),
    // function value in a record field
    ("function_in_record", "state"),
    // non-TEA state pattern
    ("non_tea_state", "state"),
    // non-`Task` main
    ("non_task_main", "main"),
];

/// Look up the topic for an anti-pattern key. Returns `None` when the key has
/// no registered topic (so a hint is never fabricated for an unshipped page).
#[must_use]
pub fn topic_for_anti_pattern(key: &str) -> Option<&'static str> {
    ANTI_PATTERN_TOPICS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, topic)| *topic)
}

// ---------------------------------------------------------------------------
// Renderers
// ---------------------------------------------------------------------------

/// Render the short overview shown by bare `ipe explain`.
#[must_use]
pub fn render_overview(format: OutputFormat, stream: &impl std::io::IsTerminal) -> String {
    match format {
        OutputFormat::Plain => "ipe explain — teaching reference\n\
             \n\
             ipe explain <query>        look up a diagnostic code, syntax form, or topic\n\
             ipe explain list           browse all pages\n\
             ipe explain list syntax    browse syntax pages\n\
             ipe explain list topics    browse topic pages\n\
             ipe explain list error-codes  browse diagnostic codes\n\
             \n\
             Examples:\n\
               ipe explain effects\n\
               ipe explain case\n\
               ipe explain IPE-T0001\n\
             \n\
             Flags: --plain (ANSI-free) | --json (machine-readable)\n"
            .to_owned(),
        OutputFormat::Json => "{\"overview\":\"ipe explain: teaching reference over error-codes, \
             syntax forms, and topics. Use ipe explain list for the index or \
             ipe explain <query> to look up a page.\"}\n"
            .to_owned(),
        OutputFormat::Human => {
            let p = style::Palette::for_stream(stream);
            let mut body = String::new();
            let _ = writeln!(
                body,
                "{}ipe explain{} — teaching reference\n",
                p.bold, p.reset
            );
            let _ = writeln!(body, "  {}ipe explain <query>{}", p.yellow, p.reset);
            let _ = writeln!(
                body,
                "      look up a diagnostic code, syntax form, or topic\n"
            );
            let _ = writeln!(body, "  {}ipe explain list{}", p.yellow, p.reset);
            let _ = writeln!(body, "      browse all pages (grouped by kind)\n");
            let _ = writeln!(
                body,
                "  {}ipe explain list syntax|topics|error-codes{}",
                p.yellow, p.reset
            );
            let _ = writeln!(body, "      browse one kind only\n");
            let _ = writeln!(body, "{}Examples:{}", p.bold, p.reset);
            let _ = writeln!(
                body,
                "  {}ipe explain effects{}   — the Task effect discipline",
                p.yellow, p.reset
            );
            let _ = writeln!(
                body,
                "  {}ipe explain case{}      — pattern matching syntax",
                p.yellow, p.reset
            );
            let _ = writeln!(
                body,
                "  {}ipe explain IPE-T0001{} — a diagnostic code page",
                p.yellow, p.reset
            );
            let _ = writeln!(
                body,
                "\n{}Flags:{} --plain (ANSI-free output) | --json (machine-readable)",
                p.bold, p.reset
            );
            style::frame(&style::gutter(&body))
        }
    }
}

/// Render `ipe explain list [kind]` output.
///
/// `kind_filter` is `None` (all kinds) or `Some(PageKind)` (one kind).
#[must_use]
pub fn render_list(
    format: OutputFormat,
    kind_filter: Option<&PageKind>,
    stream: &impl std::io::IsTerminal,
) -> String {
    let pages = all_pages();
    let filtered: Vec<&Page> = pages
        .iter()
        .filter(|p| kind_filter.is_none_or(|k| &p.kind == k))
        .collect();

    match format {
        OutputFormat::Plain => {
            let mut out = String::new();
            for p in &filtered {
                let _ = writeln!(out, "{}\t{}\t{}", p.id, p.kind.label(), p.title);
            }
            out
        }
        OutputFormat::Json => {
            let items: Vec<String> = filtered
                .iter()
                .map(|p| {
                    format!(
                        "{{\"id\":{},\"kind\":{},\"title\":{},\"summary\":{}}}",
                        json_str(&p.id),
                        json_str(p.kind.label()),
                        json_str(&p.title),
                        json_str(&p.summary)
                    )
                })
                .collect();
            format!("{{\"pages\":[{}]}}\n", items.join(","))
        }
        OutputFormat::Human => {
            let p = style::Palette::for_stream(stream);
            let mut body = String::new();

            // Group by kind in a stable order
            let kind_order: &[PageKind] = &[PageKind::ErrorCode, PageKind::Syntax, PageKind::Topic];

            let mut printed_any = false;
            for kind in kind_order {
                if kind_filter.is_some_and(|k| k != kind) {
                    continue;
                }
                let group: Vec<&Page> = filtered
                    .iter()
                    .copied()
                    .filter(|pg| &pg.kind == kind)
                    .collect();
                if group.is_empty() {
                    continue;
                }
                if printed_any {
                    body.push('\n');
                }
                let _ = writeln!(body, "{}{}{}:", p.bold, kind.heading(), p.reset);
                for pg in group {
                    let _ = writeln!(body, "  {}{}{}  {}", p.yellow, pg.id, p.reset, pg.title);
                }
                printed_any = true;
            }

            // Unknown kinds (future-proof)
            let other_group: Vec<&Page> = filtered
                .iter()
                .copied()
                .filter(|pg| matches!(pg.kind, PageKind::Other(_)))
                .collect();
            if !other_group.is_empty() {
                if printed_any {
                    body.push('\n');
                }
                for pg in other_group {
                    let _ = writeln!(
                        body,
                        "  {}{}{}  {}  {}[{}]{}",
                        p.yellow,
                        pg.id,
                        p.reset,
                        pg.title,
                        p.dim,
                        pg.kind.label(),
                        p.reset
                    );
                }
            }

            if !body.is_empty() {
                let _ = writeln!(
                    body,
                    "\n{}tip:{} run {}ipe explain <id>{} for the full page",
                    p.dim, p.reset, p.yellow, p.reset
                );
            }
            style::frame(&style::gutter(&body))
        }
    }
}

/// Render the `{query, resolved, matches}` query envelope for `--json`.
#[must_use]
pub fn render_query_json(query: &str, exact: Option<&Page>, matches: &[Match]) -> String {
    let resolved = exact.map_or_else(|| "null".to_owned(), Page::to_json);
    let match_items: Vec<String> = matches
        .iter()
        .map(|m| {
            format!(
                "{{\"id\":{},\"kind\":{},\"title\":{},\"score\":{}}}",
                json_str(&m.id),
                json_str(m.kind.label()),
                json_str(&m.title),
                m.score
            )
        })
        .collect();
    format!(
        "{{\"query\":{},\"resolved\":{},\"matches\":[{}]}}\n",
        json_str(query),
        resolved,
        match_items.join(",")
    )
}

/// Render the chooser for a non-exact human query: ranked candidates grouped by kind.
#[must_use]
pub fn render_chooser(query: &str, matches: &[Match], p: &style::Palette) -> String {
    let mut body = String::new();
    let _ = writeln!(
        body,
        "{}No exact match for {:?}. Did you mean one of these?{}",
        p.bold, query, p.reset
    );

    if matches.is_empty() {
        let _ = writeln!(
            body,
            "\nNo close matches found. Try {}ipe explain list{} to browse all pages.",
            p.yellow, p.reset
        );
    } else {
        // Group by kind
        let mut by_kind: std::collections::BTreeMap<String, Vec<&Match>> =
            std::collections::BTreeMap::new();
        for m in matches {
            by_kind
                .entry(m.kind.label().to_owned())
                .or_default()
                .push(m);
        }
        for (kind_label, group) in &by_kind {
            let _ = writeln!(body, "\n  {}{}{}:", p.bold, kind_label, p.reset);
            for m in group {
                let _ = writeln!(body, "    {}{}{}  {}", p.yellow, m.id, p.reset, m.title);
            }
        }
        let _ = writeln!(
            body,
            "\nRun {}ipe explain <id>{} for the full page.",
            p.yellow, p.reset
        );
    }
    style::frame(&style::gutter(&body))
}

// ---------------------------------------------------------------------------
// JSON helpers (no serde dependency — the schema is simple)
// ---------------------------------------------------------------------------

fn json_str(s: &str) -> String {
    // Escape backslash and double-quote; these pages contain no control chars.
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

fn json_str_array(items: &[String]) -> String {
    let inner: Vec<String> = items.iter().map(|s| json_str(s)).collect();
    format!("[{}]", inner.join(","))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- page loader --

    #[test]
    fn all_pages_includes_error_codes_and_curated() {
        let pages = all_pages();
        let ec_count = pages
            .iter()
            .filter(|p| p.kind == PageKind::ErrorCode)
            .count();
        assert_eq!(
            ec_count,
            ALL_CODES.len(),
            "one error-code page per taxonomy code"
        );
        let has_curated = pages.iter().any(|p| p.kind != PageKind::ErrorCode);
        assert!(has_curated, "at least one curated (syntax/topic) page");
    }

    #[test]
    fn curated_pages_parse_required_fields() {
        let pages = all_pages();
        for p in pages.iter().filter(|p| p.kind != PageKind::ErrorCode) {
            assert!(!p.title.is_empty(), "page {} has empty title", p.id);
            assert!(!p.summary.is_empty(), "page {} has empty summary", p.id);
        }
    }

    #[test]
    fn syntax_and_topic_pages_present() {
        let pages = all_pages();
        let has_syntax = pages.iter().any(|p| p.kind == PageKind::Syntax);
        let has_topic = pages.iter().any(|p| p.kind == PageKind::Topic);
        assert!(has_syntax, "at least one syntax page must exist");
        assert!(has_topic, "at least one topic page must exist");
    }

    // -- resolver --

    #[test]
    fn resolver_exact_match_on_error_code() {
        let (exact, _) = resolve("IPE-T0001");
        assert!(exact.is_some(), "IPE-T0001 must resolve exactly");
        let page = exact.unwrap();
        assert_eq!(page.kind, PageKind::ErrorCode);
        assert_eq!(page.id, "IPE-T0001");
    }

    #[test]
    fn resolver_exact_match_case_insensitive() {
        let (exact, _) = resolve("ipe-t0001");
        assert!(exact.is_some(), "case-insensitive exact must resolve");
    }

    #[test]
    fn resolver_exact_match_on_syntax_page() {
        let (exact, _) = resolve("case");
        assert!(exact.is_some(), "`case` syntax page must resolve");
        let page = exact.unwrap();
        assert_eq!(page.kind, PageKind::Syntax);
    }

    #[test]
    fn resolver_exact_match_on_topic_page() {
        let (exact, _) = resolve("effects");
        assert!(exact.is_some(), "`effects` topic page must resolve");
        let page = exact.unwrap();
        assert_eq!(page.kind, PageKind::Topic);
    }

    #[test]
    fn resolver_alias_match() {
        // "task" is an alias for the effects topic
        let (exact, _) = resolve("task");
        assert!(
            exact.is_some(),
            "alias `task` must resolve to the effects page"
        );
        let page = exact.unwrap();
        assert_eq!(page.id, "effects");
    }

    #[test]
    fn resolver_fuzzy_returns_candidates_for_near_miss() {
        let (exact, matches) = resolve("efects"); // typo
        assert!(exact.is_none(), "typo should not exact-match");
        assert!(
            !matches.is_empty(),
            "near-miss should yield fuzzy candidates"
        );
    }

    #[test]
    fn resolver_no_match_returns_empty_matches_not_panic() {
        let (exact, _matches) = resolve("xyzzy_not_a_real_page_name_9999");
        assert!(exact.is_none());
        // Must not panic; matches may be empty for a truly distant query.
    }

    // -- list grouping --

    #[test]
    fn list_json_is_valid_json_envelope() {
        let out = render_list(OutputFormat::Json, None, &std::io::stdout());
        assert!(
            out.starts_with("{\"pages\":["),
            "list --json starts with {{\"pages\":["
        );
    }

    #[test]
    fn list_plain_is_ansi_free() {
        let out = render_list(OutputFormat::Plain, None, &std::io::stdout());
        assert!(
            !out.contains('\x1b'),
            "--plain output must not contain ANSI escapes"
        );
    }

    #[test]
    fn list_plain_contains_error_codes_syntax_and_topic() {
        let out = render_list(OutputFormat::Plain, None, &std::io::stdout());
        assert!(out.contains("IPE-T0001"), "list plain contains error codes");
        assert!(out.contains("case"), "list plain contains syntax pages");
        assert!(out.contains("effects"), "list plain contains topic pages");
    }

    #[test]
    fn list_kind_filter_syntax_only() {
        let out = render_list(
            OutputFormat::Plain,
            Some(&PageKind::Syntax),
            &std::io::stdout(),
        );
        assert!(out.contains("case"), "syntax filter includes case page");
        assert!(
            !out.contains("IPE-T0001"),
            "syntax filter excludes error-code pages"
        );
    }

    #[test]
    fn list_kind_filter_topic_only() {
        let out = render_list(
            OutputFormat::Plain,
            Some(&PageKind::Topic),
            &std::io::stdout(),
        );
        assert!(
            out.contains("effects"),
            "topic filter includes effects page"
        );
        assert!(
            !out.contains("IPE-T0001"),
            "topic filter excludes error-code pages"
        );
    }

    // -- --json schema snapshot --

    #[test]
    fn json_schema_snapshot_for_error_code_page() {
        let (exact, _) = resolve("IPE-T0001");
        let page = exact.expect("IPE-T0001 must resolve");
        let json = page.to_json();
        // Stable schema fields must all be present
        assert!(json.contains("\"kind\""), "schema has kind");
        assert!(json.contains("\"id\""), "schema has id");
        assert!(json.contains("\"title\""), "schema has title");
        assert!(json.contains("\"summary\""), "schema has summary");
        assert!(json.contains("\"syntax\""), "schema has syntax");
        assert!(json.contains("\"idiom\""), "schema has idiom");
        assert!(json.contains("\"example\""), "schema has example");
        assert!(json.contains("\"see_also\""), "schema has see_also");
        assert!(json.contains("\"explain_ref\""), "schema has explain_ref");
        // Values
        assert!(json.contains("\"error-code\""), "kind is error-code");
        assert!(json.contains("\"IPE-T0001\""), "id is IPE-T0001");
        assert!(
            json.contains("ipe explain IPE-T0001"),
            "explain_ref carries the command"
        );
    }

    #[test]
    fn json_schema_snapshot_for_topic_page() {
        let (exact, _) = resolve("effects");
        let page = exact.expect("effects must resolve");
        let json = page.to_json();
        assert!(json.contains("\"topic\""), "kind is topic");
        assert!(json.contains("\"effects\""), "id is effects");
        assert!(json.contains("\"explain_ref\""), "schema has explain_ref");
    }

    #[test]
    fn json_schema_snapshot_for_syntax_page() {
        let (exact, _) = resolve("case");
        let page = exact.expect("case must resolve");
        let json = page.to_json();
        assert!(json.contains("\"case\""), "id is case");
        assert!(json.contains("\"explain_ref\""), "schema has explain_ref");
    }

    #[test]
    fn query_json_envelope_exact() {
        let (exact, matches) = resolve("effects");
        let json = render_query_json("effects", exact.as_ref(), &matches);
        assert!(json.starts_with("{\"query\":"), "envelope has query");
        assert!(json.contains("\"resolved\":"), "envelope has resolved");
        assert!(json.contains("\"matches\":"), "envelope has matches");
        assert!(
            !json.contains("\"resolved\":null"),
            "exact hit: resolved is not null"
        );
    }

    #[test]
    fn query_json_envelope_no_match() {
        let (exact, matches) = resolve("xyzzy_no_such_page_12345");
        let json = render_query_json("xyzzy_no_such_page_12345", exact.as_ref(), &matches);
        assert!(
            json.contains("\"resolved\":null"),
            "no match: resolved is null"
        );
    }

    // -- --plain ANSI-free --

    #[test]
    fn overview_plain_is_ansi_free() {
        let out = render_overview(OutputFormat::Plain, &std::io::stdout());
        assert!(
            !out.contains('\x1b'),
            "overview --plain must not contain ANSI escapes"
        );
    }

    // -- anti-pattern topic map --

    #[test]
    fn anti_pattern_let_discard_maps_to_effects() {
        assert_eq!(
            topic_for_anti_pattern("let_discard_task"),
            Some("effects"),
            "let_discard_task → effects"
        );
    }

    #[test]
    fn anti_pattern_function_in_record_maps_to_state() {
        assert_eq!(
            topic_for_anti_pattern("function_in_record"),
            Some("state"),
            "function_in_record → state"
        );
    }

    #[test]
    fn anti_pattern_non_task_main_maps_to_main() {
        assert_eq!(
            topic_for_anti_pattern("non_task_main"),
            Some("main"),
            "non_task_main → main"
        );
    }

    #[test]
    fn anti_pattern_unknown_returns_none() {
        assert_eq!(
            topic_for_anti_pattern("not_a_real_key"),
            None,
            "unknown anti-pattern returns None"
        );
    }

    #[test]
    fn every_anti_pattern_topic_has_a_shipped_page() {
        let pages = all_pages();
        for &(key, topic) in ANTI_PATTERN_TOPICS {
            assert!(
                pages.iter().any(|p| p.id == topic),
                "anti-pattern `{key}` maps to topic `{topic}` which has no page"
            );
        }
    }

    // -- nudge linkage --

    #[test]
    fn nudge_topics_all_resolve_exactly() {
        for &(key, topic) in ANTI_PATTERN_TOPICS {
            let (exact, _) = resolve(topic);
            assert!(
                exact.is_some(),
                "anti-pattern `{key}` → topic `{topic}` must resolve exactly"
            );
        }
    }

    // -- diagnostic SeeExplain wiring --
    //
    // These tests verify that the three wired diagnostics produce a
    // `HelpLine::SeeExplain` carrying a topic that actually resolves to a page,
    // closing the loop between the diagnostics crate and the explain module.

    #[test]
    fn wired_diagnostic_l0141_topic_resolves() {
        use ipe_diagnostics::{Diagnostic, HelpLine, LowerError, Span};
        let d = Diagnostic::Lower {
            span: Span::DUMMY,
            msg: LowerError::LawlessEffectDiscard,
        };
        let topics: Vec<&'static str> = d
            .help()
            .into_iter()
            .filter_map(|h| {
                if let HelpLine::SeeExplain(t) = h {
                    Some(t)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(
            topics,
            ["effects"],
            "IPE-L0141 must nudge to the effects topic"
        );
        let (exact, _) = resolve("effects");
        assert!(exact.is_some(), "effects topic must resolve to a page");
    }

    #[test]
    fn wired_diagnostic_l0136_topic_resolves() {
        use ipe_diagnostics::{Diagnostic, HelpLine, LowerError, Span};
        let d = Diagnostic::Lower {
            span: Span::DUMMY,
            msg: LowerError::NonEntryMain {
                found: "a String".into(),
            },
        };
        let topics: Vec<&'static str> = d
            .help()
            .into_iter()
            .filter_map(|h| {
                if let HelpLine::SeeExplain(t) = h {
                    Some(t)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(topics, ["main"], "IPE-L0136 must nudge to the main topic");
        let (exact, _) = resolve("main");
        assert!(exact.is_some(), "main topic must resolve to a page");
    }

    #[test]
    fn wired_diagnostic_l0107_topic_resolves() {
        use ipe_diagnostics::{Diagnostic, Feature, HelpLine, LowerError, Span};
        let d = Diagnostic::Lower {
            span: Span::DUMMY,
            msg: LowerError::Unsupported(Feature::FirstClassFunctions),
        };
        let topics: Vec<&'static str> = d
            .help()
            .into_iter()
            .filter_map(|h| {
                if let HelpLine::SeeExplain(t) = h {
                    Some(t)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(topics, ["state"], "IPE-L0107 must nudge to the state topic");
        let (exact, _) = resolve("state");
        assert!(exact.is_some(), "state topic must resolve to a page");
    }

    #[test]
    fn see_explain_human_render_appends_topic_nudge() {
        // The human renderer for a wired diagnostic must contain
        // "ipe explain <topic>" in its output.
        use ipe_diagnostics::{Diagnostic, LowerError, Span, render};
        let d = Diagnostic::Lower {
            span: Span::DUMMY,
            msg: LowerError::LawlessEffectDiscard,
        };
        let rendered = render(&d, "test.ipe", "");
        assert!(
            rendered.contains("ipe explain effects"),
            "human render for IPE-L0141 must contain 'ipe explain effects'; got: {rendered:?}"
        );
    }

    #[test]
    fn see_explain_json_schema_snapshot_for_error_code_page_unchanged() {
        // The explain --json page schema has not changed: existing fields still
        // present after adding SeeExplain wiring (additive guarantee).
        let (exact, _) = resolve("IPE-L0141");
        let page = exact.expect("IPE-L0141 must resolve");
        let json = page.to_json();
        assert!(json.contains("\"kind\""), "schema has kind");
        assert!(json.contains("\"id\""), "schema has id");
        assert!(json.contains("\"IPE-L0141\""), "id is IPE-L0141");
        assert!(json.contains("\"explain_ref\""), "schema has explain_ref");
        assert!(
            json.contains("ipe explain IPE-L0141"),
            "explain_ref carries the command"
        );
    }
}
