//! Multi-module project manifest parsing, module discovery, import graph, and
//! topological sort.
//!
//! The `sky.toml` format is a minimal subset: only `[project]` and `name` / the
//! source root (`src/`) are significant for M7. No external TOML crate is used —
//! the relevant structure is simple enough for a line-by-line parser.
//!
//! # Discovery
//!
//! Given a project directory (containing `sky.toml`), the driver:
//!
//! 1. Reads `sky.toml` to obtain the project name and confirm the source root
//!    exists (`src/` by default).
//! 2. Walks `src/` recursively, collecting every `*.sky` file.
//! 3. Maps each file path to a module name by:
//!    - Stripping the `src/` prefix and `.sky` suffix.
//!    - Splitting on the OS path separator to obtain segment strings.
//!    - Rejecting any segment that is not a valid Sky module segment
//!      (`[A-Z][A-Za-z0-9_]*`).
//! 4. The entry module is always `Main` (`src/Main.sky`).
//!
//! # Topological sort
//!
//! A three-colour DFS (White / Gray / Black) produces a stable dep-first
//! ordering of all discovered modules. A Gray → Gray back-edge is an import
//! cycle ([`CycleError`]).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use crate::CliError;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// The parsed, validated content of a `sky.toml` manifest.
#[derive(Clone, Debug)]
pub struct ProjectManifest {
    /// The project name (from `[project] name = "…"`).
    pub name: String,
    /// Absolute path to the project root directory (where `sky.toml` lives).
    pub root: PathBuf,
    /// Absolute path to the source root (`<root>/src` by default).
    pub src_root: PathBuf,
}

/// A discovered Sky source file with its resolved module path.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DiscoveredModule {
    /// Absolute path to the `.sky` source file.
    pub path: PathBuf,
    /// Module path segments, e.g. `Lib/Utils.sky` → `["Lib", "Utils"]`.
    pub module_path: Vec<String>,
}

/// An import edge: the importing module's path and the imported module's path.
#[derive(Clone, Debug)]
pub struct ImportEdge {
    pub from: Vec<String>,
    pub to: Vec<String>,
}

/// An import cycle detected during topological sort.
#[derive(Clone, Debug)]
pub struct CycleError {
    /// The cycle, in the order the DFS discovered it. The first and last element
    /// are the same module (the back-edge target).
    pub path: Vec<String>,
}

// ---------------------------------------------------------------------------
// Manifest parsing
// ---------------------------------------------------------------------------

/// Parse a `sky.toml` file and return a [`ProjectManifest`].
///
/// The format recognised:
/// ```toml
/// [project]
/// name = "my-app"
/// ```
/// Lines that start with `#` are comments and are ignored. All other lines
/// outside `[project]` are ignored (forward-compatible). Within `[project]`,
/// only `name` is extracted; other keys are ignored.
///
/// # Errors
/// [`CliError::Io`] if the file cannot be read; [`CliError::Usage`] if the
/// manifest is malformed or the `src/` directory does not exist.
pub fn parse_manifest(manifest_path: &Path) -> Result<ProjectManifest, CliError> {
    let root = manifest_path
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);

    let text = fs::read_to_string(manifest_path).map_err(|e| CliError::Io {
        path: manifest_path.to_path_buf(),
        source: e,
    })?;

    // `sky.toml` schema: `name` may sit at the top level (Sky's own examples) or
    // under `[project]`; the source root comes from `[source] root = "…"`,
    // defaulting to `src`. `section` is the empty string at the top level.
    let mut section = "";
    let mut name: Option<String> = None;
    let mut src_rel: Option<String> = None;

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            section = if line == "[project]" {
                "[project]"
            } else if line == "[source]" {
                "[source]"
            } else {
                "other"
            };
            continue;
        }
        let Some((key, val)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let val = val.trim().trim_matches('"');
        match (section, key) {
            ("" | "[project]", "name") => name = Some(val.to_owned()),
            ("[source]", "root") => src_rel = Some(val.to_owned()),
            _ => {}
        }
    }

    let name = name.ok_or(CliError::Usage("sky.toml: missing a `name = \"…\"` entry"))?;

    let src_root = root.join(src_rel.as_deref().unwrap_or("src"));
    if !src_root.is_dir() {
        return Err(CliError::Usage(
            "sky.toml: the source root directory does not exist",
        ));
    }

    Ok(ProjectManifest {
        name,
        root,
        src_root,
    })
}

// ---------------------------------------------------------------------------
// Module discovery
// ---------------------------------------------------------------------------

/// Walk `src_root` recursively, collecting every `*.sky` file as a
/// [`DiscoveredModule`].
///
/// Files whose path contains a non-module-segment (e.g. lowercase first char
/// or characters outside `[A-Za-z0-9_]`) are silently skipped — they may be
/// build artefacts or editor swap files.
///
/// # Errors
/// [`CliError::Io`] if the directory cannot be read.
pub fn discover_modules(src_root: &Path) -> Result<Vec<DiscoveredModule>, CliError> {
    let mut result: Vec<DiscoveredModule> = Vec::new();
    let mut stack: VecDeque<PathBuf> = VecDeque::new();
    stack.push_back(src_root.to_path_buf());

    while let Some(dir) = stack.pop_front() {
        let entries = fs::read_dir(&dir).map_err(|e| CliError::Io {
            path: dir.clone(),
            source: e,
        })?;
        for entry in entries {
            let entry = entry.map_err(|e| CliError::Io {
                path: dir.clone(),
                source: e,
            })?;
            let path = entry.path();
            let file_type = entry.file_type().map_err(|e| CliError::Io {
                path: path.clone(),
                source: e,
            })?;
            if file_type.is_dir() {
                stack.push_back(path);
            } else if file_type.is_file()
                && path.extension().and_then(|e| e.to_str()) == Some("sky")
                && let Some(m) = file_to_module(src_root, &path)
            {
                result.push(m);
            }
        }
    }

    result.sort();
    Ok(result)
}

/// Map a `.sky` file path to a [`DiscoveredModule`], or `None` when the path
/// contains a non-module segment.
fn file_to_module(src_root: &Path, path: &Path) -> Option<DiscoveredModule> {
    // Strip the src_root prefix and the .sky extension.
    let rel = path.strip_prefix(src_root).ok()?;
    let without_ext = rel.with_extension("");
    // Split into segments using the OS path separator.
    let mut segments: Vec<String> = Vec::new();
    for component in without_ext.components() {
        let s = component.as_os_str().to_str()?;
        if !is_module_segment(s) {
            return None;
        }
        segments.push(s.to_owned());
    }
    if segments.is_empty() {
        return None;
    }
    Some(DiscoveredModule {
        path: path.to_path_buf(),
        module_path: segments,
    })
}

/// A Sky module path segment must start with an ASCII uppercase letter and
/// contain only ASCII alphanumerics and `_`.
fn is_module_segment(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_uppercase() => chars.all(|c| c.is_ascii_alphanumeric() || c == '_'),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Import graph + topological sort
// ---------------------------------------------------------------------------

/// Three-colour DFS node state. Declared at module scope so no items appear
/// after statements inside the function body (clippy `items_after_statements`).
#[derive(PartialEq)]
enum Color {
    White,
    Gray,
    Black,
}

/// DFS stack frame: (`module_path`, `remaining_deps`, `dfs_path_for_cycle_report`).
type DfsFrame = (Vec<String>, Vec<Vec<String>>, Vec<String>);

/// Build a dependency-first topological order of `modules`, given a function
/// `imports_of(module_path) -> Vec<Vec<String>>` that returns the modules each
/// source module imports.
///
/// Only modules whose path appears in `module_set` are followed; stdlib /
/// kernel imports (e.g. `List`, `String`) are silently ignored.
///
/// Returns the modules in dep-first order (i.e. a module's deps come before
/// it in the returned slice). The entry module (`["Main"]`) is always last.
///
/// # Errors
/// Returns [`CycleError`] when an import cycle is detected.
pub fn topological_order<F>(
    modules: &[DiscoveredModule],
    entry_path: &[String],
    imports_of: F,
) -> Result<Vec<DiscoveredModule>, CycleError>
where
    F: Fn(&[String]) -> Vec<Vec<String>>,
{
    // Build the module set for fast membership testing.
    let module_set: BTreeSet<Vec<String>> = modules.iter().map(|m| m.module_path.clone()).collect();

    // Map from module_path → DiscoveredModule for output reconstruction.
    let module_map: BTreeMap<Vec<String>, &DiscoveredModule> =
        modules.iter().map(|m| (m.module_path.clone(), m)).collect();

    let mut color: BTreeMap<Vec<String>, Color> = modules
        .iter()
        .map(|m| (m.module_path.clone(), Color::White))
        .collect();

    let mut result: Vec<DiscoveredModule> = Vec::new();
    // Explicit stack avoids recursion-stack overflow on deep dep graphs.
    let mut stack: Vec<DfsFrame> = Vec::new();

    // We start the DFS from `entry_path` so we only visit modules reachable
    // from the entry. Unknown modules (not in `module_set`) are skipped —
    // the caller's `canonicalise_module` will emit SKY-N0020 for them.
    let entry_deps = imports_of(entry_path)
        .into_iter()
        .filter(|d| module_set.contains(d))
        .collect();
    if let Some(color_entry) = color.get_mut(entry_path) {
        *color_entry = Color::Gray;
    }
    stack.push((
        entry_path.to_vec(),
        entry_deps,
        vec![format_path(entry_path)],
    ));

    while let Some((node, mut deps, dfs_path)) = stack.pop() {
        if let Some(next_dep) = deps.pop() {
            // Re-push the current node with remaining deps.
            stack.push((node, deps, dfs_path.clone()));

            match color.get(&next_dep) {
                Some(Color::Gray) => {
                    // Back edge → cycle. Build the cycle path.
                    let target = format_path(&next_dep);
                    let mut cycle_path = dfs_path;
                    cycle_path.push(target);
                    return Err(CycleError { path: cycle_path });
                }
                Some(Color::Black) | None => {
                    // Black: already fully visited — skip.
                    // None: not in module_set (stdlib import) — skip; SKY-N0020
                    // will fire if it's a real local dep that's missing.
                }
                Some(Color::White) => {
                    // First visit — push with its deps.
                    let sub_deps: Vec<Vec<String>> = imports_of(&next_dep)
                        .into_iter()
                        .filter(|d| module_set.contains(d))
                        .collect();
                    if let Some(c) = color.get_mut(&next_dep) {
                        *c = Color::Gray;
                    }
                    let mut sub_path = dfs_path.clone();
                    sub_path.push(format_path(&next_dep));
                    stack.push((next_dep, sub_deps, sub_path));
                }
            }
        } else {
            // All deps processed — mark node Black and record it.
            if let Some(c) = color.get_mut(&node) {
                *c = Color::Black;
            }
            if let Some(&m) = module_map.get(&node) {
                result.push(m.clone());
            }
        }
    }

    // Ensure every module reachable from the graph is included, even if not
    // reachable from entry (e.g. isolated modules — they get appended after).
    // In practice this handles orphaned modules gracefully.
    for m in modules {
        if !matches!(color.get(&m.module_path), Some(Color::Black)) {
            result.push(m.clone());
        }
    }

    Ok(result)
}

fn format_path(path: &[String]) -> String {
    path.join(".")
}

// ---------------------------------------------------------------------------
// Compiled-source stdlib injection (#98)
// ---------------------------------------------------------------------------

/// Transitively inject every compiled-source stdlib module the graph imports.
///
/// For each compiled-source module (`Std.Palette`, later `Std.Css` /
/// `Sky.Core.Error`) reachable from the current `sources`, seed a synthetic
/// source entry + [`DiscoveredModule`] so the EXISTING topo → dep-first
/// canonicalise → link path handles it unchanged.
///
/// Returns the set of module paths that were **actually injected from the embed
/// table** — the driver's unforgeable record of which modules are trusted
/// `EmbeddedStdlib` source. A path is added to this set ONLY when a NEW synthetic
/// entry is inserted; if `sources` already holds the key (a user file squatting
/// on `Std.Palette`, or an earlier injection), injection is skipped and the path
/// is NOT tagged trusted. So a hostile `src/Std/Palette.sky` is canonicalised as
/// `ModuleOrigin::User` and stays SKY-N0025-rejected.
///
/// Efficiency (design §7): the worklist is seeded only from imports that match a
/// compiled-source module, so a build that imports none does zero work.
pub fn inject_compiled_std_closure(
    sources: &mut BTreeMap<Vec<String>, (PathBuf, String)>,
    discovered: &mut Vec<DiscoveredModule>,
) -> BTreeSet<Vec<String>> {
    let mut injected: BTreeSet<Vec<String>> = BTreeSet::new();

    // Seed the worklist from every compiled-source import across current sources.
    // Short-circuit: an unused-stdlib build enqueues nothing and returns empty.
    let mut work: VecDeque<Vec<String>> = VecDeque::new();
    for (_, src) in sources.values() {
        for imp in extract_imports_from_source(src) {
            if crate::stdlib::is_compiled_source_segments(&imp) {
                work.push_back(imp);
            }
        }
    }

    while let Some(path) = work.pop_front() {
        // Already present — a user file OR an already-injected node. Skip; do NOT
        // tag trusted (BTreeMap key = free dedup; user-squat stays User origin).
        if sources.contains_key(&path) {
            continue;
        }
        let Some(embedded) = crate::stdlib::compiled_std_source_segments(&path) else {
            // Not a compiled-source module (kernel import inside an embedded
            // source, e.g. `Sky.Core.Prelude`): leave it kernel-resolved.
            continue;
        };

        // Synthetic on-disk-looking path, for diagnostics only. It is never read
        // from disk: `sources` already carries the embedded text.
        let synth_path = PathBuf::from("<embedded-stdlib>").join(path.join("."));
        sources.insert(path.clone(), (synth_path.clone(), embedded.to_owned()));
        discovered.push(DiscoveredModule {
            path: synth_path,
            module_path: path.clone(),
        });
        injected.insert(path.clone());

        // Std → Std closure: enqueue the embedded module's OWN compiled-source
        // imports (a kernel import inside it is not enqueued — it stays
        // qualifier-resolved). Fixpoint via the `sources.contains_key` guard.
        for imp in extract_imports_from_source(embedded) {
            if crate::stdlib::is_compiled_source_segments(&imp) && !sources.contains_key(&imp) {
                work.push_back(imp);
            }
        }
    }

    injected
}

// ---------------------------------------------------------------------------
// Import extraction from source text (pre-parse)
// ---------------------------------------------------------------------------

/// Extract the module paths named by `import` declarations from raw Sky source
/// text, without a full parse.
///
/// This is a best-effort line scanner used by the topo-sort driver to build
/// the import graph before any canonicalisation runs. It recognises:
///
/// ```sky
/// import Lib.Utils
/// import Lib.Utils as U
/// import Lib.Utils exposing (..)
/// import Lib.Utils exposing (foo, Bar)
/// ```
///
/// Kernel / stdlib imports (`import String`, `import List.Extra`) whose first
/// segment is lowercase or does not correspond to a discovered local module are
/// harmlessly included in the returned set — the topo-sort driver filters them
/// against the `module_set`.
///
/// Returns a `Vec<Vec<String>>` of path segments.
#[must_use]
pub fn extract_imports_from_source(source: &str) -> Vec<Vec<String>> {
    let mut imports: Vec<Vec<String>> = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("import ") {
            continue;
        }
        // Take the token after `import `, stopping at `as`, `exposing`, or
        // whitespace.
        let rest = trimmed["import ".len()..].trim_start();
        let module_str = rest
            .split(|c: char| c.is_whitespace() || c == '(')
            .next()
            .unwrap_or("");
        // Remove a trailing `as` keyword if it bled in (shouldn't happen but
        // defensive).
        let module_str = module_str
            .strip_suffix(" as")
            .map_or(module_str, |s| s.trim());
        let module_str = module_str.trim_end_matches(" as");
        let parts: Vec<String> = module_str.split('.').map(str::to_owned).collect();
        if parts.first().is_some_and(|s| !s.is_empty()) {
            imports.push(parts);
        }
    }
    imports
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_module_segment_rules() {
        assert!(is_module_segment("Main"));
        assert!(is_module_segment("Lib"));
        assert!(is_module_segment("Utils2"));
        assert!(is_module_segment("My_Module"));
        assert!(!is_module_segment("main"));
        assert!(!is_module_segment("123"));
        assert!(!is_module_segment(""));
        assert!(!is_module_segment("_Foo"));
    }

    #[test]
    fn extract_imports_parses_all_forms() {
        let src = "
module Main exposing (main)
import Lib.Utils
import Lib.Other as O
import Lib.Fmt exposing (..)
import Lib.Str exposing (fmt)
import String
";
        let imports = extract_imports_from_source(src);
        assert!(imports.contains(&vec!["Lib".to_owned(), "Utils".to_owned()]));
        assert!(imports.contains(&vec!["Lib".to_owned(), "Other".to_owned()]));
        assert!(imports.contains(&vec!["Lib".to_owned(), "Fmt".to_owned()]));
        assert!(imports.contains(&vec!["Lib".to_owned(), "Str".to_owned()]));
        assert!(imports.contains(&vec!["String".to_owned()]));
        assert!(!imports.contains(&vec!["main".to_owned()]));
    }

    #[test]
    fn topological_order_two_modules() {
        let modules = vec![
            DiscoveredModule {
                path: PathBuf::from("src/Main.sky"),
                module_path: vec!["Main".to_owned()],
            },
            DiscoveredModule {
                path: PathBuf::from("src/Lib/Utils.sky"),
                module_path: vec!["Lib".to_owned(), "Utils".to_owned()],
            },
        ];
        let order = topological_order(&modules, &["Main".to_owned()], |path| {
            if path == ["Main".to_owned()] {
                vec![vec!["Lib".to_owned(), "Utils".to_owned()]]
            } else {
                vec![]
            }
        });
        assert!(order.is_ok(), "no cycle expected");
        let order = order.expect("checked above");
        // Lib.Utils must come before Main.
        let lib_pos = order
            .iter()
            .position(|m| m.module_path == vec!["Lib".to_owned(), "Utils".to_owned()]);
        let main_pos = order
            .iter()
            .position(|m| m.module_path == vec!["Main".to_owned()]);
        assert!(
            lib_pos < main_pos,
            "Lib.Utils must precede Main in topo order"
        );
    }

    #[test]
    fn inject_closure_seeds_compiled_source_module() {
        // A Main importing Std.Palette gets the embedded source injected + a
        // DiscoveredModule pushed, and the path is recorded as trusted.
        let mut sources: BTreeMap<Vec<String>, (PathBuf, String)> = BTreeMap::new();
        sources.insert(
            vec!["Main".to_owned()],
            (
                PathBuf::from("src/Main.sky"),
                "module Main exposing (main)\nimport Std.Palette exposing (..)\nmain = 0\n"
                    .to_owned(),
            ),
        );
        let mut discovered = vec![DiscoveredModule {
            path: PathBuf::from("src/Main.sky"),
            module_path: vec!["Main".to_owned()],
        }];

        let injected = super::inject_compiled_std_closure(&mut sources, &mut discovered);

        let palette = vec!["Std".to_owned(), "Palette".to_owned()];
        assert!(injected.contains(&palette), "Std.Palette must be injected");
        assert!(sources.contains_key(&palette), "source seeded");
        assert!(
            discovered.iter().any(|m| m.module_path == palette),
            "DiscoveredModule pushed"
        );
    }

    #[test]
    fn inject_closure_short_circuits_when_no_compiled_import() {
        // Efficiency: a build importing no compiled-source module does zero work.
        let mut sources: BTreeMap<Vec<String>, (PathBuf, String)> = BTreeMap::new();
        sources.insert(
            vec!["Main".to_owned()],
            (
                PathBuf::from("src/Main.sky"),
                "module Main exposing (main)\nimport Sky.Core.Prelude exposing (..)\nmain = 0\n"
                    .to_owned(),
            ),
        );
        let mut discovered = vec![DiscoveredModule {
            path: PathBuf::from("src/Main.sky"),
            module_path: vec!["Main".to_owned()],
        }];

        let injected = super::inject_compiled_std_closure(&mut sources, &mut discovered);
        assert!(injected.is_empty(), "no compiled-source import → nothing injected");
        assert_eq!(sources.len(), 1, "sources untouched");
    }

    #[test]
    fn inject_closure_does_not_tag_user_squat_as_trusted() {
        // SECURITY: a user file already occupying the Std.Palette key is NOT
        // overwritten and NOT tagged trusted — it will canonicalise as User and
        // hit SKY-N0025.
        let mut sources: BTreeMap<Vec<String>, (PathBuf, String)> = BTreeMap::new();
        sources.insert(
            vec!["Main".to_owned()],
            (
                PathBuf::from("src/Main.sky"),
                "module Main exposing (main)\nimport Std.Palette exposing (..)\nmain = 0\n"
                    .to_owned(),
            ),
        );
        let palette = vec!["Std".to_owned(), "Palette".to_owned()];
        sources.insert(
            palette.clone(),
            (
                PathBuf::from("src/Std/Palette.sky"),
                "module Std.Palette exposing (..)\ntoHex = 0\n".to_owned(),
            ),
        );
        let mut discovered = vec![
            DiscoveredModule {
                path: PathBuf::from("src/Main.sky"),
                module_path: vec!["Main".to_owned()],
            },
            DiscoveredModule {
                path: PathBuf::from("src/Std/Palette.sky"),
                module_path: palette.clone(),
            },
        ];

        let injected = super::inject_compiled_std_closure(&mut sources, &mut discovered);
        assert!(
            !injected.contains(&palette),
            "a user file squatting on Std.Palette must NOT be tagged trusted"
        );
        // The user's source is preserved (not clobbered by the embed).
        let (_, src) = sources.get(&palette).expect("user source kept");
        assert!(
            src.contains("toHex = 0"),
            "user file preserved (injection skipped it)"
        );
    }

    #[test]
    fn topological_order_detects_cycle() {
        let modules = vec![
            DiscoveredModule {
                path: PathBuf::from("src/A.sky"),
                module_path: vec!["A".to_owned()],
            },
            DiscoveredModule {
                path: PathBuf::from("src/B.sky"),
                module_path: vec!["B".to_owned()],
            },
        ];
        let result = topological_order(&modules, &["A".to_owned()], |path| {
            if path == ["A".to_owned()] {
                vec![vec!["B".to_owned()]]
            } else if path == ["B".to_owned()] {
                vec![vec!["A".to_owned()]]
            } else {
                vec![]
            }
        });
        assert!(result.is_err(), "A ↔ B cycle must be detected");
    }
}
