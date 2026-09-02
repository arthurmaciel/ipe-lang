//! `ipe migrate config` — convert an interim manifest to the record form.
//!
//! The `package.ipe` manifest is a typed inert record (`package : Package`).
//! Two older shapes predate it: the interim `Package.named "…" |> …` pipe-builder
//! `package.ipe`, and the legacy `ipe.toml`. `ipe migrate config` reads whichever
//! it finds in the current directory and rewrites it as the record form, in
//! place, once.
//!
//! The rewrite is lossless over the fields the manifest carries: the interim
//! manifest is read into the same [`ProjectManifest`] the record reader produces,
//! then serialised back with [`crate::package_manifest::render_manifest_record`].
//! A `package.ipe` already in the record form is detected and left untouched (the
//! command is idempotent).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use ipe_intern::Interner;
use ipe_syntax::{Expr, Expr_, Module};

use crate::CliError;
use crate::package_manifest::{PACKAGE_IPE, render_manifest_record};
use crate::project::{
    Capability, EntryShape, IPE_TOML, IpeDep, Program, ProjectManifest, RustDep, WasmConfig,
    is_denylisted_public_env_name,
};

/// `ipe migrate config` — rewrite the current directory's interim manifest as the
/// record form.
///
/// # Errors
/// [`CliError::UsageOwned`] on an unrecognised flag or argument, when no manifest
/// is found, or when the manifest is malformed; [`CliError::Io`] on a filesystem
/// failure.
pub fn run_migrate(rest: &[String]) -> Result<(), CliError> {
    let mut sub: Option<String> = None;
    for arg in rest {
        match arg.as_str() {
            flag if flag.starts_with('-') => {
                return Err(crate::cli_args::usage_unknown_flag("migrate", flag));
            }
            positional if sub.is_none() => sub = Some(positional.to_owned()),
            other => return Err(crate::cli_args::usage_unexpected_argument("migrate", other)),
        }
    }
    match sub.as_deref() {
        Some("config") | None => migrate_config(Path::new(".")),
        Some(other) => Err(CliError::UsageOwned(format!(
            "migrate: unknown subcommand {other:?} — the only subcommand is `config`"
        ))),
    }
}

/// Migrate the manifest in `dir`, writing the record form in place.
fn migrate_config(dir: &Path) -> Result<(), CliError> {
    let package_ipe = dir.join(PACKAGE_IPE);
    if package_ipe.is_file() {
        return migrate_package_ipe(&package_ipe);
    }
    let ipe_toml = dir.join(IPE_TOML);
    if ipe_toml.is_file() {
        return migrate_ipe_toml(dir, &ipe_toml, &package_ipe);
    }
    Err(CliError::Usage(
        "migrate config: no package.ipe or ipe.toml in this directory — nothing to migrate",
    ))
}

/// Migrate an existing `package.ipe`. If it already reads as the record form,
/// leave it untouched (idempotent). Otherwise read it as the interim
/// pipe-builder form and rewrite it as a record.
fn migrate_package_ipe(path: &Path) -> Result<(), CliError> {
    let root = path
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let text =
        crate::io_bounded::read_to_string_capped(path, crate::io_bounded::MANIFEST_READ_CAP)?;

    if crate::package_manifest::read_package_manifest(&text, &root, path).is_ok() {
        println!(
            "{} is already in the record form — nothing to migrate",
            path.display()
        );
        return Ok(());
    }

    let manifest = read_interim_builder(&text, &root, path)?;
    let record = render_manifest_record(&manifest);
    write_manifest(path, &record)?;
    println!("migrated {} to the record form", path.display());
    Ok(())
}

/// Migrate a legacy `ipe.toml` into a new `package.ipe` record. The TOML is read
/// with the line-scanner into a [`ProjectManifest`], serialised as a record, and
/// written to `package.ipe`; the `ipe.toml` is left in place for the author to
/// remove.
fn migrate_ipe_toml(root: &Path, toml_path: &Path, out_path: &Path) -> Result<(), CliError> {
    let text =
        crate::io_bounded::read_to_string_capped(toml_path, crate::io_bounded::MANIFEST_READ_CAP)?;
    let manifest = read_legacy_toml(&text, root)?;
    let record = render_manifest_record(&manifest);
    write_manifest(out_path, &record)?;
    println!(
        "migrated {} to {} (the record form) — you can now remove {}",
        toml_path.display(),
        out_path.display(),
        toml_path.display()
    );
    Ok(())
}

/// Write manifest source to `path`.
fn write_manifest(path: &Path, contents: &str) -> Result<(), CliError> {
    std::fs::write(path, contents).map_err(|e| CliError::Io {
        path: path.to_path_buf(),
        source: e,
    })
}

// ── interim pipe-builder reader (migration input only) ─────────────────────────

/// Read the interim `Package.named "…" |> …` pipe-builder manifest into a
/// [`ProjectManifest`]. Used ONLY by `migrate config` to read an old manifest for
/// re-serialisation; the live reader is the record reader. Kept deliberately
/// narrow: it recognises exactly the builder vocabulary the interim form used.
fn read_interim_builder(src: &str, root: &Path, path: &Path) -> Result<ProjectManifest, CliError> {
    let mut interner = Interner::new();
    let module =
        ipe_parse::parse_module(src, &mut interner).map_err(|diag| CliError::Pipeline {
            file: path.to_path_buf(),
            src: src.to_owned(),
            diag: Box::new(diag),
        })?;
    let reader = BuilderReader {
        interner: &interner,
    };
    reader.read(&module, root)
}

/// The interim-builder reader's borrowed context.
struct BuilderReader<'a> {
    interner: &'a Interner,
}

/// Fields accumulated while walking the interim pipeline.
#[derive(Default)]
struct BuilderFields {
    name: Option<String>,
    version: Option<semver::Version>,
    dependencies: BTreeMap<String, IpeDep>,
    rust_dependencies: BTreeMap<String, RustDep>,
    capabilities: BTreeSet<Capability>,
    capabilities_accept: BTreeSet<Capability>,
    wasm: WasmConfig,
    programs: Vec<Program>,
    exposed_modules: Vec<String>,
}

impl BuilderReader<'_> {
    fn text(&self, sym: ipe_intern::Symbol) -> &str {
        self.interner.resolve(sym).unwrap_or("")
    }

    fn read(&self, module: &Module, root: &Path) -> Result<ProjectManifest, CliError> {
        let package = module
            .values
            .iter()
            .find(|v| self.text(v.value.name.value) == "package")
            .ok_or_else(|| oops("no top-level `package = …` binding to migrate"))?;
        let stages = linearise(&package.value.body);
        let mut fields = BuilderFields::default();
        for stage in stages {
            self.apply_stage(stage, &mut fields)?;
        }
        let name = fields
            .name
            .ok_or_else(|| oops("the manifest has no `Package.named` stage"))?;
        Ok(ProjectManifest {
            name,
            version: fields.version,
            root: root.to_path_buf(),
            src_root: root.join("src"),
            driver: ipe_backend_rust::DbDriver::Sqlite,
            static_request: crate::build_plan::StaticRequestLayer::default(),
            wasm: fields.wasm,
            dependencies: fields.dependencies,
            rust_dependencies: fields.rust_dependencies,
            capabilities: fields.capabilities,
            capabilities_accept: fields.capabilities_accept,
            has_rust_wrapper: false,
            programs: fields.programs,
            exposed_modules: fields.exposed_modules,
        })
    }

    /// A qualified call `Module.name args`, or a bare `Module.name` atom.
    fn call<'e>(&self, expr: &'e Expr) -> Option<(&str, &str, &'e [Expr])> {
        match &expr.value {
            Expr_::Call(callee, args) => match &callee.value {
                Expr_::VarQual(m, n) => Some((self.text(*m), self.text(*n), args.as_slice())),
                _ => None,
            },
            Expr_::VarQual(m, n) => Some((self.text(*m), self.text(*n), &[])),
            _ => None,
        }
    }

    fn apply_stage(&self, stage: &Expr, fields: &mut BuilderFields) -> Result<(), CliError> {
        let Some((module, name, args)) = self.call(stage) else {
            return Err(oops(
                "an interim manifest stage is not a recognised builder call",
            ));
        };
        match (module, name) {
            ("Package", "named") => fields.name = Some(str_arg(args, 0)?),
            ("Package", "version") => {
                let raw = str_arg(args, 0)?;
                fields.version = Some(
                    semver::Version::parse(&raw)
                        .map_err(|e| oops(&format!("bad version {raw:?}: {e}")))?,
                );
            }
            ("Package", "dependencies") => {
                fields.dependencies = self.read_deps(args.first())?;
            }
            ("Package", "rustDependencies") => {
                fields.rust_dependencies = self.read_rust_deps(args.first())?;
            }
            ("Package", "declares") => {
                fields.capabilities = self.read_caps(args.first())?;
            }
            ("Package", "accepts") => {
                fields.capabilities_accept = self.read_caps(args.first())?;
            }
            ("Package", "exposedModules") => {
                fields.exposed_modules = read_str_list(args.first())?;
            }
            ("Package", "wasm") => {
                fields.wasm = self.read_wasm(args.first())?;
            }
            ("Package", "programs") => {
                fields.programs = self.read_programs(args.first())?;
            }
            // Fields with no wild-manifest instances (sourceRoot / database /
            // static / target / allocator / wrapper) are rare; migrate the common
            // ones and report the rest so nothing is silently dropped.
            _ => {
                return Err(oops(&format!(
                    "the interim stage `{module}.{name}` has no automatic migration — rewrite it \
                     by hand into the `package.ipe` record"
                )));
            }
        }
        Ok(())
    }

    fn read_deps(&self, expr: Option<&Expr>) -> Result<BTreeMap<String, IpeDep>, CliError> {
        let mut deps = BTreeMap::new();
        for item in list_items(expr)? {
            let Some((m, builder, args)) = self.call(item) else {
                return Err(oops("a dependency is not a recognised builder call"));
            };
            if m != "Package" {
                return Err(oops("a dependency builder must be a `Package.*` call"));
            }
            let (name, dep) = match builder {
                "dep" => {
                    let name = str_arg(args, 0)?;
                    let raw = str_arg(args, 1)?;
                    let req = raw
                        .parse::<semver::VersionReq>()
                        .map_err(|e| oops(&format!("bad requirement {raw:?}: {e}")))?;
                    (name, IpeDep::Index(req))
                }
                "depGit" => (
                    str_arg(args, 0)?,
                    IpeDep::Git {
                        url: str_arg(args, 1)?,
                        rev: None,
                    },
                ),
                "depGitRev" => (
                    str_arg(args, 0)?,
                    IpeDep::Git {
                        url: str_arg(args, 1)?,
                        rev: Some(str_arg(args, 2)?),
                    },
                ),
                "depPath" => (
                    str_arg(args, 0)?,
                    IpeDep::Path(PathBuf::from(str_arg(args, 1)?)),
                ),
                other => return Err(oops(&format!("unknown dependency builder {other:?}"))),
            };
            deps.insert(name, dep);
        }
        Ok(deps)
    }

    fn read_rust_deps(&self, expr: Option<&Expr>) -> Result<BTreeMap<String, RustDep>, CliError> {
        let mut deps = BTreeMap::new();
        for item in list_items(expr)? {
            let stages = linearise(item);
            let mut name: Option<String> = None;
            let mut dep = RustDep::default();
            for stage in stages {
                let Some((m, builder, args)) = self.call(stage) else {
                    return Err(oops("a rust dependency is not a recognised builder call"));
                };
                match (m, builder) {
                    ("Package", "rustDep") => {
                        name = Some(str_arg(args, 0)?);
                        dep.version = str_arg(args, 1)?;
                    }
                    ("Rust", "features") => {
                        dep.features = read_str_list(args.first())?;
                    }
                    _ => return Err(oops("unknown rust-dependency builder")),
                }
            }
            let name = name.ok_or_else(|| oops("a rust dependency has no `Package.rustDep`"))?;
            deps.insert(name, dep);
        }
        Ok(deps)
    }

    fn read_caps(&self, expr: Option<&Expr>) -> Result<BTreeSet<Capability>, CliError> {
        let mut set = BTreeSet::new();
        for item in list_items(expr)? {
            let Some((m, cap, _)) = self.call(item) else {
                return Err(oops("a capability is not a `Capability.*` reference"));
            };
            if m != "Capability" {
                return Err(oops("a capability must be a `Capability.*` reference"));
            }
            let wire = interim_capability_wire(cap);
            let parsed = wire
                .parse::<Capability>()
                .map_err(|e| oops(&format!("unknown capability: {e}")))?;
            set.insert(parsed);
        }
        Ok(set)
    }

    fn read_wasm(&self, expr: Option<&Expr>) -> Result<WasmConfig, CliError> {
        let head = expr.ok_or_else(|| oops("`Package.wasm` has no argument"))?;
        let stages = linearise(head);
        let mut wasm = WasmConfig::default();
        for stage in stages {
            // A bare `Wasm.spa` / `Wasm.hydrate` head atom, or a refinement call.
            let Some((m, builder, args)) = self.call(stage) else {
                return Err(oops("a wasm stage is not a recognised builder"));
            };
            match (m, builder) {
                ("Wasm", "spa") => wasm.mode = Some("spa".to_owned()),
                ("Wasm", "hydrate") => wasm.mode = Some("hydrate".to_owned()),
                ("Wasm", "entry") => wasm.entry = Some(str_arg(args, 0)?),
                ("Wasm", "mount") => wasm.mount = Some(str_arg(args, 0)?),
                ("Wasm", "publicEnv") => {
                    let names = read_str_list(args.first())?;
                    for n in &names {
                        if is_denylisted_public_env_name(n) {
                            return Err(oops(&format!(
                                "`Wasm.publicEnv` lists {n:?}, which matches the secret denylist"
                            )));
                        }
                    }
                    wasm.public_env = names;
                }
                ("Wasm", "optLevel") => wasm.opt_level = Some(str_arg(args, 0)?),
                _ => return Err(oops("unknown wasm builder")),
            }
        }
        Ok(wasm)
    }

    fn read_programs(&self, expr: Option<&Expr>) -> Result<Vec<Program>, CliError> {
        let mut programs = Vec::new();
        for item in list_items(expr)? {
            let stages = linearise(item);
            let mut name: Option<String> = None;
            let mut entry: Option<String> = None;
            let mut shape: Option<EntryShape> = None;
            for stage in stages {
                let Some((m, builder, args)) = self.call(stage) else {
                    return Err(oops("a program is not a recognised builder"));
                };
                match (m, builder) {
                    ("Program", "named") => name = Some(str_arg(args, 0)?),
                    ("Program", "entry") => entry = Some(str_arg(args, 0)?),
                    ("Program", "shape") => {
                        shape = Some(self.read_shape(args.first())?);
                    }
                    _ => return Err(oops("unknown program builder")),
                }
            }
            let name = name.ok_or_else(|| oops("a program has no `Program.named`"))?;
            programs.push(Program {
                name,
                entry: entry.unwrap_or_else(|| "Main.ipe".to_owned()),
                shape,
            });
        }
        Ok(programs)
    }

    fn read_shape(&self, expr: Option<&Expr>) -> Result<EntryShape, CliError> {
        let Some(e) = expr else {
            return Err(oops("`Program.shape` has no argument"));
        };
        let Some((m, name, _)) = self.call(e) else {
            return Err(oops("a shape is not a `Shape.*` reference"));
        };
        if m != "Shape" {
            return Err(oops("a shape must be a `Shape.*` reference"));
        }
        match name {
            "web" => Ok(EntryShape::Web),
            "webView" => Ok(EntryShape::WebView),
            "terminal" => Ok(EntryShape::Terminal),
            "program" => Ok(EntryShape::Program),
            other => Err(oops(&format!("unknown shape {other:?}"))),
        }
    }
}

/// A `migrate config: <reason>` rejection for a malformed interim manifest.
fn oops(reason: &str) -> CliError {
    CliError::UsageOwned(format!("migrate config: {reason}"))
}

/// Linearise a `|>` spine into its ordered stages; a bare head is one stage.
fn linearise(body: &Expr) -> Vec<&Expr> {
    match &body.value {
        Expr_::Binops(ops, last) => {
            let mut stages: Vec<&Expr> = ops.iter().map(|(operand, _)| operand).collect();
            stages.push(last.as_ref());
            stages
        }
        _ => vec![body],
    }
}

/// Read the `idx`-th argument of a call as a string literal.
fn str_arg(args: &[Expr], idx: usize) -> Result<String, CliError> {
    match args.get(idx).map(|e| &e.value) {
        Some(Expr_::Str(s)) => Ok(s.clone()),
        _ => Err(oops(
            "expected a string literal argument in the interim manifest",
        )),
    }
}

/// The element expressions of a list-literal argument, or a rejection.
fn list_items(expr: Option<&Expr>) -> Result<&[Expr], CliError> {
    match expr.map(|e| &e.value) {
        Some(Expr_::List(items)) => Ok(items.as_slice()),
        _ => Err(oops("expected a list literal")),
    }
}

/// Read a list-literal argument as a `Vec<String>` of its string-literal elements.
fn read_str_list(expr: Option<&Expr>) -> Result<Vec<String>, CliError> {
    list_items(expr)?
        .iter()
        .map(|item| match &item.value {
            Expr_::Str(s) => Ok(s.clone()),
            _ => Err(oops("expected a string in a list")),
        })
        .collect()
}

/// The wire name of an interim `Capability.<builder>` reference (the interim form
/// used the same camelCase suffixes the record form's constructors do).
fn interim_capability_wire(builder: &str) -> &str {
    match builder {
        "nativeFfi" => "native-ffi",
        "ffiRaw" => "ffi-raw",
        "customElement" => "custom-element",
        "jsPort" => "js-port",
        other => other,
    }
}

// ── legacy ipe.toml reader (migration input only) ──────────────────────────────

/// Read a legacy `ipe.toml` into a [`ProjectManifest`] for re-serialisation.
///
/// A deliberately small line/section scanner over the historical `ipe.toml`
/// shape: `[project] name/version`, `[database] driver`, `[dependencies]`,
/// `[capabilities] declared/accept`, `[wasm]`. It reads only the fields the
/// record form can round-trip; a section it does not recognise is ignored (its
/// keys are reported by the record reader on the next build if they mattered).
fn read_legacy_toml(text: &str, root: &Path) -> Result<ProjectManifest, CliError> {
    let table: toml::Value = text.parse::<toml::Value>().map_err(|e| {
        CliError::UsageOwned(format!("migrate config: ipe.toml is not valid TOML: {e}"))
    })?;

    let oops = |reason: &str| CliError::UsageOwned(format!("migrate config: {reason}"));

    let project = table.get("project").and_then(toml::Value::as_table);
    let name = project
        .and_then(|p| p.get("name"))
        .and_then(toml::Value::as_str)
        .ok_or_else(|| oops("ipe.toml has no `[project] name`"))?
        .to_owned();
    let version = match project
        .and_then(|p| p.get("version"))
        .and_then(toml::Value::as_str)
    {
        Some(v) => Some(semver::Version::parse(v).map_err(|e| {
            oops(&format!(
                "ipe.toml `[project] version` {v:?} is invalid: {e}"
            ))
        })?),
        None => None,
    };

    let driver = match table
        .get("database")
        .and_then(toml::Value::as_table)
        .and_then(|d| d.get("driver"))
        .and_then(toml::Value::as_str)
    {
        Some("postgres") => ipe_backend_rust::DbDriver::Postgres,
        Some("sqlite") | None => ipe_backend_rust::DbDriver::Sqlite,
        Some(other) => {
            return Err(oops(&format!(
                "ipe.toml has an unknown database driver {other:?}"
            )));
        }
    };

    let dependencies = read_toml_deps(table.get("dependencies"), &oops)?;
    let capabilities = read_toml_caps(&table, "declared", &oops)?;
    let capabilities_accept = read_toml_caps(&table, "accept", &oops)?;

    Ok(ProjectManifest {
        name,
        version,
        root: root.to_path_buf(),
        src_root: root.join("src"),
        driver,
        static_request: crate::build_plan::StaticRequestLayer::default(),
        wasm: WasmConfig::default(),
        dependencies,
        rust_dependencies: BTreeMap::new(),
        capabilities,
        capabilities_accept,
        has_rust_wrapper: false,
        programs: Vec::new(),
        exposed_modules: Vec::new(),
    })
}

/// Read the `[dependencies]` table of a legacy `ipe.toml`.
fn read_toml_deps(
    section: Option<&toml::Value>,
    oops: &impl Fn(&str) -> CliError,
) -> Result<BTreeMap<String, IpeDep>, CliError> {
    let mut deps = BTreeMap::new();
    let Some(table) = section.and_then(toml::Value::as_table) else {
        return Ok(deps);
    };
    for (name, value) in table {
        let dep = match value {
            toml::Value::String(req_raw) => {
                let req = req_raw.parse::<semver::VersionReq>().map_err(|e| {
                    oops(&format!("dependency {name:?} requirement is invalid: {e}"))
                })?;
                IpeDep::Index(req)
            }
            toml::Value::Table(t) => {
                if let Some(git) = t.get("git").and_then(toml::Value::as_str) {
                    let rev = t
                        .get("rev")
                        .and_then(toml::Value::as_str)
                        .map(str::to_owned);
                    IpeDep::Git {
                        url: git.to_owned(),
                        rev,
                    }
                } else if let Some(path) = t.get("path").and_then(toml::Value::as_str) {
                    IpeDep::Path(PathBuf::from(path))
                } else {
                    return Err(oops(&format!(
                        "dependency {name:?} is neither an index requirement, a git, nor a path"
                    )));
                }
            }
            _ => {
                return Err(oops(&format!(
                    "dependency {name:?} has an unexpected shape"
                )));
            }
        };
        deps.insert(name.clone(), dep);
    }
    Ok(deps)
}

/// Read a `[capabilities] <key> = [ … ]` list of a legacy `ipe.toml`.
fn read_toml_caps(
    table: &toml::Value,
    key: &str,
    oops: &impl Fn(&str) -> CliError,
) -> Result<BTreeSet<Capability>, CliError> {
    let mut set = BTreeSet::new();
    let Some(list) = table
        .get("capabilities")
        .and_then(toml::Value::as_table)
        .and_then(|c| c.get(key))
        .and_then(toml::Value::as_array)
    else {
        return Ok(set);
    };
    for item in list {
        let name = item
            .as_str()
            .ok_or_else(|| oops(&format!("`[capabilities] {key}` must be a list of strings")))?;
        let cap = name
            .parse::<Capability>()
            .map_err(|e| oops(&format!("unknown capability: {e}")))?;
        set.insert(cap);
    }
    Ok(set)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh(test_name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("ipe_migrate_{test_name}"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).expect("create src/");
        std::fs::write(
            root.join("src").join("Main.ipe"),
            "module Main exposing (main)\nmain = 0\n",
        )
        .expect("write Main.ipe");
        root
    }

    #[test]
    fn migrates_a_minimal_interim_builder() {
        let root = fresh("minimal");
        let path = root.join(PACKAGE_IPE);
        std::fs::write(
            &path,
            "module Package exposing (package)\n\npackage =\n    Package.named \"demo\"\n        |> Package.version \"0.2.0\"\n",
        )
        .expect("write");
        migrate_config(&root).expect("migrate");
        let out = std::fs::read_to_string(&path).expect("read back");
        assert!(out.contains("{ name = \"demo\""), "record name: {out}");
        assert!(out.contains("version = \"0.2.0\""), "record version: {out}");
        // The rewritten manifest re-reads through the record reader.
        let m = crate::package_manifest::read_package_manifest(&out, &root, &path)
            .expect("record reads");
        assert_eq!(m.name, "demo");
        assert_eq!(m.version.map(|v| v.to_string()).as_deref(), Some("0.2.0"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn migrates_interim_wasm_and_caps() {
        let root = fresh("wasm_caps");
        let path = root.join(PACKAGE_IPE);
        std::fs::write(
            &path,
            "module Package exposing (package)\n\npackage =\n    Package.named \"w\"\n        |> Package.wasm (Wasm.spa |> Wasm.mount \"#app\")\n        |> Package.accepts [ Capability.unsafe ]\n",
        )
        .expect("write");
        migrate_config(&root).expect("migrate");
        let out = std::fs::read_to_string(&path).expect("read back");
        let m = crate::package_manifest::read_package_manifest(&out, &root, &path)
            .expect("record reads");
        assert_eq!(m.wasm.mode.as_deref(), Some("spa"));
        assert_eq!(m.wasm.mount.as_deref(), Some("#app"));
        assert!(m.capabilities_accept.iter().any(|c| c.as_str() == "unsafe"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn record_form_is_left_untouched() {
        let root = fresh("idempotent");
        let path = root.join(PACKAGE_IPE);
        let record = "module Package exposing (package)\n\nimport Ipe.Package exposing (..)\n\n\npackage : Package\npackage =\n    { name = \"already\"\n    , version = \"1.0.0\"\n    }\n";
        std::fs::write(&path, record).expect("write");
        migrate_config(&root).expect("migrate no-op");
        let out = std::fs::read_to_string(&path).expect("read back");
        assert_eq!(out, record, "an already-record manifest is untouched");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn migrates_a_legacy_ipe_toml() {
        let root = fresh("toml");
        std::fs::write(
            root.join(IPE_TOML),
            "[project]\nname = \"legacy\"\nversion = \"2.1.0\"\n\n[database]\ndriver = \"postgres\"\n\n[dependencies]\nhttp = \"^1.2\"\n",
        )
        .expect("write toml");
        migrate_config(&root).expect("migrate");
        let path = root.join(PACKAGE_IPE);
        let out = std::fs::read_to_string(&path).expect("read back package.ipe");
        let m = crate::package_manifest::read_package_manifest(&out, &root, &path)
            .expect("record reads");
        assert_eq!(m.name, "legacy");
        assert_eq!(m.driver, ipe_backend_rust::DbDriver::Postgres);
        assert!(matches!(m.dependencies.get("http"), Some(IpeDep::Index(_))));
        let _ = std::fs::remove_dir_all(&root);
    }
}
