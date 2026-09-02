//! The env-var surface: one reconciled enumeration of every `IPE_*` variable the
//! runtime reads, paired with a scan of the source tree for the read sites.
//!
//! The `IPE_*` env vars drift the same way a stdlib symbol does: a variable read
//! in code but absent from the [`ipe_docs::env_vars`] registry is an orphan read,
//! and a registry entry no code reads is a dead entry. This surface fuses the
//! registry with a one-time source scan so both drifts are named at their
//! coordinate by the coverage columns.
//!
//! An [`EnvItem`] is either a [`ipe_docs::env_vars::EnvVar`] registry entry or an
//! orphan read literal found in the source with no registry (or exclusion) home.
//! The registered column judges orphan reads; the remaining columns apply only to
//! registry entries (an orphan has no registry facets to judge).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ipe_docs::env_vars::{ENV_VARS, EXCLUDED_NAMES, EnvVar};

use crate::coverage::contract::Surface;

/// One item of the env-var surface: a registry entry, or an orphan read.
///
/// The registered column turns an orphan into a hole; the read-in-code,
/// documented, truthy-parse, and prod-safety columns judge a registry entry and
/// treat an orphan as not applicable (it has no registry row to judge).
#[derive(Clone, Debug)]
pub enum EnvItem {
    /// A variable declared in the [`ENV_VARS`] registry.
    Registered(&'static EnvVar),
    /// An `IPE_*` literal read in the source with no registry or exclusion home.
    OrphanRead(String),
}

impl EnvItem {
    /// The variable name, for either kind.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Registered(v) => v.name,
            Self::OrphanRead(name) => name.as_str(),
        }
    }
}

/// The env-var surface. Zero-sized: it reads the registry and scans the source
/// afresh on each [`Surface::all`], so the enumeration reflects the current tree.
#[derive(Clone, Copy, Debug, Default)]
pub struct EnvVarSurface;

impl Surface for EnvVarSurface {
    type Item = EnvItem;

    fn name(&self) -> &'static str {
        "env-var"
    }

    fn all(&self) -> Vec<EnvItem> {
        let registered: BTreeSet<&'static str> = ENV_VARS.iter().map(|v| v.name).collect();
        let excluded: BTreeSet<&'static str> = EXCLUDED_NAMES.iter().copied().collect();

        let mut items: Vec<EnvItem> = ENV_VARS.iter().map(EnvItem::Registered).collect();

        // Orphan reads: an `IPE_*` literal read in the tree that neither the
        // registry nor the exclusion list homes.
        let scan = SourceReads::scan();
        for name in scan.names() {
            if !registered.contains(name.as_str()) && !excluded.contains(name.as_str()) {
                items.push(EnvItem::OrphanRead(name.clone()));
            }
        }

        // Deterministic order: by name, registry entries before an orphan of the
        // same name (an orphan never shares a registered name by construction).
        items.sort_by(|a, b| {
            a.name()
                .cmp(b.name())
                .then_with(|| kind_rank(a).cmp(&kind_rank(b)))
        });
        items
    }

    fn label(item: &EnvItem) -> String {
        item.name().to_owned()
    }
}

/// Sort tie-break: a registry entry sorts before an orphan of the same name.
const fn kind_rank(item: &EnvItem) -> u8 {
    match item {
        EnvItem::Registered(_) => 0,
        EnvItem::OrphanRead(_) => 1,
    }
}

/// The result of scanning the source tree for `IPE_*` reads: the set of variable
/// names read, and the set of names whose read site hand-rolls a boolean parse.
///
/// Shared by the read-in-code column (is a registered var read anywhere?) and the
/// truthy-parse column (does a boolean var's read hand-roll a `matches!` truthy
/// table instead of a single canonical parser?). Scanned once and cloned into
/// each column so the tree is walked a single time per run.
#[derive(Clone, Debug, Default)]
pub struct SourceReads {
    inner: Arc<SourceReadsInner>,
}

#[derive(Debug, Default)]
struct SourceReadsInner {
    names: BTreeSet<String>,
    hand_rolled_truthy: bool,
}

impl SourceReads {
    /// Scan the `src/` and `tools/` trees for `IPE_*` reads.
    #[must_use]
    pub fn scan() -> Self {
        let mut names = BTreeSet::new();
        let mut hand_rolled_truthy = false;
        for root in [workspace_path("src"), workspace_path("tools")] {
            scan_tree(&root, &mut names, &mut hand_rolled_truthy);
        }
        Self {
            inner: Arc::new(SourceReadsInner {
                names,
                hand_rolled_truthy,
            }),
        }
    }

    /// Every `IPE_*` literal name read in the scanned trees.
    #[must_use]
    pub fn names(&self) -> &BTreeSet<String> {
        &self.inner.names
    }

    /// Whether any read site hand-rolls a `matches!(v, "1" | "true" | …)` truthy
    /// table rather than routing through a single canonical parser.
    #[must_use]
    pub fn has_hand_rolled_truthy(&self) -> bool {
        self.inner.hand_rolled_truthy
    }
}

/// Resolve a workspace-root-relative path from this crate's manifest directory.
fn workspace_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// Files whose `IPE_*` literals are declarations or indexes of the registry, not
/// reads of a variable. Counting them as reads would make every registered name
/// trivially "read", hiding a dead registry entry from the read-in-code column.
fn is_registry_authoring_file(path: &Path) -> bool {
    let s = path.to_string_lossy();
    s.contains("ipe-docs/src/env_vars.rs")
        || s.contains("ipe-docs/src/bin/gen_env_docs.rs")
        || s.contains("ipe-docs/src/lib.rs")
        || s.contains("ipe-cli/src/coverage/")
}

/// Walk one tree, folding every `.rs` file's `IPE_*` literals into `names` and
/// noting whether any file hand-rolls a truthy `matches!` table.
fn scan_tree(root: &Path, names: &mut BTreeSet<String>, hand_rolled_truthy: &mut bool) {
    for path in rust_files(root) {
        if is_registry_authoring_file(&path) {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        for name in extract_ipe_literals(&src) {
            names.insert(name);
        }
        if src.contains("matches!(v, \"1\" | \"true\"") {
            *hand_rolled_truthy = true;
        }
    }
}

/// Collect every `.rs` file under `root`, skipping hidden and `target` dirs.
fn rust_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if !root.exists() {
        return out;
    }
    let mut stack = vec![root.to_owned()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if !name.starts_with('.') && name != "target" {
                    stack.push(path);
                }
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                out.push(path);
            }
        }
    }
    out
}

/// Extract every `"IPE_[A-Z0-9_]+"` string literal from a Rust source string.
fn extract_ipe_literals(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = src;
    while let Some(pos) = rest.find("\"IPE_") {
        // Advance past the opening quote so the closing quote is the next one.
        let Some(after_quote) = rest.get(pos + 1..) else {
            break;
        };
        rest = after_quote;
        let end = rest.find('"').unwrap_or(rest.len());
        let candidate = rest.get(..end).unwrap_or(rest);
        if is_valid_ipe_name(candidate) {
            out.push(candidate.to_owned());
        }
        rest = rest.get(end..).unwrap_or("");
    }
    out
}

/// Whether `s` matches `IPE_[A-Z0-9_]+` with a non-empty suffix.
fn is_valid_ipe_name(s: &str) -> bool {
    let Some(suffix) = s.strip_prefix("IPE_") else {
        return false;
    };
    !suffix.is_empty()
        && suffix
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}
