//! `ipe migrate config` — render an existing `ipe.toml` into an equivalent
//! `package.ipe`.
//!
//! The command is mechanical: it reads the legacy `ipe.toml` through the same
//! line-scanner every other reader uses ([`crate::project::parse_toml_manifest`]),
//! obtaining a typed [`ProjectManifest`], then renders that struct back through
//! the `Ipe.Package` builder vocabulary as `package.ipe` source text. It never
//! infers, prompts, or edits the source — it is a pure `ProjectManifest -> String`
//! projection followed by a guarded write.
//!
//! # Round-trip guarantee
//!
//! Every field is rendered as a blessed builder over string / nullary-constructor
//! literals, exactly the shape [`crate::package_manifest::read_package_manifest`]
//! accepts. Reading the emitted `package.ipe` back yields the identical
//! `ProjectManifest`, so migration is lossless — a round-trip property test pins
//! this (see this module's tests).
//!
//! # Safety
//!
//! The command refuses to clobber an existing `package.ipe` unless `--force`, and
//! refuses cleanly when there is no `ipe.toml` to migrate. It leaves the
//! `ipe.toml` in place; deleting it is a deliberate later step the author takes.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::CliError;
use crate::project::{IpeDep, ProjectManifest};

/// `ipe migrate config [--force]` — render the current project's `ipe.toml` into
/// an equivalent `package.ipe`.
///
/// # Errors
/// [`CliError::UsageOwned`] on an unknown flag or a second positional;
/// [`CliError::Usage`] when there is no `ipe.toml` to migrate, or when a
/// `package.ipe` already exists and `--force` was not given;
/// [`CliError::Io`] on a read/write failure; the line-scanner's own errors when
/// the `ipe.toml` is malformed.
pub fn run_migrate(rest: &[String]) -> Result<(), CliError> {
    match rest.split_first() {
        Some((sub, tail)) if sub == "config" => run_migrate_config(tail),
        Some((sub, _)) => Err(crate::cli_args::usage_unknown_subcommand(
            "migrate", sub, "`config`",
        )),
        None => Err(CliError::Usage("usage: ipe migrate config [--force]")),
    }
}

/// Parse `migrate config`'s flags and perform the render + guarded write in the
/// current directory.
fn run_migrate_config(rest: &[String]) -> Result<(), CliError> {
    let mut force = false;
    for arg in rest {
        match arg.as_str() {
            "--force" => force = true,
            other => return Err(crate::cli_args::usage_unknown_flag("migrate config", other)),
        }
    }

    let toml_path = PathBuf::from(crate::project::IPE_TOML);
    if !toml_path.is_file() {
        return Err(CliError::Usage(
            "ipe migrate config: no ipe.toml found in the current directory to migrate",
        ));
    }
    let package_path = PathBuf::from(crate::package_manifest::PACKAGE_IPE);
    if package_path.is_file() && !force {
        return Err(CliError::Usage(
            "ipe migrate config: package.ipe already exists — pass --force to overwrite it \
             (the existing ipe.toml is never deleted)",
        ));
    }

    let manifest = crate::project::parse_toml_manifest(&toml_path)?;
    // The wrapper's path / expose / capabilities live in the raw `[rust.wrapper]`
    // section, not on `ProjectManifest` (which carries only `has_rust_wrapper`),
    // so they are read verbatim here to render the wrapper stage faithfully.
    let wrapper_text =
        crate::io_bounded::read_to_string_capped(&toml_path, crate::io_bounded::MANIFEST_READ_CAP)?;
    let wrapper = crate::ffi::rust_wrapper_from_manifest(&wrapper_text);
    let rendered = render_package_ipe(&manifest, wrapper.as_ref());
    write_package_ipe(&package_path, &rendered)?;

    println!(
        "migrated {} -> {} (the ipe.toml is left in place; delete it once you are satisfied)",
        toml_path.display(),
        package_path.display()
    );
    Ok(())
}

/// Write the rendered `package.ipe`, mapping an IO failure to [`CliError::Io`].
fn write_package_ipe(path: &Path, text: &str) -> Result<(), CliError> {
    std::fs::write(path, text).map_err(|e| CliError::Io {
        path: path.to_path_buf(),
        source: e,
    })
}

/// Render a [`ProjectManifest`] as `package.ipe` source text.
///
/// The inverse of [`crate::package_manifest::read_package_manifest`]: every set
/// field becomes a blessed `Package.*` / `Wasm.*` / `Rust.*` pipeline stage over
/// literal arguments, in a fixed order. Only fields that are actually set are
/// emitted — an absent section (an empty dep map, a default `WasmConfig`, an
/// unset static knob) produces no stage, so a minimal manifest renders as a bare
/// `Package.named`. The output re-parses to the identical struct.
#[must_use]
pub fn render_package_ipe(
    m: &ProjectManifest,
    wrapper: Option<&crate::ffi::RawWrapperTable>,
) -> String {
    let mut stages: Vec<String> = Vec::new();

    // `named` is required and is always the pipeline head.
    stages.push(format!("Package.named {}", string_lit(&m.name)));

    if let Some(version) = &m.version {
        stages.push(format!(
            "Package.version {}",
            string_lit(&version.to_string())
        ));
    }

    // The source root is only rendered when it diverges from the default `src`,
    // so a default project stays minimal. `src_root` is an absolute path
    // (`root.join(rel)`); recover the relative segment for the literal.
    if let Some(rel) = source_root_rel(m)
        && rel != "src"
    {
        stages.push(format!("Package.sourceRoot {}", string_lit(&rel)));
    }

    // The driver is a nullary constructor; sqlite is the default and is not
    // rendered (an absent stage defaults to sqlite in the reader).
    match m.driver {
        ipe_backend_rust::DbDriver::Sqlite => {}
        ipe_backend_rust::DbDriver::Postgres => {
            stages.push("Package.database Package.postgres".to_owned());
        }
    }

    if !m.dependencies.is_empty() {
        stages.push(render_dependencies(m));
    }
    if !m.rust_dependencies.is_empty() {
        stages.push(render_rust_dependencies(m));
    }

    // The wrapper stage is rendered from the raw `[rust.wrapper]` table only when
    // the manifest declared one (`has_rust_wrapper`); its path / expose / caps are
    // not on `ProjectManifest`, so the caller supplies them.
    if m.has_rust_wrapper
        && let Some(w) = wrapper
    {
        stages.push(render_wrapper(w));
    }

    let sr = &m.static_request;
    if let Some(on) = sr.static_build {
        stages.push(format!("Package.static {}", switch_lit(on)));
    }
    if let Some(target) = &sr.target {
        stages.push(format!("Package.target {}", string_lit(target)));
    }
    if let Some(alloc) = sr.allocator {
        stages.push(format!("Package.allocator {}", allocator_ctor(alloc)));
    }
    if let Some(on) = sr.allow_slow_allocator {
        stages.push(format!("Package.allowSlowAllocator {}", switch_lit(on)));
    }
    if let Some(on) = sr.c_free {
        stages.push(format!("Package.cFree {}", switch_lit(on)));
    }

    if !m.capabilities.is_empty() {
        stages.push(format!(
            "Package.declares {}",
            capability_list(&m.capabilities)
        ));
    }
    if !m.capabilities_accept.is_empty() {
        stages.push(format!(
            "Package.accepts {}",
            capability_list(&m.capabilities_accept)
        ));
    }

    if let Some(wasm) = render_wasm(&m.wasm) {
        stages.push(wasm);
    }

    assemble(&stages)
}

/// The project's source root as a path relative to the project root, or `None`
/// when it cannot be expressed relative to the root (it always can for a manifest
/// this crate produced, where `src_root = root.join(rel)`).
fn source_root_rel(m: &ProjectManifest) -> Option<String> {
    m.src_root
        .strip_prefix(&m.root)
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

/// Assemble the ordered pipeline stages into a `package.ipe` module. The head is
/// the first stage; every later stage is threaded with `|>` on its own indented
/// line — the exact spine [`crate::package_manifest`] linearises.
fn assemble(stages: &[String]) -> String {
    let mut out = String::from("module Package exposing (package)\n\n\npackage =\n");
    let mut iter = stages.iter();
    if let Some(head) = iter.next() {
        let _ = writeln!(out, "    {head}");
    }
    for stage in iter {
        let _ = writeln!(out, "        |> {stage}");
    }
    out
}

/// Render the `Package.dependencies [ … ]` stage: one dependency builder per
/// element, in the map's (sorted) key order. Each [`IpeDep`] variant maps to its
/// distinct builder (`dep` / `depGit` / `depGitRev` / `depPath`).
fn render_dependencies(m: &ProjectManifest) -> String {
    let elems: Vec<String> = m
        .dependencies
        .iter()
        .map(|(name, dep)| render_one_dep(name, dep))
        .collect();
    format!("Package.dependencies\n{}", indented_list(&elems, 3))
}

/// Render one dependency as its blessed builder call.
fn render_one_dep(name: &str, dep: &IpeDep) -> String {
    match dep {
        IpeDep::Index(req) => format!(
            "Package.dep {} {}",
            string_lit(name),
            string_lit(&req.to_string())
        ),
        IpeDep::Git { url, rev: None } => {
            format!("Package.depGit {} {}", string_lit(name), string_lit(url))
        }
        IpeDep::Git {
            url,
            rev: Some(rev),
        } => format!(
            "Package.depGitRev {} {} {}",
            string_lit(name),
            string_lit(url),
            string_lit(rev)
        ),
        IpeDep::Path(path) => format!(
            "Package.depPath {} {}",
            string_lit(name),
            string_lit(&path.to_string_lossy())
        ),
    }
}

/// Render the `Package.rustDependencies [ … ]` stage: a `Package.rustDep name
/// version` per element, threaded through `|> Rust.features [ … ]` when the
/// crate requests features.
fn render_rust_dependencies(m: &ProjectManifest) -> String {
    let elems: Vec<String> = m
        .rust_dependencies
        .iter()
        .map(|(name, dep)| {
            let head = format!(
                "Package.rustDep {} {}",
                string_lit(name),
                string_lit(&dep.version)
            );
            if dep.features.is_empty() {
                head
            } else {
                format!("{head} |> Rust.features {}", string_list(&dep.features))
            }
        })
        .collect();
    format!("Package.rustDependencies\n{}", indented_list(&elems, 3))
}

/// Render the `Package.wrapper ( Rust.wrapper "…" |> Rust.expose [ … ] |>
/// Rust.wrapperCaps [ … ] )` stage from the raw `[rust.wrapper]` table. The
/// `expose` and `wrapperCaps` refinements are emitted only when non-empty; the
/// shared wrapper gate (which the reader re-runs) requires a non-empty expose, so
/// a table that declared none renders `Rust.expose []` verbatim and the reader
/// rejects it exactly as the `ipe.toml` path would.
fn render_wrapper(w: &crate::ffi::RawWrapperTable) -> String {
    let mut sub: Vec<String> = vec![format!("Rust.wrapper {}", string_lit(&w.path))];
    sub.push(format!("Rust.expose {}", string_list(&w.expose)));
    if !w.capabilities.is_empty() {
        let caps: Vec<String> = w
            .capabilities
            .iter()
            .map(|c| format!("Capability.{}", capability_builder_name(c)))
            .collect();
        sub.push(format!("Rust.wrapperCaps {}", inline_list(&caps)));
    }
    let mut inner = String::new();
    let mut iter = sub.iter();
    if let Some(head) = iter.next() {
        inner.push_str(head);
    }
    for s in iter {
        let _ = write!(inner, " |> {s}");
    }
    format!("Package.wrapper ({inner})")
}

/// Render the `Package.wasm ( … )` stage, or `None` when the config is the
/// default (mode-off, no fields) — an absent stage yields `WasmConfig::default()`
/// in the reader.
fn render_wasm(wasm: &crate::project::WasmConfig) -> Option<String> {
    // A default WasmConfig round-trips as no stage at all.
    if wasm == &crate::project::WasmConfig::default() {
        return None;
    }

    let mut sub: Vec<String> = Vec::new();
    // The mode is the head atom. `Wasm.spa` / `Wasm.hydrate` are the two the
    // vocabulary names; any other mode (including an explicit "off") has no
    // constructor and is dropped to the default, which the reader treats
    // identically — so the mode head is emitted only for spa / hydrate.
    match wasm.mode.as_deref() {
        Some("spa") => sub.push("Wasm.spa".to_owned()),
        Some("hydrate") => sub.push("Wasm.hydrate".to_owned()),
        _ => {}
    }
    if let Some(entry) = &wasm.entry {
        sub.push(format!("Wasm.entry {}", string_lit(entry)));
    }
    if let Some(mount) = &wasm.mount {
        sub.push(format!("Wasm.mount {}", string_lit(mount)));
    }
    if !wasm.public_env.is_empty() {
        sub.push(format!("Wasm.publicEnv {}", string_list(&wasm.public_env)));
    }
    if let Some(level) = &wasm.opt_level {
        sub.push(format!("Wasm.optLevel {}", string_lit(level)));
    }

    // A wasm config whose only content is a non-spa/hydrate mode leaves `sub`
    // empty; there is nothing the vocabulary can represent, so emit no stage.
    if sub.is_empty() {
        return None;
    }

    // Thread the sub-pipeline inline: `( head |> a |> b )`.
    let mut inner = String::new();
    let mut iter = sub.iter();
    if let Some(head) = iter.next() {
        inner.push_str(head);
    }
    for s in iter {
        let _ = write!(inner, " |> {s}");
    }
    Some(format!("Package.wasm ({inner})"))
}

/// Render a `BTreeSet<Capability>` as a `[ Capability.foo, … ]` list, each
/// element the blessed `Capability.<builder>` constructor for the capability's
/// wire name.
fn capability_list(caps: &std::collections::BTreeSet<crate::project::Capability>) -> String {
    let elems: Vec<String> = caps
        .iter()
        .map(|c| format!("Capability.{}", capability_builder_name(c.as_str())))
        .collect();
    inline_list(&elems)
}

/// The `Capability.<builder>` suffix for a capability wire name — the inverse of
/// the reader's `capability_wire_name`. Hyphenated wire names (`native-ffi`,
/// `ffi-raw`) become their camelCase builder spelling; every other wire name is
/// already a valid builder suffix and passes through.
fn capability_builder_name(wire: &str) -> String {
    match wire {
        "native-ffi" => "nativeFfi".to_owned(),
        "ffi-raw" => "ffiRaw".to_owned(),
        other => other.to_owned(),
    }
}

/// The blessed `Package.<ctor>` allocator constructor for an [`AllocatorChoice`].
const fn allocator_ctor(choice: crate::build_plan::AllocatorChoice) -> &'static str {
    use crate::build_plan::AllocatorChoice as A;
    match choice {
        A::Auto => "Package.autoAlloc",
        A::System => "Package.system",
        A::Dlmalloc => "Package.dlmalloc",
        A::Talc => "Package.talc",
        A::Mimalloc => "Package.mimalloc",
    }
}

/// The blessed on/off switch constructor for a boolean field.
const fn switch_lit(on: bool) -> &'static str {
    if on { "Static.on" } else { "Static.off" }
}

/// A `[ "a", "b", … ]` list of string literals, rendered inline.
fn string_list(items: &[String]) -> String {
    let elems: Vec<String> = items.iter().map(|s| string_lit(s)).collect();
    inline_list(&elems)
}

/// Render `elems` as an inline list literal `[ a, b, c ]` (or `[]` when empty).
fn inline_list(elems: &[String]) -> String {
    if elems.is_empty() {
        return "[]".to_owned();
    }
    format!("[ {} ]", elems.join(", "))
}

/// Render `elems` as a multi-line list literal, Elm-style: each element on its
/// own line, the first opening the bracket and subsequent ones comma-led, closed
/// by a trailing `]`. `indent` is the number of 4-space levels for the bracket
/// column. Used for the dependency lists, which read better one-per-line.
fn indented_list(elems: &[String], indent: usize) -> String {
    let pad = "    ".repeat(indent);
    if elems.is_empty() {
        return format!("{pad}[]");
    }
    let mut out = String::new();
    for (i, elem) in elems.iter().enumerate() {
        let lead = if i == 0 { '[' } else { ',' };
        let _ = writeln!(out, "{pad}{lead} {elem}");
    }
    let _ = write!(out, "{pad}]");
    out
}

/// Encode a string as an Ipê double-quoted string literal, escaping exactly the
/// escapes the lexer resolves (`\\ \" \n \t \r \0`). Every other character passes
/// through verbatim, so the literal decodes back to the original string — the
/// escaping rule is the inverse of the lexer's `push_escape`.
fn string_lit(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\0' => out.push_str("\\0"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
#[allow(clippy::panic)] // a rendered manifest that fails to re-read IS the test failure
mod tests {
    use super::*;
    use crate::package_manifest::read_package_manifest;
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::PathBuf;

    /// A fresh project root with a minimal `src/Main.ipe`, so the reader's
    /// source-root existence check passes when it re-reads the rendered manifest.
    fn fresh_root(test_name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("ipe_migrate_{test_name}"));
        let _ = std::fs::remove_dir_all(&root);
        let src = root.join("src");
        std::fs::create_dir_all(&src).expect("create src/");
        std::fs::write(
            src.join("Main.ipe"),
            "module Main exposing (main)\nmain = 0\n",
        )
        .expect("write Main.ipe");
        root
    }

    /// Write `toml_body` as an `ipe.toml` under a fresh root and parse it via the
    /// legacy line-scanner, returning `(root, ProjectManifest)`. Any wrapper table
    /// is re-read from the toml text at render time (see [`assert_round_trips`]).
    fn parse_toml(test_name: &str, toml_body: &str) -> (PathBuf, ProjectManifest) {
        let root = fresh_root(test_name);
        let toml_path = root.join(crate::project::IPE_TOML);
        std::fs::write(&toml_path, toml_body).expect("write ipe.toml");
        let manifest =
            crate::project::parse_toml_manifest(&toml_path).expect("ipe.toml must parse");
        (root, manifest)
    }

    /// The round-trip crux: render `a` to `package.ipe`, read it back with the P1
    /// reader against `root`, and assert the readback equals `a` field-for-field.
    /// A field that does not survive is a renderer bug. The wrapper table (if any)
    /// is re-read from the on-disk `ipe.toml` under `root`, mirroring the command.
    fn assert_round_trips(root: &Path, a: &ProjectManifest) {
        let wrapper = std::fs::read_to_string(root.join(crate::project::IPE_TOML))
            .ok()
            .and_then(|t| crate::ffi::rust_wrapper_from_manifest(&t));
        let rendered = render_package_ipe(a, wrapper.as_ref());
        let pkg_path = root.join(crate::package_manifest::PACKAGE_IPE);
        let b = read_package_manifest(&rendered, root, &pkg_path).unwrap_or_else(|e| {
            panic!("rendered package.ipe must re-read; error: {e}\n--- rendered ---\n{rendered}")
        });
        assert_manifests_eq(a, &b, &rendered);
    }

    /// Compare the two manifests on every field the manifest surface carries. The
    /// `root` / `src_root` absolute paths are identical by construction (both use
    /// the same project root), so they are compared too.
    fn assert_manifests_eq(a: &ProjectManifest, b: &ProjectManifest, rendered: &str) {
        let ctx = |field: &str| {
            format!("field `{field}` did not round-trip\n--- rendered ---\n{rendered}")
        };
        assert_eq!(a.name, b.name, "{}", ctx("name"));
        assert_eq!(a.version, b.version, "{}", ctx("version"));
        assert_eq!(a.root, b.root, "{}", ctx("root"));
        assert_eq!(a.src_root, b.src_root, "{}", ctx("src_root"));
        assert_eq!(a.driver, b.driver, "{}", ctx("driver"));
        assert_eq!(
            a.static_request,
            b.static_request,
            "{}",
            ctx("static_request")
        );
        assert_eq!(a.wasm, b.wasm, "{}", ctx("wasm"));
        assert_eq!(a.dependencies, b.dependencies, "{}", ctx("dependencies"));
        assert_eq!(
            a.rust_dependencies,
            b.rust_dependencies,
            "{}",
            ctx("rust_dependencies")
        );
        assert_eq!(a.capabilities, b.capabilities, "{}", ctx("capabilities"));
        assert_eq!(
            a.capabilities_accept,
            b.capabilities_accept,
            "{}",
            ctx("capabilities_accept")
        );
        assert_eq!(
            a.has_rust_wrapper,
            b.has_rust_wrapper,
            "{}",
            ctx("has_rust_wrapper")
        );
    }

    // ── The field-matrix round-trip fixtures ──────────────────────────────────

    #[test]
    fn round_trip_minimal() {
        // Only `name` — every other field defaults. Renders as a bare head.
        let (root, m) = parse_toml("minimal", "[project]\nname = \"my-app\"\n");
        assert_round_trips(&root, &m);
    }

    #[test]
    fn round_trip_name_and_version() {
        let (root, m) = parse_toml(
            "name_version",
            "[project]\nname = \"my-app\"\nversion = \"1.2.3\"\n",
        );
        assert_eq!(m.version, Some(semver::Version::new(1, 2, 3)));
        assert_round_trips(&root, &m);
    }

    #[test]
    fn round_trip_postgres_driver() {
        let (root, m) = parse_toml(
            "postgres",
            "[project]\nname = \"db-app\"\n[database]\ndriver = \"postgres\"\n",
        );
        assert_eq!(m.driver, ipe_backend_rust::DbDriver::Postgres);
        assert_round_trips(&root, &m);
    }

    #[test]
    fn round_trip_every_dependency_kind() {
        let (root, m) = parse_toml(
            "deps",
            "[project]\nname = \"deps-app\"\n\
             [dependencies]\n\
             ipe-http = \"^1.2\"\n\
             ipe-widgets = { git = \"https://example.test/w.git\", rev = \"a1b2c3\" }\n\
             ipe-plain = { git = \"https://example.test/p.git\" }\n\
             ipe-local = { path = \"../local\" }\n",
        );
        assert_eq!(m.dependencies.len(), 4);
        assert!(matches!(
            m.dependencies.get("ipe-widgets"),
            Some(IpeDep::Git { rev: Some(_), .. })
        ));
        assert!(matches!(
            m.dependencies.get("ipe-plain"),
            Some(IpeDep::Git { rev: None, .. })
        ));
        assert_round_trips(&root, &m);
    }

    #[test]
    fn round_trip_rust_dependencies_with_features() {
        let (root, m) = parse_toml(
            "rustdeps",
            "[project]\nname = \"ffi-app\"\n\
             [rust.dependencies]\n\
             uuid = \"1.10\"\n\
             image = { version = \"0.25\", features = [\"png\", \"jpeg\"] }\n",
        );
        assert_eq!(m.rust_dependencies.len(), 2);
        assert_eq!(
            m.rust_dependencies.get("image").map(|d| d.features.clone()),
            Some(vec!["png".to_owned(), "jpeg".to_owned()])
        );
        assert_round_trips(&root, &m);
    }

    #[test]
    fn round_trip_static_build_knobs() {
        let (root, m) = parse_toml(
            "static",
            "[project]\nname = \"static-app\"\n\
             [rust]\n\
             static = \"true\"\n\
             target = \"x86_64-unknown-linux-musl\"\n\
             allocator = \"dlmalloc\"\n\
             allowSlowAllocator = \"false\"\n\
             cFree = \"true\"\n",
        );
        assert_eq!(m.static_request.static_build, Some(true));
        assert_eq!(
            m.static_request.allocator,
            Some(crate::build_plan::AllocatorChoice::Dlmalloc)
        );
        assert_round_trips(&root, &m);
    }

    #[test]
    fn round_trip_every_allocator_choice() {
        // The allocator is a closed enum; every variant that an ipe.toml can name
        // must survive migration through the vocabulary.
        for (wire, choice) in [
            ("auto", crate::build_plan::AllocatorChoice::Auto),
            ("system", crate::build_plan::AllocatorChoice::System),
            ("dlmalloc", crate::build_plan::AllocatorChoice::Dlmalloc),
            ("talc", crate::build_plan::AllocatorChoice::Talc),
            ("mimalloc", crate::build_plan::AllocatorChoice::Mimalloc),
        ] {
            let (root, m) = parse_toml(
                &format!("alloc_{wire}"),
                &format!("[project]\nname = \"a\"\n[rust]\nallocator = \"{wire}\"\n"),
            );
            assert_eq!(m.static_request.allocator, Some(choice));
            assert_round_trips(&root, &m);
        }
    }

    #[test]
    fn round_trip_multiple_capabilities() {
        let (root, m) = parse_toml(
            "caps",
            "[project]\nname = \"cap-app\"\n\
             [capabilities]\n\
             declared = [\"network\", \"clock\", \"native-ffi\", \"ffi-raw\"]\n\
             accept = [\"unsafe\"]\n",
        );
        assert_eq!(m.capabilities.len(), 4);
        assert!(
            m.capabilities_accept
                .contains(&crate::project::Capability::Unsafe)
        );
        assert_round_trips(&root, &m);
    }

    #[test]
    fn round_trip_wasm_with_public_env() {
        let (root, m) = parse_toml(
            "wasm",
            "[project]\nname = \"web-app\"\n\
             [wasm]\n\
             mode = \"spa\"\n\
             entry = \"src/Client.ipe\"\n\
             mount = \"#app\"\n\
             publicEnv = [\"API_BASE_URL\", \"APP_VERSION\"]\n\
             optLevel = \"z\"\n",
        );
        assert_eq!(m.wasm.mode.as_deref(), Some("spa"));
        assert_eq!(m.wasm.public_env, vec!["API_BASE_URL", "APP_VERSION"]);
        assert_round_trips(&root, &m);
    }

    #[test]
    fn round_trip_wasm_hydrate_mode() {
        let (root, m) = parse_toml(
            "wasm_hydrate",
            "[project]\nname = \"ssr-app\"\n[wasm]\nmode = \"hydrate\"\nentry = \"src/Client.ipe\"\n",
        );
        assert_eq!(m.wasm.mode.as_deref(), Some("hydrate"));
        assert_round_trips(&root, &m);
    }

    #[test]
    fn round_trip_rust_wrapper() {
        // A `[rust.wrapper]` section sets `has_rust_wrapper`; the renderer must
        // emit a wrapper stage (read from the raw table) so the boolean survives.
        let (root, m) = parse_toml(
            "wrapper",
            "[project]\nname = \"wrap-app\"\n\
             [rust.wrapper]\n\
             path = \"./vendor/mycrate\"\n\
             expose = [\"encode\", \"decode\"]\n\
             capabilities = [\"network\"]\n",
        );
        assert!(m.has_rust_wrapper, "the wrapper section sets the flag");
        assert_round_trips(&root, &m);
    }

    #[test]
    fn round_trip_full_every_section() {
        // Every field the manifest surface carries, set at once — the maximal
        // fixture, including a package-jailed `[rust.wrapper]`.
        let (root, m) = parse_toml(
            "full",
            "[project]\nname = \"kitchen-sink\"\nversion = \"0.3.0\"\n\
             [source]\nroot = \"src\"\n\
             [database]\ndriver = \"postgres\"\n\
             [rust]\n\
             static = \"true\"\n\
             target = \"x86_64-unknown-linux-musl\"\n\
             allocator = \"dlmalloc\"\n\
             allowSlowAllocator = \"false\"\n\
             cFree = \"true\"\n\
             [dependencies]\n\
             ipe-http = \"^1.2\"\n\
             ipe-widgets = { git = \"https://example.test/w.git\", rev = \"a1b2c3\" }\n\
             ipe-plain = { git = \"https://example.test/p.git\" }\n\
             ipe-local = { path = \"../local\" }\n\
             [rust.dependencies]\n\
             uuid = \"1.10\"\n\
             image = { version = \"0.25\", features = [\"png\", \"jpeg\"] }\n\
             [rust.wrapper]\n\
             path = \"./vendor/mycrate\"\n\
             expose = [\"encode\", \"decode\"]\n\
             capabilities = [\"network\"]\n\
             [capabilities]\n\
             declared = [\"network\", \"clock\"]\n\
             accept = [\"unsafe\"]\n\
             [wasm]\n\
             mode = \"spa\"\n\
             entry = \"src/Client.ipe\"\n\
             mount = \"#app\"\n\
             publicEnv = [\"API_BASE_URL\", \"APP_VERSION\"]\n\
             optLevel = \"z\"\n",
        );
        assert!(m.has_rust_wrapper);
        assert_round_trips(&root, &m);
    }

    #[test]
    fn round_trip_string_escapes() {
        // A dependency path carrying characters the lexer escapes must survive the
        // render/parse round-trip verbatim.
        let root = fresh_root("escapes");
        let mut deps = BTreeMap::new();
        deps.insert("weird".to_owned(), IpeDep::Path(PathBuf::from("a\"b\\c/d")));
        let m = ProjectManifest {
            name: "esc-app".to_owned(),
            version: None,
            root: root.clone(),
            src_root: root.join("src"),
            driver: ipe_backend_rust::DbDriver::Sqlite,
            static_request: crate::build_plan::StaticRequestLayer::default(),
            wasm: crate::project::WasmConfig::default(),
            dependencies: deps,
            rust_dependencies: BTreeMap::new(),
            capabilities: BTreeSet::new(),
            capabilities_accept: BTreeSet::new(),
            has_rust_wrapper: false,
        };
        assert_round_trips(&root, &m);
    }

    #[test]
    fn string_lit_escapes_the_lexer_set() {
        assert_eq!(string_lit("plain"), "\"plain\"");
        assert_eq!(string_lit("a\"b"), "\"a\\\"b\"");
        assert_eq!(string_lit("a\\b"), "\"a\\\\b\"");
        assert_eq!(string_lit("a\nb"), "\"a\\nb\"");
    }

    #[test]
    fn minimal_render_is_a_bare_head() {
        let root = fresh_root("bare");
        let m = ProjectManifest {
            name: "solo".to_owned(),
            version: None,
            root: root.clone(),
            src_root: root.join("src"),
            driver: ipe_backend_rust::DbDriver::Sqlite,
            static_request: crate::build_plan::StaticRequestLayer::default(),
            wasm: crate::project::WasmConfig::default(),
            dependencies: BTreeMap::new(),
            rust_dependencies: BTreeMap::new(),
            capabilities: BTreeSet::new(),
            capabilities_accept: BTreeSet::new(),
            has_rust_wrapper: false,
        };
        let rendered = render_package_ipe(&m, None);
        assert!(
            rendered.contains("Package.named \"solo\""),
            "renders the name head: {rendered}"
        );
        assert!(
            !rendered.contains("|>"),
            "a minimal manifest has no pipeline stages: {rendered}"
        );
    }
}
