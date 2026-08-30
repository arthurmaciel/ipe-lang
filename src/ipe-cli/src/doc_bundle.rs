//! Unified documentation bundle: entry index, kind-qualified resolver,
//! `[[kind:key]]` cross-reference rewriter, and fuzzy CLI search.
//!
//! Every documentation entity is a [`DocEntry`] in one of eight per-kind maps
//! inside a [`DocBundle`]. The bundle is built once per `ipe doc` invocation.
//!
//! Cross-references in Markdown bodies use `[[kind:key]]` or
//! `[[kind:key|display text]]` syntax. [`rewrite_refs`] resolves every
//! reference against the bundle and rewrites it in the target format.
//! A reference to an unknown kind or a missing key is a build error -- the
//! bundle never emits a dangling or passthrough link.
//!
//! `ipe doc <bare-word>` calls [`fuzzy_rank`], which scores every entry
//! against the query and returns the ranked candidates.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

// == DocKind ==================================================================

/// The eight documentation kinds. Each kind has its own lookup namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DocKind {
    /// A stdlib module (e.g. `Ipe.List`).
    Module,
    /// An exposed symbol from a stdlib module (e.g. `Ipe.List.map`).
    Symbol,
    /// A compiler diagnostic code (e.g. `IPE-L0107`).
    Diagnostic,
    /// A language construct page (from `docs/constructs/` or `docs/content/`).
    Construct,
    /// An idiom page (from `docs/idioms/`).
    Idiom,
    /// A topic page (from `docs/topics/`).
    Topic,
    /// A guide page (from `docs/guide/`).
    Guide,
    /// A CLI subcommand (e.g. `build`).
    Cli,
}

impl DocKind {
    /// The lowercase prefix used in `kind:key` qualified references.
    #[must_use]
    pub const fn prefix(self) -> &'static str {
        match self {
            Self::Module => "module",
            Self::Symbol => "symbol",
            Self::Diagnostic => "diagnostic",
            Self::Construct => "construct",
            Self::Idiom => "idiom",
            Self::Topic => "topic",
            Self::Guide => "guide",
            Self::Cli => "cli",
        }
    }

    /// Parse a lowercase prefix string into a [`DocKind`], or `None` for an
    /// unrecognised prefix.
    #[must_use]
    pub fn from_prefix(s: &str) -> Option<Self> {
        match s {
            "module" => Some(Self::Module),
            "symbol" => Some(Self::Symbol),
            "diagnostic" => Some(Self::Diagnostic),
            "construct" => Some(Self::Construct),
            "idiom" => Some(Self::Idiom),
            "topic" => Some(Self::Topic),
            "guide" => Some(Self::Guide),
            "cli" => Some(Self::Cli),
            _ => None,
        }
    }
}

impl fmt::Display for DocKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.prefix())
    }
}

// == DocEntry =================================================================

/// One documentation entry in the unified bundle index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocEntry {
    /// Which documentation kind this entry belongs to.
    pub kind: DocKind,
    /// The canonical lookup key within its kind (unique per kind).
    pub key: String,
    /// The human-readable title -- the first `# ` heading, a humanised slug,
    /// or an overridden `title:` from YAML front-matter.
    pub title: String,
    /// The raw Markdown body (front-matter already stripped when present).
    pub body: String,
    /// Optional sort position from front-matter `order:` -- lower values sort
    /// first. Absent means the entry sorts after all explicitly ordered entries,
    /// then alphabetically by title.
    pub order: Option<i64>,
}

// == DocBundle ================================================================

/// The unified in-memory index for one `ipe doc` invocation.
///
/// Eight per-kind maps, each keyed by the canonical key string. Built once
/// via [`DocBundle::build`]; queried via [`DocBundle::resolve_qualified`] and
/// [`fuzzy_rank`].
pub struct DocBundle {
    maps: BTreeMap<DocKind, BTreeMap<String, DocEntry>>,
}

impl DocBundle {
    /// Build the bundle from all sources.
    ///
    /// Modules, symbols, diagnostics, and CLI commands come from the
    /// compile-time index already populated by the caller. Constructs,
    /// idioms, topics, and guide pages are ingested from disk via directory
    /// convention. An absent directory yields zero entries (never an error).
    ///
    /// # Errors
    ///
    /// Returns a [`BundleError`] for any hard error: a duplicate `(kind, key)`,
    /// a malformed front-matter block, or a slug outside `[a-z0-9-]`.
    pub fn build(
        docs_root: &Path,
        modules: &[BundleSource],
        symbols: &[BundleSource],
        diagnostics: &[BundleSource],
        cli_commands: &[BundleSource],
    ) -> Result<Self, BundleError> {
        let mut maps: BTreeMap<DocKind, BTreeMap<String, DocEntry>> = BTreeMap::new();

        for src in modules {
            insert_entry(
                &mut maps,
                DocKind::Module,
                src.key.clone(),
                src.title.clone(),
                src.body.clone(),
                None,
            )?;
        }
        for src in symbols {
            insert_entry(
                &mut maps,
                DocKind::Symbol,
                src.key.clone(),
                src.title.clone(),
                src.body.clone(),
                None,
            )?;
        }
        for src in diagnostics {
            insert_entry(
                &mut maps,
                DocKind::Diagnostic,
                src.key.clone(),
                src.title.clone(),
                src.body.clone(),
                None,
            )?;
        }
        for src in cli_commands {
            insert_entry(
                &mut maps,
                DocKind::Cli,
                src.key.clone(),
                src.title.clone(),
                src.body.clone(),
                None,
            )?;
        }

        // Prefer `docs/constructs/`; fall back to `docs/content/`.
        let construct_dir = docs_root.join("constructs");
        let construct_fallback = docs_root.join("content");
        let construct_root = if construct_dir.is_dir() {
            construct_dir
        } else {
            construct_fallback
        };
        ingest_markdown_dir(&construct_root, DocKind::Construct, &mut maps)?;

        ingest_markdown_dir(&docs_root.join("idioms"), DocKind::Idiom, &mut maps)?;
        ingest_markdown_dir(&docs_root.join("topics"), DocKind::Topic, &mut maps)?;
        ingest_markdown_dir(&docs_root.join("guide"), DocKind::Guide, &mut maps)?;

        Ok(Self { maps })
    }

    /// Build an empty bundle (for tests).
    #[cfg(test)]
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            maps: BTreeMap::new(),
        }
    }

    /// Resolve a `kind:key` qualified reference.
    ///
    /// Returns `Ok(&DocEntry)` on a hit, [`BundleError::UnknownKind`] for an
    /// unrecognised prefix, and [`BundleError::UnknownKey`] for a known kind
    /// with no entry under `key`.
    ///
    /// # Errors
    ///
    /// [`BundleError::UnknownKind`] or [`BundleError::UnknownKey`].
    pub fn resolve_qualified(&self, qualified: &str) -> Result<&DocEntry, BundleError> {
        let (kind_str, key) = split_qualified(qualified)
            .ok_or_else(|| BundleError::UnknownKind(qualified.to_owned()))?;
        let kind = DocKind::from_prefix(kind_str)
            .ok_or_else(|| BundleError::UnknownKind(kind_str.to_owned()))?;
        self.maps
            .get(&kind)
            .and_then(|m| m.get(key))
            .ok_or_else(|| BundleError::UnknownKey {
                kind,
                key: key.to_owned(),
            })
    }

    /// All entries across all kinds, in kind + key order.
    pub fn all_entries(&self) -> impl Iterator<Item = &DocEntry> {
        self.maps.values().flat_map(|m| m.values())
    }

    /// All entries for a specific kind.
    pub fn entries_for_kind(&self, kind: DocKind) -> impl Iterator<Item = &DocEntry> {
        self.maps.get(&kind).into_iter().flat_map(|m| m.values())
    }

    /// Insert a single entry from a pre-computed source.
    ///
    /// # Errors
    ///
    /// [`BundleError::DuplicateKey`] when a `(kind, key)` pair already exists.
    pub fn insert(
        &mut self,
        kind: DocKind,
        key: String,
        title: String,
        body: String,
    ) -> Result<(), BundleError> {
        insert_entry(&mut self.maps, kind, key, title, body, None)
    }
}

// == BundleSource =============================================================

/// A pre-built entry to seed the bundle from a non-filesystem source (modules,
/// symbols, diagnostics, CLI commands).
pub struct BundleSource {
    /// Canonical lookup key.
    pub key: String,
    /// Human-readable title.
    pub title: String,
    /// Raw Markdown body.
    pub body: String,
}

impl BundleSource {
    /// Construct a source with key and title; no prose body.
    pub fn titled(key: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            title: title.into(),
            body: String::new(),
        }
    }

    /// Construct a source with key, title, and body.
    pub fn with_body(
        key: impl Into<String>,
        title: impl Into<String>,
        body: impl Into<String>,
    ) -> Self {
        Self {
            key: key.into(),
            title: title.into(),
            body: body.into(),
        }
    }
}

// == BundleError ==============================================================

/// A typed error produced during bundle build or reference resolution.
#[derive(Debug, PartialEq, Eq)]
pub enum BundleError {
    /// A `kind:` prefix is not one of the eight known kinds.
    UnknownKind(String),
    /// A known kind has no entry for `key`.
    UnknownKey { kind: DocKind, key: String },
    /// Two entries share the same `(kind, key)`.
    DuplicateKey {
        kind: DocKind,
        key: String,
        source: String,
    },
    /// A filename slug contains characters outside `[a-z0-9-]`.
    InvalidSlug { slug: String, source: String },
    /// A YAML front-matter block is structurally malformed.
    MalformedFrontMatter { source: String, detail: String },
    /// A cross-reference `[[kind:key]]` in a body names an unknown ref.
    UnknownRef {
        reference: String,
        source_file: String,
    },
}

impl fmt::Display for BundleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownKind(prefix) => write!(
                f,
                "unknown doc kind `{prefix}` \
                 (want: module, symbol, diagnostic, construct, idiom, topic, guide, cli)"
            ),
            Self::UnknownKey { kind, key } => {
                write!(f, "no `{kind}` entry for key `{key}`")
            }
            Self::DuplicateKey { kind, key, source } => {
                write!(f, "duplicate doc entry ({kind}:{key}) in `{source}`")
            }
            Self::InvalidSlug { slug, source } => write!(
                f,
                "filename slug `{slug}` in `{source}` contains characters outside [a-z0-9-]"
            ),
            Self::MalformedFrontMatter { source, detail } => {
                write!(f, "malformed front-matter in `{source}`: {detail}")
            }
            Self::UnknownRef {
                reference,
                source_file,
            } => write!(
                f,
                "unresolved cross-reference `[[{reference}]]` in `{source_file}`"
            ),
        }
    }
}

// == Ingestion ================================================================

/// Scan `dir` for `.md` files and insert each as a `kind` entry.
///
/// An absent or non-directory `dir` yields zero entries (never an error).
/// A malformed slug, duplicate key, or bad front-matter is a hard error.
fn ingest_markdown_dir(
    dir: &Path,
    kind: DocKind,
    maps: &mut BTreeMap<DocKind, BTreeMap<String, DocEntry>>,
) -> Result<(), BundleError> {
    if !dir.is_dir() {
        return Ok(());
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    let mut paths: Vec<std::path::PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("md"))
        })
        .collect();
    paths.sort();

    for path in paths {
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        let slug = file_name.strip_suffix(".md").unwrap_or(file_name);
        let source_label = path.display().to_string();

        if !is_valid_slug(slug) {
            // Skip housekeeping files (README.md, etc.) that are not bundle entries.
            continue;
        }

        let raw =
            std::fs::read_to_string(&path).map_err(|_| BundleError::MalformedFrontMatter {
                source: source_label.clone(),
                detail: "could not read file".to_owned(),
            })?;

        let ParsedMarkdown {
            key,
            title,
            body,
            order,
        } = parse_markdown_file(slug, &raw, &source_label)?;
        insert_entry(maps, kind, key, title, body, order)?;
    }
    Ok(())
}

/// Parse one markdown file's optional YAML front-matter and extract key, title,
/// and body.
///
/// Front-matter is `---\n...\n---\n` at the very start of the file. Only
/// `key:` and `title:` fields are read; other fields are ignored. When
/// front-matter is absent: key = slug, title = first `# ` heading (fallback:
/// humanised slug), body = full file text.
fn parse_markdown_file(
    slug: &str,
    raw: &str,
    source_label: &str,
) -> Result<ParsedMarkdown, BundleError> {
    let (front_matter_str, body_start) = if raw.starts_with("---\n") || raw.starts_with("---\r\n") {
        let after_open = raw.find('\n').map_or(raw.len(), |i| i + 1);
        raw[after_open..].find("\n---\n").map_or((None, 0), |rel| {
            let fm_end = after_open + rel;
            // Skip the full closing `\n---\n` (5 bytes) to land on the content.
            let close_len = "\n---\n".len();
            (Some(&raw[after_open..fm_end]), after_open + rel + close_len)
        })
    } else {
        (None, 0)
    };

    let body = raw[body_start..].to_owned();

    let mut fm_key: Option<String> = None;
    let mut fm_title: Option<String> = None;
    let mut fm_order: Option<i64> = None;
    if let Some(fm) = front_matter_str {
        for line in fm.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Some(colon_pos) = line.find(':') else {
                return Err(BundleError::MalformedFrontMatter {
                    source: source_label.to_owned(),
                    detail: format!("line `{line}` has no colon separator"),
                });
            };
            let field = line[..colon_pos].trim();
            let value = line[colon_pos + 1..].trim().to_owned();
            match field {
                "key" => fm_key = Some(value),
                "title" => fm_title = Some(value),
                "order" => {
                    fm_order = value.parse::<i64>().ok();
                }
                _ => {}
            }
        }
    }

    let key = match fm_key {
        Some(k) if !k.is_empty() => {
            if !is_valid_slug(&k) {
                return Err(BundleError::InvalidSlug {
                    slug: k,
                    source: source_label.to_owned(),
                });
            }
            k
        }
        _ => slug.to_owned(),
    };

    let title = fm_title
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| extract_h1_title(&body).unwrap_or_else(|| humanise_slug(&key)));

    Ok(ParsedMarkdown {
        key,
        title,
        body,
        order: fm_order,
    })
}

struct ParsedMarkdown {
    key: String,
    title: String,
    body: String,
    order: Option<i64>,
}

/// Return the text of the first `# ` heading in `body`, or `None` when absent.
fn extract_h1_title(body: &str) -> Option<String> {
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("# ") {
            let title = rest.trim();
            if !title.is_empty() {
                return Some(title.to_owned());
            }
        }
    }
    None
}

/// Convert a slug like `getting-started` to a human-readable title.
fn humanise_slug(slug: &str) -> String {
    let mut out = String::new();
    for (i, word) in slug.split('-').enumerate() {
        if word.is_empty() {
            continue;
        }
        if i == 0 {
            let mut chars = word.chars();
            if let Some(first) = chars.next() {
                out.extend(first.to_uppercase());
                out.push_str(chars.as_str());
            }
        } else {
            out.push(' ');
            out.push_str(word);
        }
    }
    out
}

/// Return `true` when `slug` consists only of `[a-z0-9-]` and is non-empty.
fn is_valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Insert an entry into the per-kind map, returning a hard error on a duplicate.
fn insert_entry(
    maps: &mut BTreeMap<DocKind, BTreeMap<String, DocEntry>>,
    kind: DocKind,
    key: String,
    title: String,
    body: String,
    order: Option<i64>,
) -> Result<(), BundleError> {
    let map = maps.entry(kind).or_default();
    if map.contains_key(&key) {
        return Err(BundleError::DuplicateKey {
            kind,
            key,
            source: String::from("<in-memory>"),
        });
    }
    map.insert(
        key.clone(),
        DocEntry {
            kind,
            key,
            title,
            body,
            order,
        },
    );
    Ok(())
}

// == Qualified key parsing ====================================================

/// Split `"kind:rest"` into `("kind", "rest")`, or `None` when no `:` is present.
#[must_use]
pub fn split_qualified(s: &str) -> Option<(&str, &str)> {
    s.find(':').map(|pos| (&s[..pos], &s[pos + 1..]))
}

/// Return `true` when `s` looks like a qualified `kind:key` reference.
#[must_use]
pub fn is_qualified(s: &str) -> bool {
    split_qualified(s)
        .and_then(|(prefix, _)| DocKind::from_prefix(prefix))
        .is_some()
}

// == Cross-reference rewriting ================================================

/// The output format for `[[kind:key]]` rewriting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefTarget {
    /// `<a href="{kind}/{key}.html">disp</a>`
    Html,
    /// `[disp]({kind}/{key}.md)`
    Markdown,
    /// A structured JSON object embedded in the body string.
    Json,
    /// `<a href="/{kind}/{key}">disp</a>` (serve route).
    Serve,
    /// `disp (ipe doc {kind}:{key})` (terminal plain text).
    Terminal,
}

/// Scan `body` for `[[kind:key]]` and `[[kind:key|display]]` cross-references,
/// resolve each against `bundle`, and rewrite in `target` format.
///
/// `source_file` appears in error messages only.
///
/// # Errors
///
/// [`BundleError::UnknownRef`] when any reference cannot be resolved. Never
/// emits a dangling or passthrough link.
pub fn rewrite_refs(
    body: &str,
    bundle: &DocBundle,
    target: RefTarget,
    source_file: &str,
) -> Result<String, BundleError> {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;

    loop {
        match rest.find("[[") {
            None => {
                out.push_str(rest);
                break;
            }
            Some(open_pos) => {
                out.push_str(&rest[..open_pos]);
                rest = &rest[open_pos + 2..];

                let Some(close_pos) = rest.find("]]") else {
                    // No closing `]]`: treat `[[` as literal text.
                    out.push_str("[[");
                    continue;
                };

                let inner = &rest[..close_pos];
                rest = &rest[close_pos + 2..];

                let (qualified, display_override) = inner.find('|').map_or((inner, None), |pipe| {
                    (&inner[..pipe], Some(inner[pipe + 1..].trim()))
                });

                let (kind_str, key) =
                    split_qualified(qualified).ok_or_else(|| BundleError::UnknownRef {
                        reference: inner.to_owned(),
                        source_file: source_file.to_owned(),
                    })?;
                let kind =
                    DocKind::from_prefix(kind_str).ok_or_else(|| BundleError::UnknownRef {
                        reference: inner.to_owned(),
                        source_file: source_file.to_owned(),
                    })?;

                let entry = bundle
                    .maps
                    .get(&kind)
                    .and_then(|m| m.get(key))
                    .ok_or_else(|| BundleError::UnknownRef {
                        reference: inner.to_owned(),
                        source_file: source_file.to_owned(),
                    })?;

                let display = display_override
                    .filter(|d| !d.is_empty())
                    .unwrap_or(entry.title.as_str());

                out.push_str(&format_ref(kind, key, display, target));
            }
        }
    }

    Ok(out)
}

/// Produce the format-aware rewriting of one resolved cross-reference.
fn format_ref(kind: DocKind, key: &str, display: &str, target: RefTarget) -> String {
    match target {
        RefTarget::Html => format!(
            "<a href=\"{}/{}.html\">{}</a>",
            kind.prefix(),
            key,
            html_escape_ref(display)
        ),
        RefTarget::Markdown => format!("[{}]({}/{}.md)", display, kind.prefix(), key),
        RefTarget::Json => format!(
            "{{\"ref\":{{\"kind\":\"{}\",\"key\":\"{}\"}},\"text\":\"{}\"}}",
            kind.prefix(),
            json_escape_ref(key),
            json_escape_ref(display)
        ),
        RefTarget::Serve => format!(
            "<a href=\"/{}/{}\">{}</a>",
            kind.prefix(),
            key,
            html_escape_ref(display)
        ),
        RefTarget::Terminal => format!("{display} (ipe doc {}:{})", kind.prefix(), key),
    }
}

/// Minimal HTML escaping for ref display text.
fn html_escape_ref(s: &str) -> String {
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

/// Minimal JSON string escaping: backslash and double-quote.
fn json_escape_ref(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

// == Fuzzy search =============================================================

/// A ranked candidate from a fuzzy search.
#[derive(Debug, Clone)]
pub struct FuzzyMatch<'a> {
    /// The matched entry.
    pub entry: &'a DocEntry,
    /// Score: higher is a better match. The exact value is an implementation
    /// detail; only the ordering is meaningful to callers.
    pub score: u32,
}

/// Rank all entries in `bundle` against `query` by a case-insensitive fuzzy
/// score over key and title. Returns candidates with score > 0, ordered
/// highest score first.
///
/// No panics and no raw indexing. An empty bundle returns an empty vec.
#[must_use]
pub fn fuzzy_rank<'a>(bundle: &'a DocBundle, query: &str) -> Vec<FuzzyMatch<'a>> {
    let q = query.to_ascii_lowercase();
    let mut results: Vec<FuzzyMatch<'a>> = bundle
        .all_entries()
        .filter_map(|entry| {
            let score = score_entry(entry, &q);
            if score > 0 {
                Some(FuzzyMatch { entry, score })
            } else {
                None
            }
        })
        .collect();
    results.sort_by_key(|m| std::cmp::Reverse(m.score));
    results
}

/// Compute the fuzzy score of a query against one entry.
fn score_entry(entry: &DocEntry, query_lower: &str) -> u32 {
    let key_lower = entry.key.to_ascii_lowercase();
    let title_lower = entry.title.to_ascii_lowercase();
    if key_lower == query_lower {
        return 1000;
    }
    if key_lower.starts_with(query_lower) {
        return 800;
    }
    if title_lower == query_lower {
        return 750;
    }
    if title_lower.starts_with(query_lower) {
        return 600;
    }
    subsequence_score(query_lower, &key_lower).max(subsequence_score(query_lower, &title_lower))
}

/// Greedy subsequence match: count how many characters of `query` appear in
/// order in `target`, returning a score proportional to the fraction matched.
/// Returns 0 when no character matches.
fn subsequence_score(query: &str, target: &str) -> u32 {
    let mut t_iter = target.chars();
    let mut matched: u32 = 0;
    let mut total: u32 = 0;
    for qc in query.chars() {
        total = total.saturating_add(1);
        if t_iter.any(|tc| tc == qc) {
            matched += 1;
        }
    }
    if matched == 0 || total == 0 {
        return 0;
    }
    matched * 400 / total
}

// == Tests ====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tempdir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ipe-doc-bundle-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // -- Ingestion ------------------------------------------------------------

    #[test]
    fn ingest_topic_without_front_matter() {
        let tmp = tempdir("ingest_no_fm");
        fs::write(tmp.join("foo.md"), "# Foo topic\n\nSome prose.\n").unwrap();
        let mut maps = BTreeMap::new();
        ingest_markdown_dir(&tmp, DocKind::Topic, &mut maps).expect("ingest");
        let map = maps.get(&DocKind::Topic).expect("topic map");
        let entry = map.get("foo").expect("foo entry");
        assert_eq!(entry.key, "foo");
        assert_eq!(entry.title, "Foo topic");
        assert!(entry.body.contains("Some prose."));
    }

    #[test]
    fn ingest_topic_with_front_matter_key_and_title() {
        let tmp = tempdir("ingest_fm");
        fs::write(
            tmp.join("bar.md"),
            "---\nkey: overridden\ntitle: Overridden Title\n---\n# Bar\n\nBody text.\n",
        )
        .unwrap();
        let mut maps = BTreeMap::new();
        ingest_markdown_dir(&tmp, DocKind::Topic, &mut maps).expect("ingest");
        let map = maps.get(&DocKind::Topic).expect("topic map");
        assert!(map.contains_key("overridden"), "key overridden: {map:?}");
        assert_eq!(map["overridden"].title, "Overridden Title");
    }

    #[test]
    fn ingest_absent_dir_yields_zero_entries() {
        let tmp = tempdir("ingest_absent");
        let nonexistent = tmp.join("doesnotexist");
        let mut maps = BTreeMap::new();
        ingest_markdown_dir(&nonexistent, DocKind::Topic, &mut maps).expect("absent dir is ok");
        assert!(maps.is_empty(), "no entries from absent dir");
    }

    #[test]
    fn ingest_empty_dir_yields_zero_entries() {
        let tmp = tempdir("ingest_empty");
        let mut maps = BTreeMap::new();
        ingest_markdown_dir(&tmp, DocKind::Topic, &mut maps).expect("empty dir is ok");
        assert!(maps.is_empty(), "no entries from empty dir");
    }

    #[test]
    fn ingest_duplicate_key_is_error() {
        let tmp = tempdir("ingest_dup");
        fs::write(tmp.join("alpha.md"), "# Alpha\n").unwrap();
        fs::write(
            tmp.join("beta.md"),
            "---\nkey: alpha\ntitle: Beta as alpha\n---\n# Beta\n",
        )
        .unwrap();
        let mut maps = BTreeMap::new();
        let err = ingest_markdown_dir(&tmp, DocKind::Topic, &mut maps)
            .expect_err("duplicate key must error");
        assert!(
            matches!(err, BundleError::DuplicateKey { .. }),
            "expected DuplicateKey: {err}"
        );
    }

    // -- Resolution -----------------------------------------------------------

    #[test]
    fn resolve_qualified_hit() {
        let mut bundle = DocBundle::empty();
        bundle
            .insert(
                DocKind::Topic,
                "foo".to_owned(),
                "Foo".to_owned(),
                String::new(),
            )
            .unwrap();
        let entry = bundle.resolve_qualified("topic:foo").expect("hit");
        assert_eq!(entry.key, "foo");
    }

    #[test]
    fn resolve_qualified_unknown_key_is_error() {
        let bundle = DocBundle::empty();
        let err = bundle
            .resolve_qualified("topic:nope")
            .expect_err("unknown key");
        assert!(
            matches!(
                err,
                BundleError::UnknownKey {
                    kind: DocKind::Topic,
                    ..
                }
            ),
            "{err}"
        );
    }

    #[test]
    fn resolve_qualified_unknown_kind_is_error() {
        let bundle = DocBundle::empty();
        let err = bundle
            .resolve_qualified("bogus:x")
            .expect_err("unknown kind");
        assert!(matches!(err, BundleError::UnknownKind(_)), "{err}");
    }

    #[test]
    fn resolve_qualified_unqualified_is_unknown_kind() {
        let bundle = DocBundle::empty();
        let err = bundle.resolve_qualified("nocolon").expect_err("no colon");
        assert!(matches!(err, BundleError::UnknownKind(_)), "{err}");
    }

    // -- Ref rewriting --------------------------------------------------------

    fn bundle_with_module_entry() -> DocBundle {
        let mut b = DocBundle::empty();
        b.insert(
            DocKind::Module,
            "Ipe.Db.Store".to_owned(),
            "the store".to_owned(),
            String::new(),
        )
        .unwrap();
        b
    }

    #[test]
    fn rewrite_html_uses_href_with_kind_prefix() {
        let bundle = bundle_with_module_entry();
        let out = rewrite_refs(
            "See [[module:Ipe.Db.Store|the store]].",
            &bundle,
            RefTarget::Html,
            "test",
        )
        .expect("rewrite");
        assert_eq!(
            out,
            "See <a href=\"module/Ipe.Db.Store.html\">the store</a>."
        );
    }

    #[test]
    fn rewrite_markdown_uses_md_link() {
        let bundle = bundle_with_module_entry();
        let out = rewrite_refs(
            "See [[module:Ipe.Db.Store|the store]].",
            &bundle,
            RefTarget::Markdown,
            "test",
        )
        .expect("rewrite");
        assert_eq!(out, "See [the store](module/Ipe.Db.Store.md).");
    }

    #[test]
    fn rewrite_json_emits_structured_object() {
        let bundle = bundle_with_module_entry();
        let out = rewrite_refs(
            "[[module:Ipe.Db.Store|the store]]",
            &bundle,
            RefTarget::Json,
            "test",
        )
        .expect("rewrite");
        assert_eq!(
            out,
            "{\"ref\":{\"kind\":\"module\",\"key\":\"Ipe.Db.Store\"},\"text\":\"the store\"}"
        );
    }

    #[test]
    fn rewrite_serve_uses_route_href() {
        let bundle = bundle_with_module_entry();
        let out = rewrite_refs(
            "[[module:Ipe.Db.Store|the store]]",
            &bundle,
            RefTarget::Serve,
            "test",
        )
        .expect("rewrite");
        assert_eq!(out, "<a href=\"/module/Ipe.Db.Store\">the store</a>");
    }

    #[test]
    fn rewrite_terminal_uses_ipe_doc_hint() {
        let bundle = bundle_with_module_entry();
        let out = rewrite_refs(
            "[[module:Ipe.Db.Store|the store]]",
            &bundle,
            RefTarget::Terminal,
            "test",
        )
        .expect("rewrite");
        assert_eq!(out, "the store (ipe doc module:Ipe.Db.Store)");
    }

    #[test]
    fn rewrite_default_display_uses_entry_title() {
        let bundle = bundle_with_module_entry();
        let out = rewrite_refs(
            "[[module:Ipe.Db.Store]]",
            &bundle,
            RefTarget::Terminal,
            "test",
        )
        .expect("rewrite");
        assert_eq!(out, "the store (ipe doc module:Ipe.Db.Store)");
    }

    #[test]
    fn rewrite_unknown_ref_is_build_error() {
        let bundle = DocBundle::empty();
        let err = rewrite_refs(
            "[[topic:does-not-exist]]",
            &bundle,
            RefTarget::Html,
            "myfile.md",
        )
        .expect_err("error");
        assert!(
            matches!(err, BundleError::UnknownRef { .. }),
            "expected UnknownRef: {err}"
        );
        let msg = err.to_string();
        assert!(msg.contains("topic:does-not-exist"), "{msg}");
        assert!(msg.contains("myfile.md"), "{msg}");
    }

    // -- Fuzzy search ---------------------------------------------------------

    fn bundle_with_select_entries() -> DocBundle {
        let mut b = DocBundle::empty();
        b.insert(
            DocKind::Construct,
            "select".to_owned(),
            "Select expression".to_owned(),
            String::new(),
        )
        .unwrap();
        b.insert(
            DocKind::Construct,
            "case".to_owned(),
            "Case expression".to_owned(),
            String::new(),
        )
        .unwrap();
        b.insert(
            DocKind::Guide,
            "getting-started".to_owned(),
            "Getting started".to_owned(),
            String::new(),
        )
        .unwrap();
        b
    }

    #[test]
    fn fuzzy_rank_typo_suggests_correct_key() {
        let bundle = bundle_with_select_entries();
        let results = fuzzy_rank(&bundle, "slect");
        assert!(!results.is_empty(), "should find suggestions for 'slect'");
        let top = results.first().expect("at least one result");
        assert_eq!(
            top.entry.key,
            "select",
            "top result is 'select': {:?}",
            results.iter().map(|r| &r.entry.key).collect::<Vec<_>>()
        );
    }

    #[test]
    fn fuzzy_rank_exact_key_is_top() {
        let bundle = bundle_with_select_entries();
        let results = fuzzy_rank(&bundle, "case");
        assert!(!results.is_empty());
        let top = results.first().expect("at least one result");
        assert_eq!(top.entry.key, "case");
        assert_eq!(top.score, 1000);
    }

    #[test]
    fn fuzzy_rank_no_match_returns_empty() {
        let bundle = bundle_with_select_entries();
        let results = fuzzy_rank(&bundle, "zzzzzzzzzzz");
        assert!(results.is_empty(), "no match for nonsense query");
    }

    #[test]
    fn fuzzy_rank_ambiguous_returns_multiple() {
        let bundle = bundle_with_select_entries();
        let results = fuzzy_rank(&bundle, "e");
        assert!(
            results.len() > 1,
            "ambiguous query returns multiple: {:?}",
            results.iter().map(|r| &r.entry.key).collect::<Vec<_>>()
        );
    }

    // -- No-panic witness -----------------------------------------------------

    #[test]
    fn non_slug_filename_is_skipped_not_an_error() {
        // Files whose stem is not a valid slug (e.g. README.md, CHANGELOG.md)
        // are housekeeping files, not bundle entries; they are skipped silently.
        let tmp = tempdir("ingest_non_slug");
        fs::write(tmp.join("README.md"), "# README\n").unwrap();
        fs::write(tmp.join("valid-entry.md"), "# Valid\n").unwrap();
        let mut maps = BTreeMap::new();
        let result = ingest_markdown_dir(&tmp, DocKind::Topic, &mut maps);
        assert!(
            result.is_ok(),
            "non-slug filename must be skipped, not an error"
        );
        assert!(
            maps.get(&DocKind::Topic)
                .is_some_and(|m| m.contains_key("valid-entry")),
            "valid entry is still ingested"
        );
        assert!(
            !maps
                .get(&DocKind::Topic)
                .is_some_and(|m| m.contains_key("README")),
            "README is not ingested"
        );
    }

    #[test]
    fn invalid_fm_key_returns_typed_error() {
        // A file with a valid slug filename but an explicit bad `key:` in
        // front-matter must still return an error.
        let tmp = tempdir("ingest_bad_fm_key");
        fs::write(
            tmp.join("valid.md"),
            "---\nkey: Bad Key!\ntitle: t\n---\nbody\n",
        )
        .unwrap();
        let mut maps = BTreeMap::new();
        let result = ingest_markdown_dir(&tmp, DocKind::Topic, &mut maps);
        assert!(result.is_err(), "invalid fm key must return Err");
        assert!(
            matches!(result, Err(BundleError::InvalidSlug { .. })),
            "expected InvalidSlug"
        );
    }

    #[test]
    fn missing_key_returns_typed_error() {
        let bundle = DocBundle::empty();
        let result = bundle.resolve_qualified("topic:missing");
        assert!(result.is_err(), "missing key must return Err");
        assert!(
            matches!(result, Err(BundleError::UnknownKey { .. })),
            "expected UnknownKey"
        );
    }
}
