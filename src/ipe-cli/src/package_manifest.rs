//! `package.ipe` — the project manifest written in Ipê as an inert typed record.
//!
//! The bootstrap constraint is decisive: the toolchain must learn a project's
//! dependencies before it can compile anything, so it cannot evaluate Ipê — that
//! would require the very dependencies it is trying to discover — to read them.
//! The resolution is to read, never run: the manifest binds one top-level
//! `package : Package` value to a record literal, and this reader extracts each
//! field by walking the AST of that record, refusing anything that is not a
//! literal, a closed-set constructor, or a nested record/list of those.
//!
//! # What this reader does
//!
//! It reuses the compiler's own front end — [`ipe_parse::parse_module`] — and
//! nothing past it: no canonicalisation, no name resolution, no type-checking,
//! no lowering, no emit, and above all no evaluation. The parser is total and
//! effect-free by construction, so reading an untrusted `package.ipe` runs none
//! of its code. The reader then operates purely on the resulting AST, producing
//! a [`ProjectManifest`].
//!
//! # The record schema
//!
//! The manifest is a `{ name = "…", version = "…", …, build = { … } }` record
//! typed by the `Ipe.Package` stdlib schema. Every finite choice — the database
//! driver, a program's shape, the allocator, the wasm mode, a capability — is a
//! closed-union constructor (`Sqlite`, `Web`, `Dlmalloc`, `Spa`, `Network`), so
//! a typo is a name that does not exist rather than a live-with-it string. Open
//! text (`version`, an `entry` file, a `Cross` triple) is a `String`, parsed at
//! this read boundary. No field can hold a function or an effect — the record is
//! inert by shape.
//!
//! # The shared record machinery
//!
//! The primitive readers here ([`Reader::expect_record`],
//! [`Reader::expect_string`], [`Reader::expect_ctor`],
//! [`Reader::expect_ctor_app`], [`Reader::expect_list`]) read a typed inert
//! record of closed-set constructors and literals from a parsed AST. They are
//! written to be reused by the FFI `foreign`-record reader, which faces the same
//! shape (a typed inert record read syntactically, never evaluated).
//!
//! # Preserved validations
//!
//! Every parse-time check the manifest must carry runs here against the
//! extracted literal, via the *same* shared functions the rest of the CLI uses —
//! the `publicEnv` secret-name denylist, semver parsing, capability validation,
//! the allocator vocabulary, and the wrapper path jail.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use ipe_diagnostics::{Located, Span};
use ipe_intern::Interner;
use ipe_syntax::{Expr, Expr_, Module};

use ipe_kernels::WebCapability;

use crate::CliError;
use crate::project::{
    Capability, EntryShape, IpeDep, Program, ProjectManifest, RustDep, WasmConfig,
    is_denylisted_public_env_name,
};

/// The manifest filename read by this reader.
pub const PACKAGE_IPE: &str = "package.ipe";

/// The one import a `package.ipe` may carry: the schema its constructors come
/// from. It is permitted and IGNORED — the reader recognises every constructor
/// by name, never by resolving the import — so the manifest type-checks in an
/// editor while the reader stays evaluation-free. No other import is allowed.
const SCHEMA_MODULE: &str = "Ipe.Package";

/// Read and validate a `package.ipe` manifest at `manifest_path`.
///
/// Totality: every input either yields a typed [`ProjectManifest`] or a typed
/// [`CliError`]; there is no code path that evaluates a `package.ipe`
/// expression, and no input can panic the reader.
///
/// # Errors
/// [`CliError::Io`] if the file cannot be read; [`CliError::Pipeline`] if the
/// source does not parse (rendered with a caret snippet); [`CliError::UsageOwned`]
/// naming the offending `package.ipe:LINE:COL` for any non-literal, non-blessed
/// shape or a failed field validation; [`CliError::Usage`] if the source root
/// directory does not exist.
pub fn parse_package_manifest(manifest_path: &Path) -> Result<ProjectManifest, CliError> {
    let root = manifest_path
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let text = crate::io_bounded::read_to_string_capped(
        manifest_path,
        crate::io_bounded::MANIFEST_READ_CAP,
    )?;
    read_package_manifest(&text, &root, manifest_path)
}

/// The total core: `&str -> Result<ProjectManifest, CliError>`, given the
/// project `root` (for path fields) and the manifest path (for diagnostics).
///
/// This is the security boundary. It parses `src` and walks the record of the
/// sole `package` binding; it never evaluates any expression.
///
/// # Errors
/// As [`parse_package_manifest`], minus the file-read error.
pub fn read_package_manifest(
    src: &str,
    root: &Path,
    manifest_path: &Path,
) -> Result<ProjectManifest, CliError> {
    let mut interner = Interner::new();
    let module =
        ipe_parse::parse_module(src, &mut interner).map_err(|diag| CliError::Pipeline {
            file: manifest_path.to_path_buf(),
            src: src.to_owned(),
            diag: Box::new(diag),
        })?;

    let reader = Reader {
        interner: &interner,
        src,
        manifest_path,
    };
    let fields = reader.read_module(&module)?;
    fields.into_manifest(root)
}

/// Borrowed context every walk step shares: the interner to resolve [`Symbol`]s
/// to their text, the source (for `line:col` rendering), and the manifest path
/// (for diagnostic prefixes).
struct Reader<'a> {
    interner: &'a Interner,
    src: &'a str,
    manifest_path: &'a Path,
}

/// The fields accumulated while reading the record, before assembly into a
/// [`ProjectManifest`]. Every field defaults to the same absent-section default
/// the manifest schema documents, so a minimal `{ name = "x" }` yields the same
/// struct a fully-specified record with defaulted sections would.
#[derive(Default)]
struct ManifestFields {
    name: Option<String>,
    version: Option<semver::Version>,
    src_rel: Option<String>,
    icon_rel: Option<String>,
    driver: Option<ipe_backend_rust::DbDriver>,
    static_build: Option<bool>,
    target: Option<String>,
    allocator: Option<crate::build_plan::AllocatorChoice>,
    allow_slow_allocator: Option<bool>,
    c_free: Option<bool>,
    dependencies: BTreeMap<String, IpeDep>,
    rust_dependencies: BTreeMap<String, RustDep>,
    capabilities: BTreeSet<Capability>,
    capabilities_accept: BTreeSet<Capability>,
    wasm: WasmConfig,
    has_rust_wrapper: bool,
    programs: Vec<crate::project::Program>,
    exposed_modules: Vec<String>,
}

impl ManifestFields {
    /// Assemble the accumulated fields into a [`ProjectManifest`], applying the
    /// two remaining whole-manifest validations: `name` is required, and the
    /// source-root directory must exist.
    fn into_manifest(self, root: &Path) -> Result<ProjectManifest, CliError> {
        let name = self.name.ok_or(CliError::Usage(
            "package.ipe: missing a `name = \"…\"` field — a package must be named",
        ))?;
        let src_rel_raw = self.src_rel.as_deref().unwrap_or("src");
        let src_root_contained = crate::contained_path::ContainedRelPath::parse(root, src_rel_raw)
            .map_err(|reason| CliError::PathEscape {
                raw: src_rel_raw.to_owned(),
                reason,
            })?;
        let src_root = src_root_contained.resolved().to_path_buf();
        if !src_root.is_dir() {
            return Err(CliError::Usage(
                "package.ipe: the source root directory does not exist",
            ));
        }
        // The icon is an optional project-relative path resolved (and contained)
        // at parse time, so a packager consumes a validated path and can never be
        // handed one that escapes the project root.
        let icon = self
            .icon_rel
            .as_deref()
            .map(|raw| {
                crate::contained_path::ContainedRelPath::parse(root, raw)
                    .map(|c| c.resolved().to_path_buf())
                    .map_err(|reason| CliError::PathEscape {
                        raw: raw.to_owned(),
                        reason,
                    })
            })
            .transpose()?;
        Ok(ProjectManifest {
            name,
            version: self.version,
            root: root.to_path_buf(),
            src_root,
            icon,
            driver: self.driver.unwrap_or(ipe_backend_rust::DbDriver::Sqlite),
            static_request: crate::build_plan::StaticRequestLayer {
                static_build: self.static_build,
                target: self.target,
                allocator: self.allocator,
                allow_slow_allocator: self.allow_slow_allocator,
                c_free: self.c_free,
            },
            wasm: self.wasm,
            dependencies: self.dependencies,
            rust_dependencies: self.rust_dependencies,
            capabilities: self.capabilities,
            capabilities_accept: self.capabilities_accept,
            has_rust_wrapper: self.has_rust_wrapper,
            programs: self.programs,
            exposed_modules: self.exposed_modules,
        })
    }
}

impl Reader<'_> {
    /// Resolve a [`Symbol`] to its interned text, or `""` when it is somehow
    /// unresolvable (never expected for a symbol the parser produced; the empty
    /// string simply fails every blessed-name match, so an unresolvable name is
    /// rejected rather than matched).
    fn text(&self, sym: ipe_intern::Symbol) -> &str {
        self.interner.resolve(sym).unwrap_or("")
    }

    /// Render a `package.ipe:LINE:COL: <reason>` [`CliError::UsageOwned`] for the
    /// offending `span`. Line/column are 1-based, computed from the byte offset
    /// against the source; an out-of-range offset degrades to `1:1` rather than
    /// panicking (totality).
    fn reject(&self, span: Span, reason: &str) -> CliError {
        let (line, col) = line_col(self.src, span.lo);
        CliError::UsageOwned(format!(
            "{}:{line}:{col}: {reason}",
            self.manifest_path.display()
        ))
    }

    /// Walk a whole parsed module into the accumulated [`ManifestFields`],
    /// enforcing the module-shape rules: only the schema import (ignored),
    /// exactly one top-level `package` value binding with no parameters, and no
    /// other declarations.
    fn read_module(&self, module: &Module) -> Result<ManifestFields, CliError> {
        for import in &module.imports {
            let name = self.import_module_name(import);
            if name != SCHEMA_MODULE {
                return Err(self.reject(
                    import.name.span,
                    &format!(
                        "a package.ipe may import only `{SCHEMA_MODULE}` (the manifest schema) — \
                         the manifest is read before dependencies are resolved, so no other module \
                         can be imported"
                    ),
                ));
            }
        }
        if let Some(union) = module.unions.first() {
            return Err(self.reject(
                union.value.name.span,
                "a package.ipe declares only the `package` value — a `type` declaration is not \
                 allowed",
            ));
        }
        if let Some(alias) = module.aliases.first() {
            return Err(self.reject(
                alias.value.name.span,
                "a package.ipe declares only the `package` value — a `type alias` declaration is \
                 not allowed",
            ));
        }

        let mut package_value: Option<&Located<ipe_syntax::Value>> = None;
        for value in &module.values {
            let vname = self.text(value.value.name.value);
            if vname == "package" {
                if package_value.is_some() {
                    return Err(self.reject(
                        value.value.name.span,
                        "a package.ipe declares the `package` value exactly once",
                    ));
                }
                package_value = Some(value);
            } else {
                return Err(self.reject(
                    value.value.name.span,
                    "a package.ipe declares only the `package` value — an extra top-level binding \
                     is not allowed",
                ));
            }
        }

        let package = package_value.ok_or(CliError::Usage(
            "package.ipe: no top-level `package = …` binding found",
        ))?;
        if !package.value.patterns.is_empty() {
            return Err(self.reject(
                package.value.name.span,
                "`package` must be a value binding, not a function — it takes no parameters",
            ));
        }

        self.read_package_record(&package.value.body)
    }

    /// The dotted module name of an import (`Ipe.Package` → `"Ipe.Package"`),
    /// joining its segments.
    fn import_module_name(&self, import: &ipe_syntax::Import) -> String {
        import
            .name
            .value
            .iter()
            .map(|seg| self.text(*seg))
            .collect::<Vec<_>>()
            .join(".")
    }

    /// Read the top-level `package = { … }` record into the accumulated fields,
    /// dispatching each field by name. An unknown field is a named rejection.
    fn read_package_record(&self, body: &Expr) -> Result<ManifestFields, CliError> {
        let record = self.expect_record(body)?;
        let mut fields = ManifestFields::default();
        for (fname, value) in record {
            match self.text(fname.value) {
                "name" => fields.name = Some(self.expect_string(value)?),
                "version" => {
                    let raw = self.expect_string(value)?;
                    let version = semver::Version::parse(&raw).map_err(|e| {
                        self.reject(
                            value.span,
                            &format!("`version` {raw:?} is not valid semver: {e}"),
                        )
                    })?;
                    fields.version = Some(version);
                }
                "sourceRoot" => fields.src_rel = Some(self.expect_string(value)?),
                "icon" => fields.icon_rel = Some(self.expect_string(value)?),
                "dependencies" => fields.dependencies = self.read_dependencies(value)?,
                "rustDependencies" => {
                    fields.rust_dependencies = self.read_rust_dependencies(value)?;
                }
                "capabilities" => self.read_capabilities_record(value, &mut fields)?,
                "exposedModules" => fields.exposed_modules = self.read_exposed_modules(value)?,
                "programs" => fields.programs = self.read_programs(value)?,
                "wasm" => fields.wasm = self.read_wasm(value)?,
                "wrapper" => fields.has_rust_wrapper = self.read_wrapper(value)?,
                "build" => self.read_build_record(value, &mut fields)?,
                other => {
                    return Err(self.reject(
                        fname.span,
                        &format!(
                            "`{other}` is not a package field — expected one of name, version, \
                             sourceRoot, icon, dependencies, rustDependencies, capabilities, \
                             exposedModules, programs, wasm, wrapper, build"
                        ),
                    ));
                }
            }
        }
        Ok(fields)
    }

    /// Read the `capabilities = { declares = [ … ], accepts = [ … ] }` record.
    fn read_capabilities_record(
        &self,
        expr: &Expr,
        fields: &mut ManifestFields,
    ) -> Result<(), CliError> {
        for (fname, value) in self.expect_record(expr)? {
            match self.text(fname.value) {
                "declares" => fields.capabilities = self.read_capability_set(value)?,
                "accepts" => fields.capabilities_accept = self.read_capability_set(value)?,
                other => {
                    return Err(self.reject(
                        fname.span,
                        &format!(
                            "`{other}` is not a capabilities field — expected `declares` or \
                             `accepts`"
                        ),
                    ));
                }
            }
        }
        Ok(())
    }

    /// Read the `build = { database = …, static = …, … }` record into the
    /// static-request / driver fields.
    fn read_build_record(&self, expr: &Expr, fields: &mut ManifestFields) -> Result<(), CliError> {
        for (fname, value) in self.expect_record(expr)? {
            match self.text(fname.value) {
                "database" => fields.driver = Some(self.read_database(value)?),
                "static" => fields.static_build = Some(self.expect_bool(value)?),
                "target" => fields.target = self.read_target(value)?,
                "allocator" => fields.allocator = Some(self.read_allocator(value)?),
                "allowSlowAllocator" => {
                    fields.allow_slow_allocator = Some(self.expect_bool(value)?);
                }
                "cFree" => fields.c_free = Some(self.expect_bool(value)?),
                other => {
                    return Err(self.reject(
                        fname.span,
                        &format!(
                            "`{other}` is not a build field — expected one of database, static, \
                             target, allocator, allowSlowAllocator, cFree"
                        ),
                    ));
                }
            }
        }
        Ok(())
    }

    // ── shared record primitives (reused by the FFI foreign-record reader) ────

    /// Require `expr` to be a record literal `{ … }`, returning its
    /// `(field-name, value)` pairs. Any other shape is rejected — a manifest
    /// section is data, never a computed value.
    fn expect_record<'e>(
        &self,
        expr: &'e Expr,
    ) -> Result<&'e [(Located<ipe_intern::Symbol>, Expr)], CliError> {
        match &expr.value {
            Expr_::Record(fields) => Ok(fields.as_slice()),
            _ => Err(self.reject(
                expr.span,
                "expected a record literal `{ … }` — a package.ipe section is written as a record \
                 of literals, never computed",
            )),
        }
    }

    /// Read a string-literal argument; reject anything computed.
    fn expect_string(&self, expr: &Expr) -> Result<String, CliError> {
        match &expr.value {
            Expr_::Str(s) => Ok(s.clone()),
            _ => Err(self.reject(
                expr.span,
                "expected a string literal — a package.ipe field may only be written as a \
                 literal, never computed",
            )),
        }
    }

    /// Read a boolean-literal field written as the constructor `True` / `False`.
    /// The parser surfaces `True`/`False` as a bare `VarLocal`; anything else is
    /// rejected.
    fn expect_bool(&self, expr: &Expr) -> Result<bool, CliError> {
        match self.ctor_name(expr) {
            Some("True") => Ok(true),
            Some("False") => Ok(false),
            _ => Err(self.reject(
                expr.span,
                "expected `True` or `False` — a package.ipe boolean setting is a literal",
            )),
        }
    }

    /// Read a list-literal argument, returning its element expressions; reject a
    /// non-list.
    fn expect_list<'e>(&self, expr: &'e Expr) -> Result<&'e [Expr], CliError> {
        match &expr.value {
            Expr_::List(items) => Ok(items.as_slice()),
            _ => Err(self.reject(expr.span, "expected a list literal `[ … ]`")),
        }
    }

    /// Read a `[ "a", "b", … ]` list of string literals.
    fn expect_string_list(&self, expr: &Expr) -> Result<Vec<String>, CliError> {
        self.expect_list(expr)?
            .iter()
            .map(|item| self.expect_string(item))
            .collect()
    }

    /// The unqualified constructor name a bare atom names, if it is one.
    ///
    /// A constructor written unqualified (`Sqlite`, `Web`, `Network` — brought
    /// in by `import Ipe.Package exposing (..)`) is a [`Expr_::VarLocal`]; one
    /// written qualified (`Package.Sqlite`) is a [`Expr_::VarQual`] whose name
    /// segment is returned. Anything else (a call, a literal, a lambda) is `None`.
    fn ctor_name<'e>(&'e self, expr: &Expr) -> Option<&'e str> {
        match &expr.value {
            Expr_::VarLocal(n) | Expr_::VarQual(_, n) => Some(self.text(*n)),
            _ => None,
        }
    }

    /// Require `expr` to be a bare nullary constructor, returning its name.
    /// `what` names the expected kind in the error.
    fn expect_ctor<'e>(&'e self, expr: &Expr, what: &str) -> Result<&'e str, CliError> {
        self.ctor_name(expr).ok_or_else(|| {
            self.reject(
                expr.span,
                &format!(
                    "expected {what} as a constructor, never a computed value or a local binding"
                ),
            )
        })
    }

    /// Require `expr` to be a constructor APPLIED to arguments (`Cross "…"`,
    /// `On { … }`, `Wrapper { … }`), returning `(ctor-name, args)`. A bare
    /// nullary constructor yields an empty argument slice.
    fn expect_ctor_app<'e>(
        &'e self,
        expr: &'e Expr,
        what: &str,
    ) -> Result<(&'e str, &'e [Expr]), CliError> {
        match &expr.value {
            Expr_::Call(callee, args) => {
                let name = self.ctor_name(callee).ok_or_else(|| {
                    self.reject(
                        callee.span,
                        &format!(
                            "expected {what} as a constructor applied to its argument, never a \
                             computed function"
                        ),
                    )
                })?;
                Ok((name, args.as_slice()))
            }
            Expr_::VarLocal(n) | Expr_::VarQual(_, n) => Ok((self.text(*n), &[])),
            _ => Err(self.reject(expr.span, &format!("expected {what} as a constructor"))),
        }
    }

    /// The `idx`-th argument of a constructor application, rejecting too few.
    fn nth_arg<'e>(
        &self,
        span: Span,
        ctor: &str,
        args: &'e [Expr],
        idx: usize,
    ) -> Result<&'e Expr, CliError> {
        args.get(idx)
            .ok_or_else(|| self.reject(span, &format!("`{ctor}` is missing argument #{}", idx + 1)))
    }

    // ── field readers ─────────────────────────────────────────────────────────

    /// Read the `build.database` field: `Sqlite` / `Postgres`.
    fn read_database(&self, expr: &Expr) -> Result<ipe_backend_rust::DbDriver, CliError> {
        match self.expect_ctor(expr, "a database driver")? {
            "Sqlite" => Ok(ipe_backend_rust::DbDriver::Sqlite),
            "Postgres" => Ok(ipe_backend_rust::DbDriver::Postgres),
            other => Err(self.reject(
                expr.span,
                &format!("`{other}` is not a database driver — use `Sqlite` or `Postgres`"),
            )),
        }
    }

    /// Read the `build.target` field: `HostTarget` (unset) or `Cross "<triple>"`
    /// (an explicit cross-compile triple, parsed at resolution into the closed
    /// target set).
    fn read_target(&self, expr: &Expr) -> Result<Option<String>, CliError> {
        let (ctor, args) = self.expect_ctor_app(expr, "a build target")?;
        match ctor {
            "HostTarget" => Ok(None),
            "Cross" => {
                let triple = self.expect_string(self.nth_arg(expr.span, ctor, args, 0)?)?;
                Ok(Some(triple))
            }
            other => Err(self.reject(
                expr.span,
                &format!(
                    "`{other}` is not a build target — use `HostTarget` or `Cross \"<triple>\"`"
                ),
            )),
        }
    }

    /// Read the `build.allocator` field, mapped to its
    /// [`crate::build_plan::AllocatorChoice`] wire name so the closed set stays
    /// the single source of truth.
    fn read_allocator(&self, expr: &Expr) -> Result<crate::build_plan::AllocatorChoice, CliError> {
        let wire = match self.expect_ctor(expr, "an allocator")? {
            "AutoAlloc" => "auto",
            "System" => "system",
            "Dlmalloc" => "dlmalloc",
            "Talc" => "talc",
            "Mimalloc" => "mimalloc",
            other => {
                return Err(self.reject(
                    expr.span,
                    &format!(
                        "`{other}` is not an allocator — use `System`, `Dlmalloc`, `Talc`, \
                         `Mimalloc`, or `AutoAlloc`"
                    ),
                ));
            }
        };
        crate::build_plan::AllocatorChoice::parse(wire)
            .map_err(|e| self.reject(expr.span, &format!("{e}")))
    }

    /// Read the `dependencies = [ … ]` list into the typed [`IpeDep`] map. Each
    /// element is a `dep` / `depGit` / `depGitRev` / `depPath` builder call — the
    /// four-builder shape mirrors the `IpeDep` sum, so a dependency is exactly one
    /// of index / git / path, never a bag of optional keys.
    fn read_dependencies(&self, expr: &Expr) -> Result<BTreeMap<String, IpeDep>, CliError> {
        let mut deps = BTreeMap::new();
        for item in self.expect_list(expr)? {
            let (name, dep) = self.read_one_dep(item)?;
            deps.insert(name, dep);
        }
        Ok(deps)
    }

    /// Read one dependency builder call into its `(name, IpeDep)`.
    fn read_one_dep(&self, expr: &Expr) -> Result<(String, IpeDep), CliError> {
        let (builder, args) = self.expect_ctor_app(expr, "a dependency builder")?;
        match builder {
            "dep" => {
                let name = self.expect_string(self.nth_arg(expr.span, builder, args, 0)?)?;
                let raw = self.expect_string(self.nth_arg(expr.span, builder, args, 1)?)?;
                let req = raw.parse::<semver::VersionReq>().map_err(|e| {
                    self.reject(
                        expr.span,
                        &format!(
                            "dependency `{name}` version requirement {raw:?} is not valid: {e}"
                        ),
                    )
                })?;
                Ok((name, IpeDep::Index(req)))
            }
            "depGit" => {
                let name = self.expect_string(self.nth_arg(expr.span, builder, args, 0)?)?;
                let url = self.expect_string(self.nth_arg(expr.span, builder, args, 1)?)?;
                Ok((name, IpeDep::Git { url, rev: None }))
            }
            "depGitRev" => {
                let name = self.expect_string(self.nth_arg(expr.span, builder, args, 0)?)?;
                let url = self.expect_string(self.nth_arg(expr.span, builder, args, 1)?)?;
                let rev = self.expect_string(self.nth_arg(expr.span, builder, args, 2)?)?;
                Ok((
                    name,
                    IpeDep::Git {
                        url,
                        rev: Some(rev),
                    },
                ))
            }
            "depPath" => {
                let name = self.expect_string(self.nth_arg(expr.span, builder, args, 0)?)?;
                let path = self.expect_string(self.nth_arg(expr.span, builder, args, 1)?)?;
                Ok((name, IpeDep::Path(PathBuf::from(path))))
            }
            other => Err(self.reject(
                expr.span,
                &format!(
                    "`{other}` is not a dependency builder — use `dep`, `depGit`, `depGitRev`, or \
                     `depPath`"
                ),
            )),
        }
    }

    /// Read the `rustDependencies = [ … ]` list into the typed [`RustDep`] map.
    /// Each element is a `rustDep name version` or `rustDepWith name version
    /// [ features ]` builder call.
    fn read_rust_dependencies(&self, expr: &Expr) -> Result<BTreeMap<String, RustDep>, CliError> {
        let mut deps = BTreeMap::new();
        for item in self.expect_list(expr)? {
            let (name, dep) = self.read_one_rust_dep(item)?;
            deps.insert(name, dep);
        }
        Ok(deps)
    }

    /// Read one rust dependency: `rustDep name version` (no features) or
    /// `rustDepWith name version [ features ]`.
    fn read_one_rust_dep(&self, expr: &Expr) -> Result<(String, RustDep), CliError> {
        let (builder, args) = self.expect_ctor_app(expr, "a rust-dependency builder")?;
        match builder {
            "rustDep" => {
                let name = self.expect_string(self.nth_arg(expr.span, builder, args, 0)?)?;
                let version = self.expect_string(self.nth_arg(expr.span, builder, args, 1)?)?;
                Ok((
                    name,
                    RustDep {
                        version,
                        features: Vec::new(),
                    },
                ))
            }
            "rustDepWith" => {
                let name = self.expect_string(self.nth_arg(expr.span, builder, args, 0)?)?;
                let version = self.expect_string(self.nth_arg(expr.span, builder, args, 1)?)?;
                let features =
                    self.expect_string_list(self.nth_arg(expr.span, builder, args, 2)?)?;
                Ok((name, RustDep { version, features }))
            }
            other => Err(self.reject(
                expr.span,
                &format!(
                    "`{other}` is not a rust-dependency builder — use `rustDep` or `rustDepWith`"
                ),
            )),
        }
    }

    /// Read the `wrapper` field: `NoWrapper` (none) or `Wrapper { path, expose,
    /// capabilities }`. A present wrapper is validated through the SAME
    /// [`ipe_ffi::wrapper::WrapperManifest::parse`] gate the rest of the CLI
    /// uses: the wrapper path is package-jailed and every declared capability is
    /// checked against the closed vocabulary. Returns whether a wrapper is
    /// declared.
    fn read_wrapper(&self, expr: &Expr) -> Result<bool, CliError> {
        let (ctor, args) = self.expect_ctor_app(expr, "a wrapper spec")?;
        match ctor {
            "NoWrapper" => Ok(false),
            "Wrapper" => {
                let options = self.nth_arg(expr.span, ctor, args, 0)?;
                let (path, expose, capabilities) = self.read_wrapper_options(options)?;
                // Reuse the wrapper gate verbatim: package-jailed path + closed-
                // vocabulary capabilities. Its Diagnostic is surfaced as the
                // reader's rejection.
                ipe_ffi::wrapper::WrapperManifest::parse(&path, &expose, &capabilities).map_err(
                    |diag| self.reject(options.span, &format!("wrapper rejected: {diag}")),
                )?;
                Ok(true)
            }
            other => Err(self.reject(
                expr.span,
                &format!("`{other}` is not a wrapper spec — use `NoWrapper` or `Wrapper {{ … }}`"),
            )),
        }
    }

    /// Read a `Wrapper { path, expose, capabilities }` options record into its
    /// `(path, expose-names, capability-wire-names)`. `path` is required; the two
    /// lists default to empty.
    fn read_wrapper_options(
        &self,
        expr: &Expr,
    ) -> Result<(String, Vec<String>, Vec<String>), CliError> {
        let mut path: Option<String> = None;
        let mut expose: Vec<String> = Vec::new();
        let mut capabilities: Vec<String> = Vec::new();
        for (fname, value) in self.expect_record(expr)? {
            match self.text(fname.value) {
                "path" => path = Some(self.expect_string(value)?),
                "expose" => expose = self.expect_string_list(value)?,
                "capabilities" => capabilities = self.read_capability_wire_names(value)?,
                other => {
                    return Err(self.reject(
                        fname.span,
                        &format!(
                            "`{other}` is not a wrapper field — expected `path`, `expose`, or \
                             `capabilities`"
                        ),
                    ));
                }
            }
        }
        let path = path.ok_or_else(|| {
            self.reject(expr.span, "a wrapper must set `path = \"<local path>\"`")
        })?;
        Ok((path, expose, capabilities))
    }

    /// Read the `wasm` field: `Off` (no bundle) or `On { mode, entry, mount,
    /// publicEnv, optLevel }`. The `publicEnv` list runs the UNCHANGED secret-name
    /// denylist. Fields other than `mode` default to absent.
    fn read_wasm(&self, expr: &Expr) -> Result<WasmConfig, CliError> {
        let (ctor, args) = self.expect_ctor_app(expr, "a wasm setting")?;
        match ctor {
            "Off" => Ok(WasmConfig::default()),
            "On" => {
                let options = self.nth_arg(expr.span, ctor, args, 0)?;
                self.read_wasm_options(options)
            }
            other => Err(self.reject(
                expr.span,
                &format!("`{other}` is not a wasm setting — use `Off` or `On {{ … }}`"),
            )),
        }
    }

    /// Read an `On { mode, entry, mount, publicEnv, optLevel }` options record.
    /// `mode` is required (an active bundle must name its rendering strategy).
    fn read_wasm_options(&self, expr: &Expr) -> Result<WasmConfig, CliError> {
        let mut wasm = WasmConfig::default();
        let mut saw_mode = false;
        for (fname, value) in self.expect_record(expr)? {
            match self.text(fname.value) {
                "mode" => {
                    wasm.mode = Some(self.read_wasm_mode(value)?);
                    saw_mode = true;
                }
                "entry" => wasm.entry = Some(self.expect_string(value)?),
                "mount" => wasm.mount = Some(self.expect_string(value)?),
                "publicEnv" => {
                    let names = self.expect_string_list(value)?;
                    self.check_public_env(value.span, &names)?;
                    wasm.public_env = names;
                }
                "optLevel" => wasm.opt_level = Some(self.expect_string(value)?),
                other => {
                    return Err(self.reject(
                        fname.span,
                        &format!(
                            "`{other}` is not a wasm field — expected mode, entry, mount, \
                             publicEnv, or optLevel"
                        ),
                    ));
                }
            }
        }
        if !saw_mode {
            return Err(self.reject(
                expr.span,
                "an `On { … }` wasm bundle must set `mode = Spa` or `mode = Hydrate`",
            ));
        }
        Ok(wasm)
    }

    /// Read a `wasm` `mode` field: `Spa` / `Hydrate`, mapped to its wire mode.
    fn read_wasm_mode(&self, expr: &Expr) -> Result<String, CliError> {
        match self.expect_ctor(expr, "a wasm mode")? {
            "Spa" => Ok("spa".to_owned()),
            "Hydrate" => Ok("hydrate".to_owned()),
            other => Err(self.reject(
                expr.span,
                &format!("`{other}` is not a wasm mode — use `Spa` or `Hydrate`"),
            )),
        }
    }

    /// Read the `programs = [ … ]` list into the typed [`Program`] vector. Each
    /// element is a `{ name = "…", entry = "…", shape = … }` record. `entry`
    /// defaults to `Main.ipe`; `shape` is absent unless the record sets one.
    fn read_programs(&self, expr: &Expr) -> Result<Vec<Program>, CliError> {
        let mut programs = Vec::new();
        let mut seen_names: BTreeSet<String> = BTreeSet::new();
        for item in self.expect_list(expr)? {
            let program = self.read_one_program(item)?;
            if !seen_names.insert(program.name.clone()) {
                return Err(self.reject(
                    item.span,
                    &format!(
                        "duplicate program name {:?} — each program's `name` must be unique",
                        program.name
                    ),
                ));
            }
            programs.push(program);
        }
        Ok(programs)
    }

    /// Read one `{ name, entry, shape }` program record into a [`Program`].
    fn read_one_program(&self, expr: &Expr) -> Result<Program, CliError> {
        let mut name: Option<String> = None;
        let mut entry: Option<String> = None;
        let mut shape: Option<EntryShape> = None;
        for (fname, value) in self.expect_record(expr)? {
            match self.text(fname.value) {
                "name" => name = Some(self.expect_string(value)?),
                "entry" => entry = Some(self.expect_string(value)?),
                "shape" => shape = Some(self.read_shape(value)?),
                other => {
                    return Err(self.reject(
                        fname.span,
                        &format!(
                            "`{other}` is not a program field — expected `name`, `entry`, or \
                             `shape`"
                        ),
                    ));
                }
            }
        }
        let name =
            name.ok_or_else(|| self.reject(expr.span, "a program must set `name = \"<name>\"`"))?;
        Ok(Program {
            name,
            entry: entry.unwrap_or_else(|| "Main.ipe".to_owned()),
            shape,
        })
    }

    /// Read a program `shape` field: `Web` / `WebView` / `Terminal` / `Program`,
    /// mapped to its [`EntryShape`].
    fn read_shape(&self, expr: &Expr) -> Result<EntryShape, CliError> {
        match self.expect_ctor(expr, "a program shape")? {
            "Web" => Ok(EntryShape::Web),
            "WebView" => Ok(EntryShape::WebView),
            "Terminal" => Ok(EntryShape::Terminal),
            "Program" => Ok(EntryShape::Program),
            other => Err(self.reject(
                expr.span,
                &format!(
                    "`{other}` is not a shape — use `Web`, `WebView`, `Terminal`, or `Program`"
                ),
            )),
        }
    }

    /// Read the `exposedModules = [ "A", "B.C" ]` list of module-name string
    /// literals. Each name is validated as a dotted sequence of module segments
    /// (`[A-Z][A-Za-z0-9_]*`), so a lowercase or malformed name is a read-time
    /// error rather than an unresolvable export.
    fn read_exposed_modules(&self, expr: &Expr) -> Result<Vec<String>, CliError> {
        let mut modules = Vec::new();
        for item in self.expect_list(expr)? {
            let name = self.expect_string(item)?;
            if !is_module_name(&name) {
                return Err(self.reject(
                    item.span,
                    &format!(
                        "`exposedModules` entry {name:?} is not a valid module name (dotted \
                         segments each matching [A-Z][A-Za-z0-9_]*)"
                    ),
                ));
            }
            modules.push(name);
        }
        Ok(modules)
    }

    /// Run the UNCHANGED `publicEnv` secret-name denylist over the extracted
    /// names. A denylisted name is a read-time build error — the single most
    /// security-load-bearing check, preserved via the shared
    /// [`is_denylisted_public_env_name`].
    fn check_public_env(&self, span: Span, names: &[String]) -> Result<(), CliError> {
        for name in names {
            if is_denylisted_public_env_name(name) {
                return Err(self.reject(
                    span,
                    &format!(
                        "`publicEnv` lists {name:?}, which matches the secret-name denylist \
                         (*_SECRET / *_TOKEN / *_KEY / *_PASSWORD / DATABASE_URL / the internal \
                         IPE_* namespace) — a secret environment variable can never be \
                         allowlisted into the public wasm bundle"
                    ),
                ));
            }
        }
        Ok(())
    }

    /// Read a `[ Network, Clock, … ]` list into the typed capability set. An
    /// unknown capability is a named hard error — the typo-drops-a-capability
    /// footgun stays closed.
    fn read_capability_set(&self, expr: &Expr) -> Result<BTreeSet<Capability>, CliError> {
        let names = self.read_capability_wire_names(expr)?;
        let mut set = BTreeSet::new();
        for name in names {
            let cap = name
                .parse::<Capability>()
                .map_err(|e| self.reject(expr.span, &format!("unknown capability: {e}")))?;
            set.insert(cap);
        }
        Ok(set)
    }

    /// Read a `[ Network, Clock, … ]` list into capability *wire names* the
    /// shared validators consume. Each element must be a capability constructor;
    /// its name is mapped to the wire spelling (e.g. `NativeFfi` → `native-ffi`).
    fn read_capability_wire_names(&self, expr: &Expr) -> Result<Vec<String>, CliError> {
        let mut names = Vec::new();
        for item in self.expect_list(expr)? {
            // A web port is the applied ctor `JsPort <WebAxis>`, spelled to the
            // dotted `js-port:<axis>` wire name; every other capability is a bare
            // nullary ctor. `expect_ctor_app` yields both, with an empty arg slice
            // for the nullary case.
            let (ctor, args) = self.expect_ctor_app(item, "a capability")?;
            if ctor == "JsPort" {
                names.push(self.read_js_port_wire_name(item.span, args)?);
                continue;
            }
            if !args.is_empty() {
                return Err(self.reject(
                    item.span,
                    &format!("`{ctor}` is a nullary capability and takes no argument"),
                ));
            }
            let wire = capability_wire_name(ctor).ok_or_else(|| {
                self.reject(
                    item.span,
                    &format!(
                        "`{ctor}` is not a capability — use one of Network, Filesystem, Database, \
                         Env, Subprocess, Clock, Random, NativeFfi, FfiRaw, Unsafe, CustomElement, \
                         or `JsPort <WebAxis>`"
                    ),
                )
            })?;
            names.push(wire.to_owned());
        }
        Ok(names)
    }

    /// Read the single `WebCapability` argument of a `JsPort <WebAxis>` capability
    /// into its `js-port:<axis>` wire name. A bare `JsPort` (no argument) is
    /// rejected — the coarse grant-everything token is unspellable, so there is no
    /// web port without a named axis.
    fn read_js_port_wire_name(&self, span: Span, args: &[Expr]) -> Result<String, CliError> {
        let axis_expr = self.nth_arg(span, "JsPort", args, 0).map_err(|_| {
            self.reject(
                span,
                "`JsPort` needs a web-capability axis — write `JsPort Clipboard`, `JsPort Raw`, … \
                 (a bare `JsPort` cannot grant the whole browser surface)",
            )
        })?;
        let axis = self.expect_ctor(axis_expr, "a web capability")?;
        let suffix = web_capability_wire_suffix(axis).ok_or_else(|| {
            let valid = WebCapability::ALL
                .iter()
                .map(|c| c.ctor_name())
                .collect::<Vec<_>>()
                .join(", ");
            self.reject(
                axis_expr.span,
                &format!("`{axis}` is not a web capability — use one of {valid}"),
            )
        })?;
        Ok(format!("js-port:{suffix}"))
    }
}

/// The wire suffix (the half after `js-port:`) of a `WebCapability` constructor,
/// or `None` for a name outside the closed web-axis vocabulary.
fn web_capability_wire_suffix(ctor: &str) -> Option<&'static str> {
    WebCapability::from_ctor(ctor).map(WebCapability::as_str)
}

/// The wire name of a capability constructor, or `None` for a name outside the
/// closed capability vocabulary. The mapping is the inverse of the constructor
/// spelling in `Ipe.Package`'s `Capability` union, targeting the wire spelling
/// the shared [`Capability`] `FromStr` consumes.
fn capability_wire_name(ctor: &str) -> Option<&'static str> {
    match ctor {
        "Network" => Some("network"),
        "Filesystem" => Some("filesystem"),
        "Database" => Some("database"),
        "Env" => Some("env"),
        "Subprocess" => Some("subprocess"),
        "Clock" => Some("clock"),
        "Random" => Some("random"),
        "NativeFfi" => Some("native-ffi"),
        "FfiRaw" => Some("ffi-raw"),
        "Unsafe" => Some("unsafe"),
        "CustomElement" => Some("custom-element"),
        // `JsPort` is not a nullary capability — it carries a `WebCapability` axis
        // and is read via the applied-ctor path, never here.
        _ => None,
    }
}

/// Whether `name` is a valid dotted Ipê module name: one or more `.`-separated
/// segments, each starting with an ASCII uppercase letter and continuing with
/// ASCII alphanumerics or `_`. An empty name or any malformed segment is invalid.
fn is_module_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    name.split('.').all(|segment| {
        let mut chars = segment.chars();
        match chars.next() {
            Some(c) if c.is_ascii_uppercase() => {
                chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
            }
            _ => false,
        }
    })
}

/// The 1-based `(line, column)` of byte offset `off` in `src`. An out-of-range
/// offset degrades to `(1, 1)` — the reader stays total, never panicking on a
/// span that (unexpectedly) falls outside the source.
fn line_col(src: &str, off: u32) -> (usize, usize) {
    let off = off as usize;
    let mut line = 1usize;
    let mut col = 1usize;
    for (i, ch) in src.char_indices() {
        if i >= off {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// Serialise a [`ProjectManifest`] into `package.ipe` record source.
///
/// The inverse of [`read_package_manifest`] over the fields the manifest carries:
/// the emitted record re-reads to an equivalent manifest. Only non-default
/// sections are written, so a minimal manifest serialises to a minimal record.
/// Used by `ipe migrate config` to rewrite an interim builder manifest (or a
/// legacy `ipe.toml`) into the record form, and available to any caller that must
/// emit a manifest.
#[must_use]
pub fn render_manifest_record(manifest: &ProjectManifest) -> String {
    let mut fields: Vec<String> = Vec::new();
    fields.push(format!("name = {}", quote(&manifest.name)));
    if let Some(version) = &manifest.version {
        fields.push(format!("version = {}", quote(&version.to_string())));
    }
    if let Some(src_rel) = manifest_src_rel(manifest)
        && src_rel != "src"
    {
        fields.push(format!("sourceRoot = {}", quote(&src_rel)));
    }
    if !manifest.dependencies.is_empty() {
        fields.push(render_dependencies(&manifest.dependencies));
    }
    if !manifest.rust_dependencies.is_empty() {
        fields.push(render_rust_dependencies(&manifest.rust_dependencies));
    }
    if !manifest.capabilities.is_empty() || !manifest.capabilities_accept.is_empty() {
        fields.push(render_capabilities(
            &manifest.capabilities,
            &manifest.capabilities_accept,
        ));
    }
    if !manifest.exposed_modules.is_empty() {
        let items = manifest
            .exposed_modules
            .iter()
            .map(|m| quote(m))
            .collect::<Vec<_>>()
            .join(", ");
        fields.push(format!("exposedModules = [ {items} ]"));
    }
    if !manifest.programs.is_empty() {
        fields.push(render_programs(&manifest.programs));
    }
    if let Some(wasm) = render_wasm(&manifest.wasm) {
        fields.push(wasm);
    }
    if let Some(build) = render_build(manifest) {
        fields.push(build);
    }

    let mut out = String::new();
    out.push_str("module Package exposing (package)\n\n");
    out.push_str("import Ipe.Package exposing (..)\n\n\n");
    out.push_str("package : Package\npackage =\n");
    for (i, field) in fields.iter().enumerate() {
        let opener = if i == 0 { "    { " } else { "    , " };
        out.push_str(opener);
        out.push_str(field);
        out.push('\n');
    }
    out.push_str("    }\n");
    out
}

/// The manifest's source-root relative to its root, when it can be expressed as
/// a relative path (it always can — `src_root` is `root`-contained by parse).
fn manifest_src_rel(manifest: &ProjectManifest) -> Option<String> {
    manifest
        .src_root
        .strip_prefix(&manifest.root)
        .ok()
        .map(|rel| rel.to_string_lossy().into_owned())
}

/// Quote a string as an Ipê string literal, escaping the two characters a
/// literal must escape (`\` and `"`). Manifest strings (names, versions, paths,
/// urls) do not carry control characters; a `\` or `"` in a path or url is
/// escaped so the emitted literal round-trips.
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// Render the `dependencies = [ … ]` field from the typed map.
fn render_dependencies(deps: &BTreeMap<String, IpeDep>) -> String {
    let items: Vec<String> = deps
        .iter()
        .map(|(name, dep)| match dep {
            IpeDep::Index(req) => format!("dep {} {}", quote(name), quote(&req.to_string())),
            IpeDep::Git { url, rev: None } => format!("depGit {} {}", quote(name), quote(url)),
            IpeDep::Git {
                url,
                rev: Some(rev),
            } => {
                format!("depGitRev {} {} {}", quote(name), quote(url), quote(rev))
            }
            IpeDep::Path(path) => {
                format!("depPath {} {}", quote(name), quote(&path.to_string_lossy()))
            }
        })
        .collect();
    format!("dependencies =\n        [ {} ]", items.join("\n        , "))
}

/// Render the `rustDependencies = [ … ]` field from the typed map.
fn render_rust_dependencies(deps: &BTreeMap<String, RustDep>) -> String {
    let items: Vec<String> = deps
        .iter()
        .map(|(name, dep)| {
            if dep.features.is_empty() {
                format!("rustDep {} {}", quote(name), quote(&dep.version))
            } else {
                let features = dep
                    .features
                    .iter()
                    .map(|f| quote(f))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "rustDepWith {} {} [ {features} ]",
                    quote(name),
                    quote(&dep.version)
                )
            }
        })
        .collect();
    format!(
        "rustDependencies =\n        [ {} ]",
        items.join("\n        , ")
    )
}

/// Render the `capabilities = { declares = …, accepts = … }` field. The
/// constructor spelling is the inverse of [`capability_wire_name`].
fn render_capabilities(declares: &BTreeSet<Capability>, accepts: &BTreeSet<Capability>) -> String {
    let render_set = |set: &BTreeSet<Capability>| {
        if set.is_empty() {
            return "[]".to_owned();
        }
        let items = set
            .iter()
            .map(|c| capability_ctor_expr(c.as_str()))
            .collect::<Vec<_>>()
            .join(", ");
        format!("[ {items} ]")
    };
    format!(
        "capabilities =\n        {{ declares = {}\n        , accepts = {}\n        }}",
        render_set(declares),
        render_set(accepts)
    )
}

/// The `Ipe.Package` `Capability` constructor expression for a wire name — the
/// inverse of the manifest reader. A `js-port:<axis>` wire renders as the applied
/// ctor `JsPort <WebAxis>`; every other wire name is a bare nullary ctor. An
/// unknown wire name (never expected from a typed [`Capability`]) passes through
/// verbatim.
fn capability_ctor_expr(wire: &str) -> String {
    if let Some(suffix) = wire.strip_prefix("js-port:") {
        return format!("JsPort {}", web_capability_ctor_name(suffix));
    }
    match wire {
        "network" => "Network",
        "filesystem" => "Filesystem",
        "database" => "Database",
        "env" => "Env",
        "subprocess" => "Subprocess",
        "clock" => "Clock",
        "random" => "Random",
        "native-ffi" => "NativeFfi",
        "ffi-raw" => "FfiRaw",
        "unsafe" => "Unsafe",
        "custom-element" => "CustomElement",
        other => other,
    }
    .to_owned()
}

/// The `WebCapability` constructor spelling for a wire suffix — the inverse of
/// [`web_capability_wire_suffix`]. An unknown suffix passes through verbatim.
fn web_capability_ctor_name(suffix: &str) -> &str {
    WebCapability::ALL
        .iter()
        .find(|c| c.as_str() == suffix)
        .map_or(suffix, |c| c.ctor_name())
}

/// Render the `programs = [ … ]` field from the typed vector.
fn render_programs(programs: &[Program]) -> String {
    let items: Vec<String> = programs
        .iter()
        .map(|p| {
            let mut parts = vec![
                format!("name = {}", quote(&p.name)),
                format!("entry = {}", quote(&p.entry)),
            ];
            if let Some(shape) = p.shape {
                parts.push(format!("shape = {}", shape_ctor_name(shape)));
            }
            format!("{{ {} }}", parts.join(", "))
        })
        .collect();
    format!("programs =\n        [ {} ]", items.join("\n        , "))
}

/// The `Ipe.Package` `Shape` constructor spelling for an [`EntryShape`].
const fn shape_ctor_name(shape: EntryShape) -> &'static str {
    match shape {
        EntryShape::Web => "Web",
        EntryShape::WebView => "WebView",
        EntryShape::Terminal => "Terminal",
        EntryShape::Program => "Program",
    }
}

/// Render the `wasm = …` field, or `None` when the config is the default (no
/// active bundle) — an absent field reads back as `Off`.
fn render_wasm(wasm: &WasmConfig) -> Option<String> {
    if *wasm == WasmConfig::default() {
        return None;
    }
    let mode = match wasm.mode.as_deref() {
        Some("hydrate") => "Hydrate",
        _ => "Spa",
    };
    let mut parts = vec![format!("mode = {mode}")];
    if let Some(entry) = &wasm.entry {
        parts.push(format!("entry = {}", quote(entry)));
    }
    if let Some(mount) = &wasm.mount {
        parts.push(format!("mount = {}", quote(mount)));
    }
    if !wasm.public_env.is_empty() {
        let names = wasm
            .public_env
            .iter()
            .map(|n| quote(n))
            .collect::<Vec<_>>()
            .join(", ");
        parts.push(format!("publicEnv = [ {names} ]"));
    }
    if let Some(opt) = &wasm.opt_level {
        parts.push(format!("optLevel = {}", quote(opt)));
    }
    Some(format!("wasm =\n        On {{ {} }}", parts.join(", ")))
}

/// Render the `build = { … }` field, or `None` when every build setting is at
/// its default (an absent field reads back as all-defaults).
fn render_build(manifest: &ProjectManifest) -> Option<String> {
    let driver_default = manifest.driver == ipe_backend_rust::DbDriver::Sqlite;
    let static_layer = &manifest.static_request;
    if driver_default && *static_layer == crate::build_plan::StaticRequestLayer::default() {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    let database = match manifest.driver {
        ipe_backend_rust::DbDriver::Sqlite => "Sqlite",
        ipe_backend_rust::DbDriver::Postgres => "Postgres",
    };
    parts.push(format!("database = {database}"));
    if let Some(b) = static_layer.static_build {
        parts.push(format!("static = {}", bool_ctor(b)));
    }
    if let Some(target) = &static_layer.target {
        parts.push(format!("target = Cross {}", quote(target)));
    }
    if let Some(alloc) = static_layer.allocator {
        parts.push(format!("allocator = {}", allocator_ctor_name(alloc)));
    }
    if let Some(b) = static_layer.allow_slow_allocator {
        parts.push(format!("allowSlowAllocator = {}", bool_ctor(b)));
    }
    if let Some(b) = static_layer.c_free {
        parts.push(format!("cFree = {}", bool_ctor(b)));
    }
    Some(format!(
        "build =\n        {{ {} }}",
        parts.join("\n        , ")
    ))
}

/// The `True` / `False` constructor for a bool.
const fn bool_ctor(b: bool) -> &'static str {
    if b { "True" } else { "False" }
}

/// The `Ipe.Package` `Allocator` constructor spelling for an
/// [`crate::build_plan::AllocatorChoice`].
const fn allocator_ctor_name(alloc: crate::build_plan::AllocatorChoice) -> &'static str {
    use crate::build_plan::AllocatorChoice as A;
    match alloc {
        A::Auto => "AutoAlloc",
        A::System => "System",
        A::Dlmalloc => "Dlmalloc",
        A::Talc => "Talc",
        A::Mimalloc => "Mimalloc",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a fresh temp project directory with a minimal `src/Main.ipe`, so
    /// the reader's source-root existence check passes. Returns the project root.
    fn fresh_project(test_name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("ipe_pkg_manifest_{test_name}"));
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

    /// Read a `package.ipe` body against a fresh project root, returning the
    /// manifest or the error. The `package.ipe` is written into the project root.
    fn read(test_name: &str, source: &str) -> Result<ProjectManifest, CliError> {
        let root = fresh_project(test_name);
        let path = root.join(PACKAGE_IPE);
        std::fs::write(&path, source).expect("write package.ipe");
        let result = parse_package_manifest(&path);
        let _ = std::fs::remove_dir_all(&root);
        result
    }

    const HEADER: &str = "module Package exposing (package)\n\n";

    #[test]
    fn minimal_manifest_names_the_package_and_defaults_everything() {
        let m = read(
            "minimal",
            &format!("{HEADER}package =\n    {{ name = \"my-app\" }}\n"),
        )
        .expect("minimal manifest must parse");
        assert_eq!(m.name, "my-app");
        assert_eq!(m.version, None);
        assert_eq!(m.driver, ipe_backend_rust::DbDriver::Sqlite);
        assert!(m.dependencies.is_empty());
        assert!(m.rust_dependencies.is_empty());
        assert!(m.capabilities.is_empty());
        assert!(m.capabilities_accept.is_empty());
        assert_eq!(m.wasm, WasmConfig::default());
        assert!(!m.has_rust_wrapper);
        assert_eq!(
            m.static_request,
            crate::build_plan::StaticRequestLayer::default()
        );
    }

    #[test]
    fn every_field_round_trips() {
        let source = format!(
            "{HEADER}package =\n\
             \x20   {{ name = \"my-app\"\n\
             \x20   , version = \"0.3.0\"\n\
             \x20   , sourceRoot = \"src\"\n\
             \x20   , dependencies =\n\
             \x20       [ dep \"ipe-http\" \"^1.2\"\n\
             \x20       , depGitRev \"ipe-widgets\" \"https://example.test/w.git\" \"a1b2c3\"\n\
             \x20       , depGit \"ipe-plain\" \"https://example.test/p.git\"\n\
             \x20       , depPath \"ipe-local\" \"../local\"\n\
             \x20       ]\n\
             \x20   , rustDependencies =\n\
             \x20       [ rustDep \"uuid\" \"1.10\"\n\
             \x20       , rustDepWith \"image\" \"0.25\" [ \"png\", \"jpeg\" ]\n\
             \x20       ]\n\
             \x20   , capabilities =\n\
             \x20       {{ declares = [ Network, Clock ]\n\
             \x20       , accepts = [ Unsafe ]\n\
             \x20       }}\n\
             \x20   , wasm =\n\
             \x20       On\n\
             \x20           {{ mode = Spa\n\
             \x20           , entry = \"src/Client.ipe\"\n\
             \x20           , mount = \"#app\"\n\
             \x20           , publicEnv = [ \"API_BASE_URL\", \"APP_VERSION\" ]\n\
             \x20           , optLevel = \"z\"\n\
             \x20           }}\n\
             \x20   , build =\n\
             \x20       {{ database = Postgres\n\
             \x20       , static = True\n\
             \x20       , target = Cross \"x86_64-unknown-linux-musl\"\n\
             \x20       , allocator = Dlmalloc\n\
             \x20       , allowSlowAllocator = False\n\
             \x20       , cFree = True\n\
             \x20       }}\n\
             \x20   }}\n"
        );
        let m = read("full", &source).expect("full manifest must parse");
        assert_eq!(m.name, "my-app");
        assert_eq!(
            m.version,
            Some(semver::Version::parse("0.3.0").expect("semver"))
        );
        assert_eq!(m.driver, ipe_backend_rust::DbDriver::Postgres);

        assert_eq!(m.dependencies.len(), 4);
        assert!(matches!(
            m.dependencies.get("ipe-http"),
            Some(IpeDep::Index(_))
        ));
        assert!(matches!(
            m.dependencies.get("ipe-widgets"),
            Some(IpeDep::Git { rev: Some(r), .. }) if r == "a1b2c3"
        ));
        assert!(matches!(
            m.dependencies.get("ipe-plain"),
            Some(IpeDep::Git { rev: None, .. })
        ));
        assert!(matches!(
            m.dependencies.get("ipe-local"),
            Some(IpeDep::Path(_))
        ));

        assert_eq!(m.rust_dependencies.len(), 2);
        assert_eq!(
            m.rust_dependencies.get("image").map(|d| d.features.clone()),
            Some(vec!["png".to_owned(), "jpeg".to_owned()])
        );

        assert_eq!(m.static_request.static_build, Some(true));
        assert_eq!(
            m.static_request.target.as_deref(),
            Some("x86_64-unknown-linux-musl")
        );
        assert_eq!(
            m.static_request.allocator,
            Some(crate::build_plan::AllocatorChoice::Dlmalloc)
        );
        assert_eq!(m.static_request.allow_slow_allocator, Some(false));
        assert_eq!(m.static_request.c_free, Some(true));

        let cap_names: Vec<&str> = m.capabilities.iter().map(|c| c.as_str()).collect();
        assert_eq!(cap_names, vec!["network", "clock"]);
        let accept_names: Vec<&str> = m.capabilities_accept.iter().map(|c| c.as_str()).collect();
        assert_eq!(accept_names, vec!["unsafe"]);

        assert_eq!(m.wasm.mode.as_deref(), Some("spa"));
        assert_eq!(m.wasm.entry.as_deref(), Some("src/Client.ipe"));
        assert_eq!(m.wasm.mount.as_deref(), Some("#app"));
        assert_eq!(m.wasm.public_env, vec!["API_BASE_URL", "APP_VERSION"]);
        assert_eq!(m.wasm.opt_level.as_deref(), Some("z"));
    }

    #[test]
    fn accepts_the_schema_import() {
        let m = read(
            "schema_import",
            &format!(
                "{HEADER}import Ipe.Package exposing (..)\n\n\npackage =\n    {{ name = \"x\" }}\n"
            ),
        )
        .expect("the schema import is permitted");
        assert_eq!(m.name, "x");
    }

    // ── Rejections: totality (a clean diagnostic, never a panic) ──────────────

    /// Every rejection asserts a `UsageOwned` (the reader's named-error channel)
    /// or a `Usage`/`Pipeline` — never a panic and never an `Ok`.
    fn assert_rejected(result: &Result<ProjectManifest, CliError>) {
        assert!(
            matches!(
                result,
                Err(CliError::UsageOwned(_) | CliError::Usage(_) | CliError::Pipeline { .. })
            ),
            "expected a clean rejection, got {result:?}"
        );
    }

    #[test]
    fn reject_non_schema_import() {
        let r = read(
            "reject_import",
            &format!("{HEADER}import Ipe.String\n\npackage =\n    {{ name = \"x\" }}\n"),
        );
        assert_rejected(&r);
    }

    #[test]
    fn reject_computed_field() {
        // A field written as a computed `if` expression, not a literal.
        let r = read(
            "reject_computed",
            &format!("{HEADER}package =\n    {{ name = if True then \"a\" else \"b\" }}\n"),
        );
        assert_rejected(&r);
    }

    #[test]
    fn reject_reference_to_another_binding() {
        // A second top-level binding referenced from the package value — the
        // reader forbids any binding other than `package`.
        let r = read(
            "reject_ref",
            &format!("{HEADER}n =\n    \"x\"\n\npackage =\n    {{ name = n }}\n"),
        );
        assert_rejected(&r);
    }

    #[test]
    fn reject_unknown_field() {
        let r = read(
            "reject_field",
            &format!("{HEADER}package =\n    {{ name = \"x\", nickname = \"y\" }}\n"),
        );
        assert_rejected(&r);
    }

    #[test]
    fn reject_non_record_body() {
        let r = read(
            "reject_non_record",
            &format!("{HEADER}package =\n    \"x\"\n"),
        );
        assert_rejected(&r);
    }

    #[test]
    fn reject_denylisted_public_env() {
        let r = read(
            "reject_secret_env",
            &format!(
                "{HEADER}package =\n    {{ name = \"x\", wasm = On {{ mode = Spa, publicEnv = [ \"DATABASE_URL\" ] }} }}\n"
            ),
        );
        assert_rejected(&r);
        if let Err(CliError::UsageOwned(msg)) = &r {
            assert!(
                msg.contains("DATABASE_URL"),
                "error names the secret: {msg}"
            );
        }
    }

    #[test]
    fn reject_unknown_database_constructor() {
        let r = read(
            "reject_driver",
            &format!("{HEADER}package =\n    {{ name = \"x\", build = {{ database = MySql }} }}\n"),
        );
        assert_rejected(&r);
    }

    #[test]
    fn reject_unknown_capability() {
        let r = read(
            "reject_capability",
            &format!(
                "{HEADER}package =\n    {{ name = \"x\", capabilities = {{ declares = [ Telepathy ] }} }}\n"
            ),
        );
        assert_rejected(&r);
    }

    #[test]
    fn reject_malformed_version() {
        let r = read(
            "reject_version",
            &format!("{HEADER}package =\n    {{ name = \"x\", version = \"not-a-version\" }}\n"),
        );
        assert_rejected(&r);
    }

    #[test]
    fn reject_missing_name() {
        let r = read(
            "reject_no_name",
            &format!("{HEADER}package =\n    {{ version = \"1.0.0\" }}\n"),
        );
        assert_rejected(&r);
    }

    #[test]
    fn reject_package_as_function() {
        let r = read(
            "reject_function",
            &format!("{HEADER}package x =\n    {{ name = x }}\n"),
        );
        assert_rejected(&r);
    }

    #[test]
    fn reject_extra_binding() {
        let r = read(
            "reject_extra",
            &format!(
                "{HEADER}package =\n    {{ name = \"x\" }}\n\nother =\n    {{ name = \"y\" }}\n"
            ),
        );
        assert_rejected(&r);
    }

    #[test]
    fn reject_bad_wrapper_path_escapes_jail() {
        let r = read(
            "reject_wrapper",
            &format!(
                "{HEADER}package =\n    {{ name = \"x\", wrapper = Wrapper {{ path = \"/etc/passwd\", expose = [ \"f\" ] }} }}\n"
            ),
        );
        assert_rejected(&r);
    }

    #[test]
    fn accepts_a_valid_wrapper() {
        let r = read(
            "ok_wrapper",
            &format!(
                "{HEADER}package =\n    {{ name = \"x\", wrapper = Wrapper {{ path = \"./vendor/mycrate\", expose = [ \"encode\" ] }} }}\n"
            ),
        );
        let m = r.expect("valid wrapper must parse");
        assert!(m.has_rust_wrapper);
    }

    #[test]
    fn no_wrapper_declares_none() {
        let m = read(
            "no_wrapper",
            &format!("{HEADER}package =\n    {{ name = \"x\", wrapper = NoWrapper }}\n"),
        )
        .expect("NoWrapper must parse");
        assert!(!m.has_rust_wrapper);
    }

    #[test]
    fn allowed_public_env_passes() {
        let r = read(
            "ok_env",
            &format!(
                "{HEADER}package =\n    {{ name = \"x\", wasm = On {{ mode = Spa, publicEnv = [ \"API_BASE_URL\" ] }} }}\n"
            ),
        );
        let m = r.expect("allowed env must parse");
        assert_eq!(m.wasm.public_env, vec!["API_BASE_URL"]);
    }

    #[test]
    fn wasm_off_is_the_default_config() {
        let m = read(
            "wasm_off",
            &format!("{HEADER}package =\n    {{ name = \"x\", wasm = Off }}\n"),
        )
        .expect("Off must parse");
        assert_eq!(m.wasm, WasmConfig::default());
    }

    #[test]
    fn reject_wasm_on_without_mode() {
        let r = read(
            "reject_wasm_no_mode",
            &format!(
                "{HEADER}package =\n    {{ name = \"x\", wasm = On {{ mount = \"#app\" }} }}\n"
            ),
        );
        assert_rejected(&r);
    }

    // ── programs / exposedModules ─────────────────────────────────────────────

    #[test]
    fn programs_and_exposed_modules_parse_into_the_typed_manifest() {
        let source = format!(
            "{HEADER}package =\n\
             \x20   {{ name = \"my-app\"\n\
             \x20   , programs =\n\
             \x20       [ {{ name = \"server\", entry = \"Main.ipe\", shape = Web }}\n\
             \x20       , {{ name = \"cli\", entry = \"Cli/Main.ipe\", shape = Terminal }}\n\
             \x20       ]\n\
             \x20   , exposedModules = [ \"Core\", \"Core.Utils\" ]\n\
             \x20   }}\n"
        );
        let m = read("programs_and_exposed", &source).expect("manifest must parse");

        assert_eq!(
            m.programs,
            vec![
                Program {
                    name: "server".to_owned(),
                    entry: "Main.ipe".to_owned(),
                    shape: Some(EntryShape::Web),
                },
                Program {
                    name: "cli".to_owned(),
                    entry: "Cli/Main.ipe".to_owned(),
                    shape: Some(EntryShape::Terminal),
                },
            ]
        );

        assert_eq!(m.exposed_modules, vec!["Core", "Core.Utils"]);
        assert_eq!(m.resolved_entry().expect("entry"), vec!["Main".to_owned()]);
    }

    #[test]
    fn program_entry_defaults_to_main_when_omitted() {
        let source = format!(
            "{HEADER}package =\n\
             \x20   {{ name = \"x\", programs = [ {{ name = \"app\" }} ] }}\n"
        );
        let m = read("program_default_entry", &source).expect("manifest must parse");
        assert_eq!(
            m.programs,
            vec![Program {
                name: "app".to_owned(),
                entry: "Main.ipe".to_owned(),
                shape: None,
            }]
        );
        assert_eq!(m.resolved_entry().expect("entry"), vec!["Main".to_owned()]);
    }

    #[test]
    fn program_entry_routes_a_nested_module_path() {
        let source = format!(
            "{HEADER}package =\n\
             \x20   {{ name = \"x\", programs = [ {{ name = \"app\", entry = \"Client/App.ipe\" }} ] }}\n"
        );
        let m = read("program_nested_entry", &source).expect("manifest must parse");
        assert_eq!(
            m.resolved_entry().expect("entry"),
            vec!["Client".to_owned(), "App".to_owned()]
        );
    }

    #[test]
    fn no_programs_resolves_entry_to_main() {
        let m = read(
            "no_programs",
            &format!("{HEADER}package =\n    {{ name = \"x\" }}\n"),
        )
        .expect("manifest must parse");
        assert!(m.programs.is_empty());
        assert!(m.exposed_modules.is_empty());
        assert_eq!(m.resolved_entry().expect("entry"), vec!["Main".to_owned()]);
    }

    #[test]
    fn reject_unknown_shape_constructor() {
        let r = read(
            "reject_shape",
            &format!(
                "{HEADER}package =\n    {{ name = \"x\", programs = [ {{ name = \"a\", shape = Hologram }} ] }}\n"
            ),
        );
        assert_rejected(&r);
    }

    #[test]
    fn reject_duplicate_program_name() {
        let r = read(
            "reject_dup_program",
            &format!(
                "{HEADER}package =\n    {{ name = \"x\", programs = [ {{ name = \"a\" }}, {{ name = \"a\" }} ] }}\n"
            ),
        );
        assert_rejected(&r);
    }

    #[test]
    fn reject_program_missing_name() {
        let r = read(
            "reject_program_no_name",
            &format!(
                "{HEADER}package =\n    {{ name = \"x\", programs = [ {{ entry = \"Main.ipe\" }} ] }}\n"
            ),
        );
        assert_rejected(&r);
    }

    #[test]
    fn reject_lowercase_exposed_module() {
        let r = read(
            "reject_exposed_lower",
            &format!("{HEADER}package =\n    {{ name = \"x\", exposedModules = [ \"core\" ] }}\n"),
        );
        assert_rejected(&r);
    }

    #[test]
    fn reject_program_entry_with_lowercase_segment() {
        let m = read(
            "reject_entry_lower",
            &format!(
                "{HEADER}package =\n    {{ name = \"x\", programs = [ {{ name = \"a\", entry = \"lower/App.ipe\" }} ] }}\n"
            ),
        )
        .expect("manifest itself parses");
        assert!(
            m.resolved_entry().is_err(),
            "a lowercase entry segment must be rejected at entry resolution"
        );
    }

    #[test]
    fn is_module_name_rules() {
        assert!(is_module_name("Core"));
        assert!(is_module_name("Core.Utils"));
        assert!(is_module_name("A.B.C2"));
        assert!(is_module_name("My_Module"));
        assert!(!is_module_name("core"));
        assert!(!is_module_name(""));
        assert!(!is_module_name("Core."));
        assert!(!is_module_name("Core..Utils"));
        assert!(!is_module_name("1Core"));
    }

    #[test]
    fn capability_wire_names_cover_the_vocabulary() {
        assert_eq!(capability_wire_name("Network"), Some("network"));
        assert_eq!(capability_wire_name("NativeFfi"), Some("native-ffi"));
        assert_eq!(
            capability_wire_name("CustomElement"),
            Some("custom-element")
        );
        // `JsPort` is not a nullary capability — it carries a web axis and is read
        // via the applied-ctor path, so it is absent from the nullary map.
        assert_eq!(capability_wire_name("JsPort"), None);
        assert_eq!(capability_wire_name("Nope"), None);
        // Every wire name a nullary constructor maps to must round-trip through the
        // shared Capability FromStr — no drift between the two sets.
        for ctor in [
            "Network",
            "Filesystem",
            "Database",
            "Env",
            "Subprocess",
            "Clock",
            "Random",
            "NativeFfi",
            "FfiRaw",
            "Unsafe",
            "CustomElement",
        ] {
            let wire = capability_wire_name(ctor).expect("mapped");
            assert!(
                wire.parse::<Capability>().is_ok(),
                "wire {wire:?} for {ctor:?} must parse as a Capability"
            );
        }
    }

    #[test]
    fn js_port_web_axis_wire_names_round_trip_through_capability_from_str() {
        // Every `JsPort <WebAxis>` renders to a `js-port:<axis>` wire name that the
        // shared Capability FromStr accepts, and a bare `js-port` never parses.
        // Derived from `WebCapability::ALL` so adding a new axis is automatically covered.
        for &web_cap in WebCapability::ALL {
            let axis = web_cap.ctor_name();
            let maybe_suffix = web_capability_wire_suffix(axis);
            assert!(
                maybe_suffix.is_some(),
                "WebCapability::ALL member {axis:?} must map through web_capability_wire_suffix"
            );
            let suffix = maybe_suffix.unwrap();
            let wire = format!("js-port:{suffix}");
            assert!(
                wire.parse::<Capability>().is_ok(),
                "wire {wire:?} for JsPort {axis} must parse as a Capability"
            );
            // The ctor renderer is the inverse: `js-port:<axis>` → `JsPort <Axis>`.
            assert_eq!(capability_ctor_expr(&wire), format!("JsPort {axis}"));
        }
        // A name genuinely outside the web-axis vocabulary must return None.
        assert!(web_capability_wire_suffix("NotARealAxis").is_none());
        assert!("js-port".parse::<Capability>().is_err());
    }

    #[test]
    fn line_col_is_one_based_and_total() {
        assert_eq!(line_col("abc", 0), (1, 1));
        assert_eq!(line_col("abc", 2), (1, 3));
        assert_eq!(line_col("a\nbc", 2), (2, 1));
        assert_eq!(line_col("a", 999), (1, 2));
    }
}
