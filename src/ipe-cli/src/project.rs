//! Multi-module project manifest parsing, module discovery, import graph, and
//! topological sort.
//!
//! The `sky.toml` format is a minimal subset: only `[project]` and `name` / the
//! source root (`src/`) are significant. No external TOML crate is used —
//! the relevant structure is simple enough for a line-by-line parser.
//!
//! # Discovery
//!
//! Given a project directory (containing `sky.toml`), the driver:
//!
//! 1. Reads `sky.toml` to obtain the project name and confirm the source root
//!    exists (`src/` by default).
//! 2. Walks `src/` recursively, collecting every `*.ipe` file.
//! 3. Maps each file path to a module name by:
//!    - Stripping the `src/` prefix and `.ipe` suffix.
//!    - Splitting on the OS path separator to obtain segment strings.
//!    - Rejecting any segment that is not a valid Ipê module segment
//!      (`[A-Z][A-Za-z0-9_]*`).
//! 4. The entry module is always `Main` (`src/Main.ipe`).
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
    /// The SQL driver the emitted project targets (from `[database] driver
    /// = "…"`). Defaults to [`ipe_backend_rust::DbDriver::Sqlite`] when the
    /// `[database]` section (or the `driver` key within it) is absent — the
    /// documented default in `AGENTS.md`'s `sky.toml` schema table.
    pub driver: ipe_backend_rust::DbDriver,
    /// The `[rust]` static-build request layer (`static` / `target` /
    /// `allocator` / `allowSlowAllocator`) — the lowest-precedence layer
    /// (CLI > env > `sky.toml`) of `crate::build_plan::resolve`'s input.
    /// Every field defaults to unset when the section (or key) is absent.
    /// Malformed values (a bad bool, an unknown allocator) are refused at
    /// parse time, never silently ignored.
    pub static_request: crate::build_plan::StaticRequestLayer,
    /// The `[wasm]` section (M5: `mode`/`entry`/`mount`/`publicEnv`/
    /// `optLevel`) — `WasmConfig::default()` (mode "off") when the section is
    /// absent.
    pub wasm: WasmConfig,
}

/// `[wasm]` `sky.toml` section (spec: `docs/architecture/wasm-target.md` Q6
/// "Opt-in mechanism").
///
/// ```toml
/// [wasm]
/// mode      = "spa"              # spa (MVP) | hydrate (MVP+1) | off (default)
/// entry     = "src/Client.ipe"   # client entry; its reachability closure is the bundle
/// mount     = "#app"             # SPA mount node
/// publicEnv = ["API_BASE_URL"]   # default-deny allowlist; rejects IPE_* / secret patterns
/// optLevel  = "z"
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WasmConfig {
    /// `"spa"` / `"hydrate"` / `"off"` (default when the key or section is
    /// absent — `--target wasm` still works without a `[wasm]` section; this
    /// field is metadata for the eventual SSR+hydration/SPA-shell split, not
    /// a gate on `ipe build --target wasm` itself).
    pub mode: Option<String>,
    /// The client entry module's file, relative to the project root
    /// (defaults to the build's own entry file when absent — see M6).
    pub entry: Option<String>,
    /// The SPA mount selector (e.g. `"#app"`).
    pub mount: Option<String>,
    /// The `Ipe.Env.public` default-deny allowlist: environment variable
    /// names the wasm bundle may read at build time. Validated against the
    /// secret-name denylist at PARSE time (below) — listing a denylisted
    /// name here is a build error, never a runtime refusal.
    pub public_env: Vec<String>,
    /// `wasm-opt` optimisation level (`"z"`/`"s"`/`"0"`..`"3"`).
    pub opt_level: Option<String>,
}

/// The `[wasm].publicEnv` secret-name denylist (spec Q5 "Config: default-deny
/// allowlist (+ layered secret denylist)"): `*_SECRET`, `*_TOKEN`, `*_KEY`,
/// `*_PASSWORD`, `DATABASE_URL`, and the internal `IPE_*` namespace. An
/// allowlisted name matching this is a BUILD error (parse time), forcing the
/// author to confirm — never a silent drop, never a runtime-only refusal.
/// Case-insensitive (`sky.toml` authors may write either case; the runtime
/// env-var namespace itself is case-sensitive POSIX convention, but a
/// same-name-different-case entry is exactly the kind of "did they mean the
/// secret" ambiguity this gate exists to catch).
#[must_use]
pub fn is_denylisted_public_env_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    upper == "DATABASE_URL"
        || upper.starts_with("IPE_")
        || upper.ends_with("_SECRET")
        || upper.ends_with("_TOKEN")
        || upper.ends_with("_KEY")
        || upper.ends_with("_PASSWORD")
}

/// Validate `[wasm] publicEnv` against [`is_denylisted_public_env_name`].
///
/// # Errors
/// [`CliError::UsageOwned`] naming the first denylisted entry found.
fn validate_public_env(names: &[String]) -> Result<(), CliError> {
    for name in names {
        if is_denylisted_public_env_name(name) {
            return Err(CliError::UsageOwned(format!(
                "sky.toml: [wasm] publicEnv lists {name:?}, which matches the secret-name \
                 denylist (*_SECRET / *_TOKEN / *_KEY / *_PASSWORD / DATABASE_URL / the \
                 internal IPE_* namespace) — a secret environment variable can never be \
                 allowlisted into the public wasm bundle, allowlisted or not"
            )));
        }
    }
    Ok(())
}

/// Parse a TOML string array `["a", "b", "c"]` — the one array shape
/// `[wasm] publicEnv` needs. Each element must be a double-quoted string;
/// whitespace around commas/brackets is tolerated. Not a general TOML array
/// parser (this file's `sky.toml` reader is a deliberately minimal line
/// parser, not a full TOML implementation — see the module doc).
fn parse_string_array(raw: &str) -> Result<Vec<String>, CliError> {
    let trimmed = raw.trim();
    let Some(inner) = trimmed.strip_prefix('[').and_then(|s| s.strip_suffix(']')) else {
        return Err(CliError::UsageOwned(format!(
            "sky.toml: [wasm] publicEnv must be a `[\"NAME\", …]` array, got: {raw}"
        )));
    };
    let inner = inner.trim();
    if inner.is_empty() {
        return Ok(Vec::new());
    }
    inner
        .split(',')
        .map(|item| {
            let item = item.trim();
            let unquoted = item.strip_prefix('"').and_then(|s| s.strip_suffix('"'));
            unquoted.map(str::to_owned).ok_or_else(|| {
                CliError::UsageOwned(format!(
                    "sky.toml: [wasm] publicEnv entry must be a quoted string, got: {item}"
                ))
            })
        })
        .collect()
}

/// A discovered Ipê source file with its resolved module path.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DiscoveredModule {
    /// Absolute path to the `.ipe` source file.
    pub path: PathBuf,
    /// Module path segments, e.g. `Lib/Utils.ipe` → `["Lib", "Utils"]`.
    pub module_path: Vec<String>,
}

/// An import edge: the importing module's path and the imported module's path.
#[derive(Clone, Debug)]
pub struct ImportEdge {
    pub from: Vec<String>,
    pub to: Vec<String>,
}

/// An import cycle detected during topological sort. The single definition
/// lives in [`ipe_db`] (beside the shared topo algorithm); re-exported here
/// for the driver and existing callers.
pub use ipe_db::CycleError;

// ---------------------------------------------------------------------------
// Manifest parsing
// ---------------------------------------------------------------------------

/// Parse a `[database] driver = "…"` value. Recognises `"sqlite"` (also the
/// default when the section/key is absent) and `"postgres"` / `"postgresql"`.
/// Any other value is a hard error naming the bad value — silently falling
/// back to sqlite on a typo (`"postgre"`, `"postgress"`) would build a project
/// the user believes targets Postgres but that actually runs against a local
/// `SQLite` file, a correctness footgun worse than a loud rejection.
///
/// # Errors
/// [`CliError::UsageOwned`] naming the unrecognised value.
fn parse_db_driver(s: &str) -> Result<ipe_backend_rust::DbDriver, CliError> {
    match s {
        "sqlite" => Ok(ipe_backend_rust::DbDriver::Sqlite),
        "postgres" | "postgresql" => Ok(ipe_backend_rust::DbDriver::Postgres),
        other => Err(CliError::UsageOwned(format!(
            "sky.toml: [database] driver = {other:?} is not supported \
             (expected \"sqlite\" or \"postgres\")"
        ))),
    }
}

/// Parse a `sky.toml` file and return a [`ProjectManifest`].
///
/// The format recognised:
/// ```toml
/// [project]
/// name = "my-app"
///
/// [database]
/// driver = "sqlite"   # or "postgres" — defaults to "sqlite" when absent
/// ```
/// Lines that start with `#` are comments and are ignored. All other lines
/// outside `[project]` / `[database]` are ignored (forward-compatible). Within
/// `[project]`, only `name` is extracted; within `[database]`, only `driver`
/// is extracted; other keys are ignored.
///
/// # Errors
/// [`CliError::Io`] if the file cannot be read; [`CliError::Usage`] if the
/// manifest is malformed or the `src/` directory does not exist;
/// [`CliError::UsageOwned`] if `[database] driver` names an unsupported value.
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
    // defaulting to `src`; the driver comes from `[database] driver = "…"`,
    // defaulting to sqlite. `section` is the empty string at the top level.
    let mut section = "";
    let mut name: Option<String> = None;
    let mut src_rel: Option<String> = None;
    let mut driver_str: Option<String> = None;
    let mut rust_static: Option<String> = None;
    let mut rust_target: Option<String> = None;
    let mut rust_allocator: Option<String> = None;
    let mut rust_allow_slow: Option<String> = None;
    let mut wasm_mode: Option<String> = None;
    let mut wasm_entry: Option<String> = None;
    let mut wasm_mount: Option<String> = None;
    let mut wasm_public_env: Vec<String> = Vec::new();
    let mut wasm_opt_level: Option<String> = None;

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
            } else if line == "[database]" {
                "[database]"
            } else if line == "[rust]" {
                "[rust]"
            } else if line == "[wasm]" {
                "[wasm]"
            } else {
                "other"
            };
            continue;
        }
        let Some((key, val)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let raw_val = val.trim();
        let val = raw_val.trim_matches('"');
        match (section, key) {
            ("" | "[project]", "name") => name = Some(val.to_owned()),
            ("[source]", "root") => src_rel = Some(val.to_owned()),
            ("[database]", "driver") => driver_str = Some(val.to_owned()),
            ("[rust]", "static") => rust_static = Some(val.to_owned()),
            ("[rust]", "target") => rust_target = Some(val.to_owned()),
            ("[rust]", "allocator") => rust_allocator = Some(val.to_owned()),
            ("[rust]", "allowSlowAllocator") => rust_allow_slow = Some(val.to_owned()),
            ("[wasm]", "mode") => wasm_mode = Some(val.to_owned()),
            ("[wasm]", "entry") => wasm_entry = Some(val.to_owned()),
            ("[wasm]", "mount") => wasm_mount = Some(val.to_owned()),
            ("[wasm]", "publicEnv") => wasm_public_env = parse_string_array(raw_val)?,
            ("[wasm]", "optLevel") => wasm_opt_level = Some(val.to_owned()),
            _ => {}
        }
    }

    validate_public_env(&wasm_public_env)?;
    let wasm = WasmConfig {
        mode: wasm_mode,
        entry: wasm_entry,
        mount: wasm_mount,
        public_env: wasm_public_env,
        opt_level: wasm_opt_level,
    };

    let name = name.ok_or(CliError::Usage("sky.toml: missing a `name = \"…\"` entry"))?;

    let src_root = root.join(src_rel.as_deref().unwrap_or("src"));
    if !src_root.is_dir() {
        return Err(CliError::Usage(
            "sky.toml: the source root directory does not exist",
        ));
    }

    let driver = match driver_str {
        Some(s) => parse_db_driver(&s)?,
        None => ipe_backend_rust::DbDriver::Sqlite,
    };

    // Parse, don't validate: the `[rust]` values become typed request fields
    // here, so a typo'd allocator or bool is a hard error at manifest-parse
    // time — the same posture as `[database] driver` above.
    let static_request = crate::build_plan::StaticRequestLayer {
        static_build: rust_static
            .map(|v| crate::build_plan::parse_bool("sky.toml: [rust] static", &v))
            .transpose()?,
        target: rust_target,
        allocator: rust_allocator
            .map(|v| crate::build_plan::AllocatorChoice::parse(&v))
            .transpose()?,
        allow_slow_allocator: rust_allow_slow
            .map(|v| crate::build_plan::parse_bool("sky.toml: [rust] allowSlowAllocator", &v))
            .transpose()?,
    };

    Ok(ProjectManifest {
        name,
        root,
        src_root,
        driver,
        static_request,
        wasm,
    })
}

// ---------------------------------------------------------------------------
// Module discovery
// ---------------------------------------------------------------------------

/// Walk `src_root` recursively, collecting every `*.ipe` file as a
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
                && path.extension().and_then(|e| e.to_str()) == Some("ipe")
                && let Some(m) = file_to_module(src_root, &path)
            {
                result.push(m);
            }
        }
    }

    result.sort();
    Ok(result)
}

/// Map a `.ipe` file path to a [`DiscoveredModule`], or `None` when the path
/// contains a non-module segment.
fn file_to_module(src_root: &Path, path: &Path) -> Option<DiscoveredModule> {
    // Strip the src_root prefix and the .ipe extension.
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

/// A Ipê module path segment must start with an ASCII uppercase letter and
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

/// Build a dependency-first topological order of `modules`, given a function
/// `imports_of(module_path) -> Vec<Vec<String>>` that returns the modules each
/// source module imports.
///
/// Only modules whose path appears in the module set are followed; stdlib /
/// kernel imports (e.g. `List`, `String`) are silently ignored.
///
/// Returns the modules in dep-first order (i.e. a module's deps come before
/// it in the returned slice). The entry module (`["Main"]`) is always last.
///
/// Delegates to [`ipe_db::topological_order_paths`] — the single topo-sort
/// algorithm, shared with the memoized `ipe_db::topo_order` query so the
/// two orders can never drift.
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
    let paths: Vec<Vec<String>> = modules.iter().map(|m| m.module_path.clone()).collect();
    let order = ipe_db::topological_order_paths(&paths, entry_path, imports_of)?;

    // Map each ordered path back to its DiscoveredModule (last claimant wins
    // on a duplicate module path, matching the pre-delegation collect()).
    let mut module_map: BTreeMap<&[String], &DiscoveredModule> = BTreeMap::new();
    for m in modules {
        module_map.insert(m.module_path.as_slice(), m);
    }
    Ok(order
        .iter()
        .filter_map(|p| module_map.get(p.as_slice()).map(|&m| m.clone()))
        .collect())
}

// ---------------------------------------------------------------------------
// Compiled-source stdlib injection
// ---------------------------------------------------------------------------

/// Transitively inject every compiled-source stdlib module the graph imports.
///
/// For each compiled-source module (`Ipe.Palette`, later `Ipe.Css` /
/// `Ipe.Error`) reachable from the current `sources`, seed a synthetic
/// source entry + [`DiscoveredModule`] so the EXISTING topo → dep-first
/// canonicalise → link path handles it unchanged.
///
/// Returns the set of module paths that were **actually injected from the embed
/// table** — the driver's unforgeable record of which modules are trusted
/// `EmbeddedStdlib` source. A path is added to this set ONLY when a NEW synthetic
/// entry is inserted; if `sources` already holds the key (a user file squatting
/// on `Ipe.Palette`, or an earlier injection), injection is skipped and the path
/// is NOT tagged trusted. So a hostile `src/Std/Palette.ipe` is canonicalised as
/// `ModuleOrigin::User` and stays IPE-N0025-rejected.
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
            // source, e.g. `Ipe.Prelude`): leave it kernel-resolved.
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

/// Extract the module paths named by `import` declarations from raw Ipê source
/// text, without a full parse.
///
/// This is a token-level scan (real lexer — its edge set is a
/// superset-or-equal of the AST's import edges, so the IPE-N0021 cycle gate
/// cannot be bypassed by lexer-legal-but-unusual spelling such as
/// `import\tB`) used by the topo-sort driver to build the import graph
/// before any canonicalisation runs. It recognises:
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
/// The single implementation lives in [`ipe_db`] (it also backs the memoized
/// `ipe_db::imports` query the topo sort consumes) — re-exported here so the
/// scan used for stdlib-closure injection and the scan used for topo ordering
/// can never drift apart.
pub use ipe_db::extract_imports_from_source;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Write a minimal manifest + `src/Main.ipe` under a fresh temp dir and
    /// return the manifest path. `database_section` is spliced in verbatim
    /// (empty string → no `[database]` section at all).
    fn write_manifest(test_name: &str, database_section: &str) -> PathBuf {
        let tmp = std::env::temp_dir().join(format!("skyc_project_{test_name}"));
        let _ = fs::remove_dir_all(&tmp);
        let src = tmp.join("src");
        fs::create_dir_all(&src).expect("create src/");
        fs::write(
            src.join("Main.ipe"),
            "module Main exposing (main)\nmain = 0\n",
        )
        .expect("write Main.ipe");
        let toml_path = tmp.join("sky.toml");
        fs::write(
            &toml_path,
            format!("[project]\nname = \"test\"\n{database_section}"),
        )
        .expect("write sky.toml");
        toml_path
    }

    /// No `[database]` section at all →
    /// the manifest defaults to `DbDriver::Sqlite`, matching the documented
    /// `sky.toml` schema default.
    #[test]
    fn parse_manifest_no_database_section_defaults_to_sqlite() {
        let toml_path = write_manifest("no_db_section", "");
        let manifest = parse_manifest(&toml_path).expect("manifest must parse");
        assert_eq!(manifest.driver, ipe_backend_rust::DbDriver::Sqlite);
        let _ = fs::remove_dir_all(toml_path.parent().expect("has parent"));
    }

    #[test]
    fn parse_manifest_explicit_sqlite_driver() {
        let toml_path = write_manifest("explicit_sqlite", "[database]\ndriver = \"sqlite\"\n");
        let manifest = parse_manifest(&toml_path).expect("manifest must parse");
        assert_eq!(manifest.driver, ipe_backend_rust::DbDriver::Sqlite);
        let _ = fs::remove_dir_all(toml_path.parent().expect("has parent"));
    }

    #[test]
    fn parse_manifest_postgres_driver() {
        let toml_path = write_manifest("postgres", "[database]\ndriver = \"postgres\"\n");
        let manifest = parse_manifest(&toml_path).expect("manifest must parse");
        assert_eq!(manifest.driver, ipe_backend_rust::DbDriver::Postgres);
        let _ = fs::remove_dir_all(toml_path.parent().expect("has parent"));
    }

    #[test]
    fn parse_manifest_postgresql_alias_driver() {
        let toml_path = write_manifest("postgresql_alias", "[database]\ndriver = \"postgresql\"\n");
        let manifest = parse_manifest(&toml_path).expect("manifest must parse");
        assert_eq!(manifest.driver, ipe_backend_rust::DbDriver::Postgres);
        let _ = fs::remove_dir_all(toml_path.parent().expect("has parent"));
    }

    /// An unsupported `driver` value must be a loud, named error — NOT a
    /// silent fallback to sqlite (a silent fallback would build a project the
    /// user believes targets `driver = "mysql"` but that actually runs
    /// against a local `SQLite` file).
    #[test]
    fn parse_manifest_unsupported_driver_is_a_named_error() {
        let toml_path = write_manifest("unsupported_driver", "[database]\ndriver = \"mysql\"\n");
        let err = parse_manifest(&toml_path).expect_err("mysql driver must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("mysql"),
            "error must name the unsupported value: {msg}"
        );
        let _ = fs::remove_dir_all(toml_path.parent().expect("has parent"));
    }

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
                path: PathBuf::from("src/Main.ipe"),
                module_path: vec!["Main".to_owned()],
            },
            DiscoveredModule {
                path: PathBuf::from("src/Lib/Utils.ipe"),
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
        // A Main importing Ipe.Palette gets the embedded source injected + a
        // DiscoveredModule pushed, and the path is recorded as trusted.
        let mut sources: BTreeMap<Vec<String>, (PathBuf, String)> = BTreeMap::new();
        sources.insert(
            vec!["Main".to_owned()],
            (
                PathBuf::from("src/Main.ipe"),
                "module Main exposing (main)\nimport Ipe.Palette exposing (..)\nmain = 0\n"
                    .to_owned(),
            ),
        );
        let mut discovered = vec![DiscoveredModule {
            path: PathBuf::from("src/Main.ipe"),
            module_path: vec!["Main".to_owned()],
        }];

        let injected = super::inject_compiled_std_closure(&mut sources, &mut discovered);

        let palette = vec!["Ipe".to_owned(), "Palette".to_owned()];
        assert!(injected.contains(&palette), "Ipe.Palette must be injected");
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
                PathBuf::from("src/Main.ipe"),
                "module Main exposing (main)\nimport Ipe.Prelude exposing (..)\nmain = 0\n"
                    .to_owned(),
            ),
        );
        let mut discovered = vec![DiscoveredModule {
            path: PathBuf::from("src/Main.ipe"),
            module_path: vec!["Main".to_owned()],
        }];

        let injected = super::inject_compiled_std_closure(&mut sources, &mut discovered);
        assert!(
            injected.is_empty(),
            "no compiled-source import → nothing injected"
        );
        assert_eq!(sources.len(), 1, "sources untouched");
    }

    #[test]
    fn inject_closure_does_not_tag_user_squat_as_trusted() {
        // SECURITY: a user file already occupying the Ipe.Palette key is NOT
        // overwritten and NOT tagged trusted — it will canonicalise as User and
        // hit IPE-N0025.
        let mut sources: BTreeMap<Vec<String>, (PathBuf, String)> = BTreeMap::new();
        sources.insert(
            vec!["Main".to_owned()],
            (
                PathBuf::from("src/Main.ipe"),
                "module Main exposing (main)\nimport Ipe.Palette exposing (..)\nmain = 0\n"
                    .to_owned(),
            ),
        );
        let palette = vec!["Ipe".to_owned(), "Palette".to_owned()];
        sources.insert(
            palette.clone(),
            (
                PathBuf::from("src/Std/Palette.ipe"),
                "module Ipe.Palette exposing (..)\ntoHex = 0\n".to_owned(),
            ),
        );
        let mut discovered = vec![
            DiscoveredModule {
                path: PathBuf::from("src/Main.ipe"),
                module_path: vec!["Main".to_owned()],
            },
            DiscoveredModule {
                path: PathBuf::from("src/Std/Palette.ipe"),
                module_path: palette.clone(),
            },
        ];

        let injected = super::inject_compiled_std_closure(&mut sources, &mut discovered);
        assert!(
            !injected.contains(&palette),
            "a user file squatting on Ipe.Palette must NOT be tagged trusted"
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
                path: PathBuf::from("src/A.ipe"),
                module_path: vec!["A".to_owned()],
            },
            DiscoveredModule {
                path: PathBuf::from("src/B.ipe"),
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

    // ── [wasm] section (M5) ──────────────────────────────────────────────

    #[test]
    fn no_wasm_section_defaults_to_empty_config() {
        let toml_path = write_manifest("no_wasm_section", "");
        let manifest = parse_manifest(&toml_path).expect("manifest must parse");
        assert_eq!(manifest.wasm, WasmConfig::default());
    }

    #[test]
    fn wasm_section_parses_every_field() {
        let toml_path = write_manifest(
            "wasm_full_section",
            "[wasm]\n\
             mode = \"spa\"\n\
             entry = \"src/Client.ipe\"\n\
             mount = \"#app\"\n\
             publicEnv = [\"API_BASE_URL\", \"APP_VERSION\"]\n\
             optLevel = \"z\"\n",
        );
        let manifest = parse_manifest(&toml_path).expect("manifest must parse");
        assert_eq!(manifest.wasm.mode.as_deref(), Some("spa"));
        assert_eq!(manifest.wasm.entry.as_deref(), Some("src/Client.ipe"));
        assert_eq!(manifest.wasm.mount.as_deref(), Some("#app"));
        assert_eq!(
            manifest.wasm.public_env,
            vec!["API_BASE_URL".to_owned(), "APP_VERSION".to_owned()]
        );
        assert_eq!(manifest.wasm.opt_level.as_deref(), Some("z"));
    }

    /// `IPE_AUTH_TOKEN_SECRET` can be neither read (no `System.getenv`
    /// denotation for wasm — Layer 1) nor allowlisted: listing it in
    /// `[wasm] publicEnv` is a BUILD error at `sky.toml` parse time, never a
    /// silently-dropped entry and never a runtime-only refusal.
    #[test]
    fn public_env_rejects_the_auth_secret_at_parse_time() {
        let toml_path = write_manifest(
            "wasm_secret_denied",
            "[wasm]\npublicEnv = [\"IPE_AUTH_TOKEN_SECRET\"]\n",
        );
        let err = parse_manifest(&toml_path).expect_err("a secret name must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("IPE_AUTH_TOKEN_SECRET"),
            "error must name the offending entry: {msg}"
        );
    }

    #[test]
    fn public_env_rejects_every_denylisted_pattern() {
        for denied in [
            "DATABASE_URL",
            "STRIPE_SECRET_KEY",
            "SESSION_TOKEN",
            "API_KEY",
            "ADMIN_PASSWORD",
            "IPE_ANYTHING",
        ] {
            assert!(
                is_denylisted_public_env_name(denied),
                "{denied} must match the secret-name denylist"
            );
        }
    }

    #[test]
    fn public_env_allows_ordinary_config_names() {
        for allowed in ["API_BASE_URL", "APP_VERSION", "FEATURE_FLAG_X"] {
            assert!(
                !is_denylisted_public_env_name(allowed),
                "{allowed} must NOT match the secret-name denylist"
            );
        }
        let toml_path = write_manifest(
            "wasm_safe_public_env",
            "[wasm]\npublicEnv = [\"API_BASE_URL\"]\n",
        );
        parse_manifest(&toml_path).expect("an ordinary config name must parse cleanly");
    }
}
