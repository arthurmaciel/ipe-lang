//! Multi-module project manifest parsing, module discovery, import graph, and
//! topological sort.
//!
//! The `ipe.toml` format is a minimal subset: only `[project]` and `name` / the
//! source root (`src/`) are significant. No external TOML crate is used —
//! the relevant structure is simple enough for a line-by-line parser.
//!
//! # Discovery
//!
//! Given a project directory (containing `ipe.toml`), the driver:
//!
//! 1. Reads `ipe.toml` to obtain the project name and confirm the source root
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

/// The parsed, validated content of a `ipe.toml` manifest.
#[derive(Clone, Debug)]
pub struct ProjectManifest {
    /// The project name (from `[project] name = "…"`).
    pub name: String,
    /// The package version (from `version = "…"`, at the top level or under
    /// `[project]`), parsed into a typed [`semver::Version`]. `None` when the
    /// manifest declares no version — the package gate's enforced-semver check
    /// needs one, so `ipe package audit` rejects a versionless manifest rather
    /// than inventing a version.
    pub version: Option<semver::Version>,
    /// Absolute path to the project root directory (where `ipe.toml` lives).
    pub root: PathBuf,
    /// Absolute path to the source root (`<root>/src` by default).
    pub src_root: PathBuf,
    /// The SQL driver the emitted project targets (from `[database] driver
    /// = "…"`). Defaults to [`ipe_backend_rust::DbDriver::Sqlite`] when the
    /// `[database]` section (or the `driver` key within it) is absent — the
    /// documented default in `AGENTS.md`'s `ipe.toml` schema table.
    pub driver: ipe_backend_rust::DbDriver,
    /// The `[rust]` static-build request layer (`static` / `target` /
    /// `allocator` / `allowSlowAllocator`) — the lowest-precedence layer
    /// (CLI > env > `ipe.toml`) of `crate::build_plan::resolve`'s input.
    /// Every field defaults to unset when the section (or key) is absent.
    /// Malformed values (a bad bool, an unknown allocator) are refused at
    /// parse time, never silently ignored.
    pub static_request: crate::build_plan::StaticRequestLayer,
    /// The `[wasm]` section (M5: `mode`/`entry`/`mount`/`publicEnv`/
    /// `optLevel`) — `WasmConfig::default()` (mode "off") when the section is
    /// absent.
    pub wasm: WasmConfig,
    /// The `[dependencies]` section: Ipê packages by name. Empty when the
    /// section is absent (back-compat). Resolution (fetch / download / lockfile)
    /// is SP3; this is the parsed schema only.
    pub dependencies: BTreeMap<String, IpeDep>,
    /// The `[rust.dependencies]` section: crates.io crates bound as
    /// foreign-function dependencies. Empty when the section is absent.
    pub rust_dependencies: BTreeMap<String, RustDep>,
    /// The `[capabilities] declared = […]` set: the security capabilities the
    /// author declares the program exercises. Empty when the section is absent.
    /// Verifying this against the compiler's inferred set is SP4.
    pub capabilities: BTreeSet<Capability>,
}

/// `[wasm]` `ipe.toml` section (spec: `docs/adr/0042-wasm-client-target.md` Q6
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

impl WasmConfig {
    /// Whether this config's `mode` implies the `WasmClient` compilation
    /// target.
    ///
    /// `true` for any active mode (`"spa"`, `"hydrate"`, or any future on-value).
    /// `false` for the explicit opt-out (`"off"`) and for the absent default
    /// (no `[wasm]` section / no `mode` key — both leave `mode` as `None`).
    #[must_use]
    pub fn implies_wasm_target(&self) -> bool {
        match self.mode.as_deref() {
            None | Some("off") => false,
            Some(_) => true,
        }
    }
}

/// The `[wasm].publicEnv` secret-name denylist (spec Q5 "Config: default-deny
/// allowlist (+ layered secret denylist)").
///
/// Denies `*_SECRET`, `*_TOKEN`, `*_KEY`, `*_PASSWORD`, `DATABASE_URL`, and
/// the internal `IPE_*` namespace. An allowlisted name matching this is a
/// BUILD error (parse time), forcing the author to confirm — never a silent
/// drop, never a runtime-only refusal. Case-insensitive (`ipe.toml` authors
/// may write either case; the runtime env-var namespace itself is
/// case-sensitive POSIX convention, but a same-name-different-case entry is
/// exactly the kind of "did they mean the secret" ambiguity this gate exists
/// to catch).
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
                "ipe.toml: [wasm] publicEnv lists {name:?}, which matches the secret-name \
                 denylist (*_SECRET / *_TOKEN / *_KEY / *_PASSWORD / DATABASE_URL / the \
                 internal IPE_* namespace) — a secret environment variable can never be \
                 allowlisted into the public wasm bundle, allowlisted or not"
            )));
        }
    }
    Ok(())
}

/// Parse a TOML string array `["a", "b", "c"]` — the one array shape both
/// `[wasm] publicEnv` and `[capabilities] declared` need. `context` names the
/// section in any error (`"[wasm] publicEnv"`). Each element must be a
/// double-quoted string; whitespace around commas/brackets is tolerated. Not a
/// general TOML array parser (this file's `ipe.toml` reader is a deliberately
/// minimal line parser, not a full TOML implementation — see the module doc).
fn parse_string_array(context: &str, raw: &str) -> Result<Vec<String>, CliError> {
    let trimmed = raw.trim();
    let Some(inner) = trimmed.strip_prefix('[').and_then(|s| s.strip_suffix(']')) else {
        return Err(CliError::UsageOwned(format!(
            "ipe.toml: {context} must be a `[\"NAME\", …]` array, got: {raw}"
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
                    "ipe.toml: {context} entry must be a quoted string, got: {item}"
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

/// The capability vocabulary, re-exported from the kernel registry so the
/// manifest's `[capabilities]` set and the compiler's inferred set are the same
/// type.
pub use ipe_ir::Capability;

/// One `ipe.toml [dependencies]` entry: an Ipê package pulled from the index by
/// version, or one of the two escapes (a git repo, a local path).
///
/// Modelled as a sum, not three optional fields, so an entry can never be both a
/// git and a path dependency at once (make-invalid-states-unrepresentable). The
/// index case carries a parsed [`semver::VersionReq`], so a malformed version is
/// a manifest-parse error, never a resolution-time surprise.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IpeDep {
    /// Resolved from the package index by a semver requirement (`http = "^1.2"`).
    Index(semver::VersionReq),
    /// A git repository escape (`{ git = "…", rev = "…" }`); `rev` is optional.
    Git {
        /// The repository URL.
        url: String,
        /// A pinned revision (commit / tag / branch), when given.
        rev: Option<String>,
    },
    /// A local path escape (`{ path = "../local" }`), relative to the manifest.
    Path(PathBuf),
}

/// One `ipe.toml [rust.dependencies]` entry: a crates.io crate bound as a
/// foreign-function dependency, with its version requirement and feature list.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct RustDep {
    /// The version requirement string (empty when unspecified). Left as the raw
    /// string the crate ecosystem's own resolver consumes, not a parsed
    /// [`semver::VersionReq`] — it is handed verbatim to cargo.
    pub version: String,
    /// The requested crate features (empty when unspecified).
    pub features: Vec<String>,
}

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
            "ipe.toml: [database] driver = {other:?} is not supported \
             (expected \"sqlite\" or \"postgres\")"
        ))),
    }
}

/// The raw per-section values a single line-scan of a `ipe.toml` collects,
/// before they are turned into the typed [`ProjectManifest`] fields. Splitting
/// the scan out keeps [`parse_manifest`] a straight assembly of typed values.
#[derive(Default)]
struct RawManifest {
    name: Option<String>,
    version_str: Option<String>,
    src_rel: Option<String>,
    driver_str: Option<String>,
    rust_static: Option<String>,
    rust_target: Option<String>,
    rust_allocator: Option<String>,
    rust_allow_slow: Option<String>,
    wasm_mode: Option<String>,
    wasm_entry: Option<String>,
    wasm_mount: Option<String>,
    wasm_public_env: Vec<String>,
    wasm_opt_level: Option<String>,
    dependencies: BTreeMap<String, IpeDep>,
    rust_dependencies: BTreeMap<String, RustDep>,
    capabilities: BTreeSet<Capability>,
}

/// Scan a `ipe.toml`'s lines once, collecting each recognised section's raw
/// values. `name` may sit at the top level (Ipê's own examples) or under
/// `[project]`; unrecognised sections and keys are ignored (forward-compatible).
///
/// # Errors
/// [`CliError::UsageOwned`] when a `[dependencies]`, `[capabilities]`, or
/// `[wasm] publicEnv` value is malformed.
fn scan_raw_manifest(text: &str) -> Result<RawManifest, CliError> {
    let mut raw = RawManifest::default();
    let mut section = "";
    for line in text.lines().map(str::trim) {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            section = match line {
                "[project]" | "[source]" | "[database]" | "[rust]" | "[wasm]"
                | "[dependencies]" | "[capabilities]" => line,
                // Both spellings of the FFI crate table are accepted, the same
                // as the `ipe rust install` reader.
                "[rust.dependencies]" | "[\"rust.dependencies\"]" => "[rust.dependencies]",
                _ => "other",
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
            ("" | "[project]", "name") => raw.name = Some(val.to_owned()),
            ("" | "[project]", "version") => raw.version_str = Some(val.to_owned()),
            ("[source]", "root") => raw.src_rel = Some(val.to_owned()),
            ("[database]", "driver") => raw.driver_str = Some(val.to_owned()),
            ("[rust]", "static") => raw.rust_static = Some(val.to_owned()),
            ("[rust]", "target") => raw.rust_target = Some(val.to_owned()),
            ("[rust]", "allocator") => raw.rust_allocator = Some(val.to_owned()),
            ("[rust]", "allowSlowAllocator") => raw.rust_allow_slow = Some(val.to_owned()),
            ("[wasm]", "mode") => raw.wasm_mode = Some(val.to_owned()),
            ("[wasm]", "entry") => raw.wasm_entry = Some(val.to_owned()),
            ("[wasm]", "mount") => raw.wasm_mount = Some(val.to_owned()),
            ("[wasm]", "publicEnv") => {
                raw.wasm_public_env = parse_string_array("[wasm] publicEnv", raw_val)?;
            }
            ("[wasm]", "optLevel") => raw.wasm_opt_level = Some(val.to_owned()),
            ("[dependencies]", dep) => {
                raw.dependencies
                    .insert(dep.to_owned(), parse_ipe_dep(dep, raw_val)?);
            }
            ("[rust.dependencies]", dep) => {
                raw.rust_dependencies
                    .insert(dep.to_owned(), parse_rust_dep(raw_val));
            }
            ("[capabilities]", "declared") => {
                raw.capabilities = parse_capabilities(raw_val)?;
            }
            _ => {}
        }
    }
    Ok(raw)
}

/// Parse a `ipe.toml` file and return a [`ProjectManifest`].
///
/// The format recognised:
/// ```toml
/// [project]
/// name = "my-app"
///
/// [database]
/// driver = "sqlite"   # or "postgres" — defaults to "sqlite" when absent
///
/// [dependencies]              # Ipê packages (SP3 resolves them)
/// http  = "^1.2"              # from the index, by semver requirement
/// mylib = { git = "…", rev = "…" }
/// local = { path = "../local" }
///
/// [rust.dependencies]         # crates.io crates bound via `ipe rust`
/// uuid = "1.10"
///
/// [capabilities]              # the author-declared capability set
/// declared = ["network", "clock"]
/// ```
/// Lines that start with `#` are comments and are ignored. Unrecognised
/// sections and keys are ignored (forward-compatible). Every section is
/// optional; absent sections yield empty maps / sets.
///
/// # Errors
/// [`CliError::Io`] if the file cannot be read; [`CliError::Usage`] if the
/// manifest is malformed or the `src/` directory does not exist;
/// [`CliError::UsageOwned`] if `[database] driver` names an unsupported value, a
/// dependency version requirement is malformed, or a capability is unknown.
pub fn parse_manifest(manifest_path: &Path) -> Result<ProjectManifest, CliError> {
    let root = manifest_path
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);

    let text = fs::read_to_string(manifest_path).map_err(|e| CliError::Io {
        path: manifest_path.to_path_buf(),
        source: e,
    })?;

    let raw = scan_raw_manifest(&text)?;

    validate_public_env(&raw.wasm_public_env)?;
    let wasm = WasmConfig {
        mode: raw.wasm_mode,
        entry: raw.wasm_entry,
        mount: raw.wasm_mount,
        public_env: raw.wasm_public_env,
        opt_level: raw.wasm_opt_level,
    };

    let name = raw
        .name
        .ok_or(CliError::Usage("ipe.toml: missing a `name = \"…\"` entry"))?;

    // Parse, don't validate: a declared version becomes a typed `semver::Version`
    // here, so a malformed `version = "…"` is a hard manifest-parse error rather
    // than a surprise the enforced-semver check hits later.
    let version = raw
        .version_str
        .map(|v| {
            semver::Version::parse(&v).map_err(|e| {
                CliError::UsageOwned(format!(
                    "ipe.toml: `version = \"{v}\"` is not valid semver: {e}"
                ))
            })
        })
        .transpose()?;

    let src_root = root.join(raw.src_rel.as_deref().unwrap_or("src"));
    if !src_root.is_dir() {
        return Err(CliError::Usage(
            "ipe.toml: the source root directory does not exist",
        ));
    }

    let driver = match raw.driver_str {
        Some(s) => parse_db_driver(&s)?,
        None => ipe_backend_rust::DbDriver::Sqlite,
    };

    // Parse, don't validate: the `[rust]` values become typed request fields
    // here, so a typo'd allocator or bool is a hard error at manifest-parse
    // time — the same posture as `[database] driver` above.
    let static_request = crate::build_plan::StaticRequestLayer {
        static_build: raw
            .rust_static
            .map(|v| crate::build_plan::parse_bool("ipe.toml: [rust] static", &v))
            .transpose()?,
        target: raw.rust_target,
        allocator: raw
            .rust_allocator
            .map(|v| crate::build_plan::AllocatorChoice::parse(&v))
            .transpose()?,
        allow_slow_allocator: raw
            .rust_allow_slow
            .map(|v| crate::build_plan::parse_bool("ipe.toml: [rust] allowSlowAllocator", &v))
            .transpose()?,
    };

    Ok(ProjectManifest {
        name,
        version,
        root,
        src_root,
        driver,
        static_request,
        wasm,
        dependencies: raw.dependencies,
        rust_dependencies: raw.rust_dependencies,
        capabilities: raw.capabilities,
    })
}

/// Parse one `[dependencies]` value into a typed [`IpeDep`]. A bare string is a
/// semver requirement (index dependency); an inline table with a `git` or `path`
/// key is the corresponding escape.
///
/// Parse, don't validate: an index version requirement is turned into a
/// [`semver::VersionReq`] here, so a malformed version is a manifest-parse error
/// naming `dep`, never a resolution-time surprise.
///
/// # Errors
/// [`CliError::UsageOwned`] on a malformed version requirement, an inline table
/// missing both `git` and `path`, or an unrecognised value shape.
fn parse_ipe_dep(dep: &str, raw_val: &str) -> Result<IpeDep, CliError> {
    if let Some(body) = raw_val
        .strip_prefix('{')
        .and_then(|rest| rest.strip_suffix('}'))
    {
        if let Some(url) = inline_table_string(body, "git") {
            let rev = inline_table_string(body, "rev");
            return Ok(IpeDep::Git { url, rev });
        }
        if let Some(path) = inline_table_string(body, "path") {
            return Ok(IpeDep::Path(PathBuf::from(path)));
        }
        return Err(CliError::UsageOwned(format!(
            "ipe.toml: [dependencies] {dep} inline table must carry a `git` or `path` key"
        )));
    }
    let version = raw_val.trim_matches('"');
    let req = version.parse::<semver::VersionReq>().map_err(|e| {
        CliError::UsageOwned(format!(
            "ipe.toml: [dependencies] {dep} = {version:?} is not a valid version requirement: {e}"
        ))
    })?;
    Ok(IpeDep::Index(req))
}

/// Render an [`IpeDep`] as the TOML value it is written as under
/// `[dependencies]`: a bare string for the index requirement, an inline table
/// for the git and path escapes. The inverse of [`parse_ipe_dep`].
#[must_use]
pub fn render_ipe_dep(dep: &IpeDep) -> String {
    match dep {
        IpeDep::Index(req) => format!("\"{req}\""),
        IpeDep::Git { url, rev } => rev.as_ref().map_or_else(
            || format!("{{ git = \"{url}\" }}"),
            |rev| format!("{{ git = \"{url}\", rev = \"{rev}\" }}"),
        ),
        IpeDep::Path(path) => format!("{{ path = \"{}\" }}", path.display()),
    }
}

/// Upsert `name = <dep>` into the manifest's `[dependencies]` section.
///
/// Every other section and line is preserved. When the key already exists its
/// line is replaced in place; when the section is absent it is appended.
///
/// The manifest is edited textually rather than reserialized so a hand-authored
/// manifest keeps its comments, ordering, and formatting — only the one
/// dependency line changes.
///
/// # Errors
/// [`CliError::Io`] if the manifest cannot be read or written.
pub fn upsert_dependency(manifest_path: &Path, name: &str, dep: &IpeDep) -> Result<(), CliError> {
    let text = read_manifest_text(manifest_path)?;
    let line = format!("{name} = {}", render_ipe_dep(dep));
    let updated = edit_dependency_section(&text, name, Some(&line));
    write_manifest_text(manifest_path, &updated)
}

/// Remove the `name = …` line from the manifest's `[dependencies]` section, if
/// present. Every other line is preserved.
///
/// # Errors
/// [`CliError::Io`] if the manifest cannot be read or written.
pub fn remove_dependency(manifest_path: &Path, name: &str) -> Result<(), CliError> {
    let text = read_manifest_text(manifest_path)?;
    let updated = edit_dependency_section(&text, name, None);
    write_manifest_text(manifest_path, &updated)
}

/// Read the manifest file's text, mapping an IO failure to [`CliError::Io`].
fn read_manifest_text(manifest_path: &Path) -> Result<String, CliError> {
    fs::read_to_string(manifest_path).map_err(|e| CliError::Io {
        path: manifest_path.to_path_buf(),
        source: e,
    })
}

/// Write the manifest file's text, mapping an IO failure to [`CliError::Io`].
fn write_manifest_text(manifest_path: &Path, text: &str) -> Result<(), CliError> {
    fs::write(manifest_path, text).map_err(|e| CliError::Io {
        path: manifest_path.to_path_buf(),
        source: e,
    })
}

/// Edit the `[dependencies]` section of a manifest's `text`: replace or insert
/// `replacement` for the key `name` when `replacement` is `Some`, or drop the
/// key's line when `None`. Returns the whole edited manifest text.
///
/// A single scan tracks whether the cursor is inside `[dependencies]`; the key's
/// existing line (if any) is replaced or dropped there. When the section exists
/// but the key does not, an insert line is added at the section's end; when the
/// section is absent entirely, it is appended with the new line.
fn edit_dependency_section(text: &str, name: &str, replacement: Option<&str>) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut in_deps = false;
    let mut saw_section = false;
    let mut handled = false;
    // The index in `out` just past the last line of `[dependencies]`, so an
    // insert lands at the section's end rather than after unrelated sections.
    let mut section_end: Option<usize> = None;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if in_deps {
                section_end = Some(out.len());
            }
            in_deps = trimmed == "[dependencies]";
            if in_deps {
                saw_section = true;
            }
            out.push(line.to_owned());
            continue;
        }
        if in_deps && key_of(trimmed) == Some(name) {
            handled = true;
            if let Some(replacement) = replacement {
                out.push(replacement.to_owned());
            }
            // A `None` replacement drops the line by not pushing it.
            continue;
        }
        out.push(line.to_owned());
    }
    if in_deps {
        section_end = Some(out.len());
    }

    if let Some(replacement) = replacement.filter(|_| !handled) {
        insert_into_dependencies(&mut out, saw_section, section_end, replacement);
    }

    let mut joined = out.join("\n");
    if text.ends_with('\n') && !joined.ends_with('\n') {
        joined.push('\n');
    }
    joined
}

/// Insert `replacement` into the `[dependencies]` section of the accumulated
/// `out` lines, creating the section when it is absent.
fn insert_into_dependencies(
    out: &mut Vec<String>,
    saw_section: bool,
    section_end: Option<usize>,
    replacement: &str,
) {
    if let (true, Some(at)) = (saw_section, section_end) {
        out.insert(at, replacement.to_owned());
    } else {
        if out.last().is_some_and(|l| !l.trim().is_empty()) {
            out.push(String::new());
        }
        out.push("[dependencies]".to_owned());
        out.push(replacement.to_owned());
    }
}

/// The `key` of a `key = value` manifest line, trimmed; `None` for a line that
/// is not a key assignment (a comment, a blank, a bare table header).
fn key_of(trimmed: &str) -> Option<&str> {
    if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('[') {
        return None;
    }
    trimmed.split_once('=').map(|(k, _)| k.trim())
}

/// Parse one `[rust.dependencies]` value into a [`RustDep`]. A bare string is a
/// version requirement; an inline table carries an optional `version` and
/// `features` list.
fn parse_rust_dep(raw_val: &str) -> RustDep {
    let inline_body = raw_val
        .strip_prefix('{')
        .and_then(|rest| rest.strip_suffix('}'));
    inline_body.map_or_else(
        || RustDep {
            version: raw_val.trim_matches('"').to_owned(),
            features: Vec::new(),
        },
        |body| RustDep {
            version: inline_table_string(body, "version").unwrap_or_default(),
            features: inline_table_string_array(body, "features"),
        },
    )
}

/// Parse the `[capabilities] declared = ["…", …]` array into a typed set, via
/// [`Capability::from_str`]. An unknown capability name is a loud, named error —
/// a typo can never become a silently-dropped capability the sandbox then fails
/// to enforce.
///
/// # Errors
/// [`CliError::UsageOwned`] naming an unrecognised capability, or a malformed
/// array.
fn parse_capabilities(raw_val: &str) -> Result<BTreeSet<Capability>, CliError> {
    let names = parse_string_array("[capabilities] declared", raw_val)?;
    let mut set = BTreeSet::new();
    for name in names {
        let cap = name
            .parse::<Capability>()
            .map_err(|e| CliError::UsageOwned(format!("ipe.toml: [capabilities] {e}")))?;
        set.insert(cap);
    }
    Ok(set)
}

/// Read `key = "value"` out of an inline-table body (`git = "…"`, `version =
/// "…"`). Whole-word key match, so `version` never matches inside a longer key.
fn inline_table_string(body: &str, key: &str) -> Option<String> {
    let at = find_inline_key(body, key)?;
    let rest = body.get(at..)?;
    let (_, after_eq) = rest.split_once('=')?;
    let after_quote = after_eq.trim_start().strip_prefix('"')?;
    after_quote.split_once('"').map(|(v, _)| v.to_owned())
}

/// Read `key = ["a", "b"]` out of an inline-table body.
fn inline_table_string_array(body: &str, key: &str) -> Vec<String> {
    let Some(at) = find_inline_key(body, key) else {
        return Vec::new();
    };
    let Some(rest) = body.get(at..) else {
        return Vec::new();
    };
    let Some((_, after_eq)) = rest.split_once('=') else {
        return Vec::new();
    };
    let Some(after_bracket) = after_eq.trim_start().strip_prefix('[') else {
        return Vec::new();
    };
    let Some((inner, _)) = after_bracket.split_once(']') else {
        return Vec::new();
    };
    inner
        .split(',')
        .map(|s| s.trim().trim_matches('"').to_owned())
        .filter(|s| !s.is_empty())
        .collect()
}

/// The byte offset of `key` as a whole word in an inline-table body, so a short
/// key (`rev`) never matches inside a longer one.
fn find_inline_key(body: &str, key: &str) -> Option<usize> {
    let bytes = body.as_bytes();
    let mut from = 0;
    while let Some(rel) = body.get(from..)?.find(key) {
        let at = from + rel;
        let before_ok = at == 0
            || bytes
                .get(at.wrapping_sub(1))
                .is_none_or(|b| !b.is_ascii_alphanumeric() && *b != b'_');
        let after = at + key.len();
        let after_ok = bytes
            .get(after)
            .is_none_or(|b| !b.is_ascii_alphanumeric() && *b != b'_');
        if before_ok && after_ok {
            return Some(at);
        }
        from = at + key.len();
    }
    None
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
            // source, e.g. `Ipe.String`): leave it kernel-resolved.
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
/// ```ipe
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
        let tmp = std::env::temp_dir().join(format!("ipec_project_{test_name}"));
        let _ = fs::remove_dir_all(&tmp);
        let src = tmp.join("src");
        fs::create_dir_all(&src).expect("create src/");
        fs::write(
            src.join("Main.ipe"),
            "module Main exposing (main)\nmain = 0\n",
        )
        .expect("write Main.ipe");
        let toml_path = tmp.join("ipe.toml");
        fs::write(
            &toml_path,
            format!("[project]\nname = \"test\"\n{database_section}"),
        )
        .expect("write ipe.toml");
        toml_path
    }

    /// No `[database]` section at all →
    /// the manifest defaults to `DbDriver::Sqlite`, matching the documented
    /// `ipe.toml` schema default.
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
                "module Main exposing (main)\nmain = 0\n".to_owned(),
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
    /// `[wasm] publicEnv` is a BUILD error at `ipe.toml` parse time, never a
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

    #[test]
    fn absent_sections_leave_the_new_maps_empty() {
        // Back-compat: a manifest with none of the SP2 sections parses with
        // empty dependency maps and capability set.
        let toml_path = write_manifest("sp2_absent", "");
        let m = parse_manifest(&toml_path).expect("bare manifest must parse");
        assert!(m.dependencies.is_empty());
        assert!(m.rust_dependencies.is_empty());
        assert!(m.capabilities.is_empty());
        let _ = fs::remove_dir_all(toml_path.parent().expect("has parent"));
    }

    #[test]
    fn parses_index_git_and_path_dependencies() {
        let section = "[dependencies]\n\
             http = \"^1.2\"\n\
             mylib = { git = \"https://example.com/mylib.git\", rev = \"abc123\" }\n\
             local = { path = \"../local\" }\n";
        let toml_path = write_manifest("sp2_deps", section);
        let m = parse_manifest(&toml_path).expect("dependency section must parse");
        let http = m.dependencies.get("http");
        assert!(
            matches!(http, Some(IpeDep::Index(req)) if req.matches(&semver::Version::new(1, 5, 0))),
            "http should be an Index dep admitting 1.5, got {http:?}"
        );
        assert_eq!(
            m.dependencies.get("mylib"),
            Some(&IpeDep::Git {
                url: "https://example.com/mylib.git".to_owned(),
                rev: Some("abc123".to_owned()),
            })
        );
        assert_eq!(
            m.dependencies.get("local"),
            Some(&IpeDep::Path(PathBuf::from("../local")))
        );
        let _ = fs::remove_dir_all(toml_path.parent().expect("has parent"));
    }

    #[test]
    fn parses_rust_dependencies_bare_and_inline_table() {
        let section = "[rust.dependencies]\n\
             uuid = \"1.10\"\n\
             stripe = { version = \"=1.0.0\", features = [\"blocking\", \"webhooks\"] }\n";
        let toml_path = write_manifest("sp2_rust_deps", section);
        let m = parse_manifest(&toml_path).expect("rust.dependencies must parse");
        let uuid = m.rust_dependencies.get("uuid").expect("uuid present");
        assert_eq!(uuid.version, "1.10");
        assert!(uuid.features.is_empty());
        let stripe = m.rust_dependencies.get("stripe").expect("stripe present");
        assert_eq!(stripe.version, "=1.0.0");
        assert_eq!(stripe.features, vec!["blocking", "webhooks"]);
        let _ = fs::remove_dir_all(toml_path.parent().expect("has parent"));
    }

    #[test]
    fn parses_capabilities_into_a_typed_set() {
        let toml_path = write_manifest(
            "sp2_caps",
            "[capabilities]\ndeclared = [\"network\", \"clock\"]\n",
        );
        let m = parse_manifest(&toml_path).expect("capabilities must parse");
        assert_eq!(
            m.capabilities,
            BTreeSet::from([Capability::Network, Capability::Clock])
        );
        let _ = fs::remove_dir_all(toml_path.parent().expect("has parent"));
    }

    #[test]
    fn an_unknown_capability_is_a_named_error() {
        let toml_path = write_manifest("sp2_bad_cap", "[capabilities]\ndeclared = [\"netwrok\"]\n");
        let err = parse_manifest(&toml_path).expect_err("a typo'd capability must be rejected");
        assert!(
            err.to_string().contains("netwrok"),
            "the error must name the bad capability: {err}"
        );
        let _ = fs::remove_dir_all(toml_path.parent().expect("has parent"));
    }

    #[test]
    fn a_malformed_version_req_is_a_named_error() {
        let toml_path = write_manifest("sp2_bad_ver", "[dependencies]\nhttp = \"not a version\"\n");
        let err = parse_manifest(&toml_path).expect_err("a malformed version must be rejected");
        assert!(
            err.to_string().contains("http"),
            "the error must name the offending dependency: {err}"
        );
        let _ = fs::remove_dir_all(toml_path.parent().expect("has parent"));
    }

    // ── WasmConfig::implies_wasm_target ──────────────────────────────────────

    #[test]
    fn implies_wasm_target_spa_and_hydrate_are_on() {
        assert!(
            WasmConfig {
                mode: Some("spa".to_owned()),
                ..Default::default()
            }
            .implies_wasm_target(),
            "mode=spa must imply wasm target"
        );
        assert!(
            WasmConfig {
                mode: Some("hydrate".to_owned()),
                ..Default::default()
            }
            .implies_wasm_target(),
            "mode=hydrate must imply wasm target"
        );
    }

    #[test]
    fn implies_wasm_target_off_and_absent_are_native() {
        assert!(
            !WasmConfig {
                mode: Some("off".to_owned()),
                ..Default::default()
            }
            .implies_wasm_target(),
            "mode=off must not imply wasm target"
        );
        assert!(
            !WasmConfig::default().implies_wasm_target(),
            "absent mode (None) must not imply wasm target"
        );
    }
}
