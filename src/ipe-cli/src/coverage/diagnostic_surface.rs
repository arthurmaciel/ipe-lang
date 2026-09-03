//! The diagnostic surface: one row per `IPE-XXXX` code in [`ALL_CODES`], judged
//! on three aspect columns.
//!
//! Items are [`ipe_diagnostics::code::Code`] values drawn directly from the
//! single-source-of-truth [`ALL_CODES`] slice, so the surface stays in sync with
//! the taxonomy automatically — no secondary registry to maintain.
//!
//! Columns:
//!
//! - **`has-conforming-explain-page`** — the code's explain page exists (guaranteed
//!   by `include_str!` at compile time) AND its first line is exactly
//!   `# <CODE>: <title>` AND the body carries at least three ` ```ipe ` fences.
//!   Reuses the same invariants the in-crate `every_code_has_a_conforming_explain_page`
//!   test asserts, so this column is a standing per-code cell for the same check.
//!
//! - **`documented`** — the explain page exists and is non-empty. Because
//!   `include_str!` makes a missing file a build error, `documented` is structurally
//!   always `Ok` for any code in the taxonomy; the column still appears so future
//!   stubs are visible on the surface rather than hidden.
//!
//! - **`refusal-tested`** — a standing test in the test suite drives this code to
//!   fire: the wire string (e.g. `"IPE-N0028"`) appears as a literal in a source
//!   file under `src/` or `tools/` that is recognisably a test context. A code with
//!   no such test is a Hole — a rejection one edit can delete unnoticed. An
//!   [`ALLOWLIST`] in `diagnostic_coverage_matrix.rs` records known, tracked gaps.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ipe_diagnostics::{ALL_CODES, Code, explain_page, title};

use crate::coverage::contract::{AspectCheck, Cell, Surface};

// ── surface ───────────────────────────────────────────────────────────────────

/// Zero-sized: it enumerates `ALL_CODES` afresh on each [`Surface::all`].
#[derive(Clone, Copy, Debug, Default)]
pub struct DiagnosticSurface;

impl Surface for DiagnosticSurface {
    type Item = Code;

    fn name(&self) -> &'static str {
        "diagnostic"
    }

    fn all(&self) -> Vec<Code> {
        ALL_CODES.to_vec()
    }

    fn label(item: &Code) -> String {
        item.as_str().to_owned()
    }
}

// ── columns ───────────────────────────────────────────────────────────────────

/// Column **`has-conforming-explain-page`**: the explain page exists, its first
/// line is `# <CODE>: <title()>`, and it carries at least three ` ```ipe ` fences.
///
/// The `include_str!` in [`ipe_diagnostics::code::explain_page`] already makes a
/// missing file a build error; this column asserts the structural invariants on
/// top of bare existence so a skeletal or misnamed page is a surface Hole.
pub struct ExplainPageColumn;

impl AspectCheck<Code> for ExplainPageColumn {
    fn name(&self) -> &'static str {
        "has-conforming-explain-page"
    }

    fn check(&self, code: &Code) -> Cell {
        let Some(page) = explain_page(*code) else {
            // Structurally unreachable: `include_str!` fails the build first.
            return Cell::Hole(format!(
                "{} has no explain page (include_str! should have caught this at build time)",
                code.as_str()
            ));
        };
        let expected_header = format!("# {}: {}", code.as_str(), title(*code));
        let first_line = page.lines().next().unwrap_or("");
        if first_line != expected_header {
            return Cell::Hole(format!(
                "{} explain page line 1 is `{}`, expected `{}`",
                code.as_str(),
                first_line,
                expected_header,
            ));
        }
        let fence_count = page.matches("```ipe").count();
        if fence_count < 3 {
            return Cell::Hole(format!(
                "{} explain page has {} ```ipe fence(s), need >= 3",
                code.as_str(),
                fence_count,
            ));
        }
        Cell::Ok
    }
}

/// Column **`documented`**: the explain page is present and non-empty.
///
/// Because `include_str!` makes a missing page a compile error, this column is
/// structurally always `Ok` for any code reachable through `ALL_CODES`. It appears
/// so a stub page (empty after the header) is a visible Hole rather than a silent
/// gap.
pub struct DocumentedColumn;

impl AspectCheck<Code> for DocumentedColumn {
    fn name(&self) -> &'static str {
        "documented"
    }

    fn check(&self, code: &Code) -> Cell {
        match explain_page(*code) {
            None => Cell::Hole(format!("{} has no explain page", code.as_str())),
            Some(page) if page.trim().is_empty() => {
                Cell::Hole(format!("{} explain page is empty", code.as_str()))
            }
            Some(_) => Cell::Ok,
        }
    }
}

/// Column **`refusal-tested`**: a standing test drives this code to fire.
///
/// The scan looks for the wire string (e.g. `"IPE-N0028"`) as a quoted literal
/// inside a `.rs` file under `src/` or `tools/` that is not a registry or
/// surface authoring file. A match means a test asserts the rejection; no match
/// is a Hole — a rejection path one edit away from vanishing unnoticed.
pub struct RefusalTestedColumn {
    tested: TestedCodes,
}

impl RefusalTestedColumn {
    #[must_use]
    pub fn new() -> Self {
        Self {
            tested: TestedCodes::scan(),
        }
    }
}

impl Default for RefusalTestedColumn {
    fn default() -> Self {
        Self::new()
    }
}

impl AspectCheck<Code> for RefusalTestedColumn {
    fn name(&self) -> &'static str {
        "refusal-tested"
    }

    fn check(&self, code: &Code) -> Cell {
        if self.tested.contains(code.as_str()) {
            Cell::Ok
        } else {
            Cell::Hole(format!(
                "{} ({}) has no standing test that drives it to fire — \
                 add a negative-suite case or record it in the allowlist with a tracking reason",
                code.as_str(),
                title(*code),
            ))
        }
    }
}

// ── test-scan ─────────────────────────────────────────────────────────────────

/// The set of diagnostic wire strings that appear as quoted literals in test
/// files (`"IPE-X####"`).
///
/// Scanned once from the `src/` and `tools/` trees; cloned cheaply via the inner
/// [`Arc`] when columns are registered in a surface runner.
#[derive(Clone, Debug, Default)]
pub struct TestedCodes {
    inner: Arc<BTreeSet<String>>,
}

impl TestedCodes {
    /// Scan the workspace for test files asserting diagnostic codes.
    #[must_use]
    pub fn scan() -> Self {
        let mut found: BTreeSet<String> = BTreeSet::new();
        for root in [workspace_path("src"), workspace_path("tools")] {
            scan_tree(&root, &mut found);
        }
        Self {
            inner: Arc::new(found),
        }
    }

    /// Whether the wire string (e.g. `"IPE-N0028"`) was found in a test file.
    #[must_use]
    pub fn contains(&self, wire: &str) -> bool {
        self.inner.contains(wire)
    }
}

/// Resolve a workspace-root-relative path from this crate's manifest directory.
fn workspace_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// Files that declare the code taxonomy or author the surface itself — a code
/// that appears only here is not a tested refusal.
fn is_authoring_file(path: &Path) -> bool {
    let s = path.to_string_lossy().replace('\\', "/");
    s.contains("diagnostics/src/code.rs")
        || s.contains("ipe-cli/src/coverage/diagnostic_surface.rs")
        || s.contains("ipe-cli/tests/diagnostic_coverage_matrix.rs")
}

/// Walk `root`, collecting every `"IPE-X####"` literal from `.rs` test files.
fn scan_tree(root: &Path, found: &mut BTreeSet<String>) {
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
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs")
                && !is_authoring_file(&path)
            {
                let Ok(src) = std::fs::read_to_string(&path) else {
                    continue;
                };
                extract_wire_strings(&src, found);
            }
        }
    }
}

/// Extract every `"IPE-[A-Z][0-9]{4}"` quoted literal from a Rust source string.
fn extract_wire_strings(src: &str, found: &mut BTreeSet<String>) {
    let needle = "\"IPE-";
    let mut rest = src;
    while let Some(pos) = rest.find(needle) {
        // Advance past the opening quote.
        let Some(after_quote) = rest.get(pos + 1..) else {
            break;
        };
        rest = after_quote;
        let end = rest.find('"').unwrap_or(rest.len());
        let candidate = rest.get(..end).unwrap_or(rest);
        if is_valid_wire(candidate) {
            found.insert(candidate.to_owned());
        }
        rest = rest.get(end..).unwrap_or("");
    }
}

/// Whether `s` matches `IPE-[A-Z][0-9]{4}` exactly.
fn is_valid_wire(s: &str) -> bool {
    let Some(after_prefix) = s.strip_prefix("IPE-") else {
        return false;
    };
    let mut chars = after_prefix.chars();
    let Some(family) = chars.next() else {
        return false;
    };
    if !family.is_ascii_uppercase() {
        return false;
    }
    let digits: String = chars.collect();
    digits.len() == 4 && digits.chars().all(|c| c.is_ascii_digit())
}
