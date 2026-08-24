//! `package.ipe` — the project manifest written in Ipê and read *syntactically*.
//!
//! The bootstrap constraint is decisive: the toolchain must learn a project's
//! dependencies before it can compile anything, so it cannot evaluate Ipê — that
//! would require the very dependencies it is trying to discover — to read them.
//! The resolution is to read, never run: the manifest declares one top-level
//! `package` binding built from a blessed vocabulary, and this reader extracts
//! each field by walking the AST of that binding, refusing anything that is not a
//! literal argument to a blessed builder.
//!
//! # What this reader does
//!
//! It reuses the compiler's own front end — [`ipe_parse::parse_module`] — and
//! nothing past it: no canonicalisation, no name resolution, no type-checking,
//! no lowering, no emit, and above all no evaluation. The parser is total and
//! effect-free by construction, so reading an untrusted `package.ipe` runs none
//! of its code. The reader then operates purely on the resulting AST, producing
//! the same [`ProjectManifest`] the `ipe.toml` line-scanner produces — one
//! struct, two front doors.
//!
//! # The blessed vocabulary
//!
//! A single builder surface, one symbol per manifest field, recognised by name.
//! A `package.ipe` is a `|>` pipeline of `Package.*` / `Wasm.*` / `Rust.*` calls
//! over literal arguments (strings, lists, nested blessed calls, and blessed
//! nullary constructors such as `Package.postgres`, `Static.on`,
//! `Capability.network`). Driver, allocator, wasm-mode, and boolean choices are
//! blessed nullary constructors rather than free strings, so a typo that today
//! reaches a runtime rejection is instead not a writable manifest at all.
//!
//! # Preserved validations
//!
//! Every parse-time check the `ipe.toml` reader carries runs here too, against
//! the extracted literal, via the *same* shared functions — the `publicEnv`
//! secret-name denylist, semver parsing, capability validation, and the wrapper
//! path jail. There is no second copy that could drift from the `ipe.toml` path.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use ipe_diagnostics::{Located, Span};
use ipe_intern::Interner;
use ipe_syntax::{Expr, Expr_, Module};

use crate::CliError;
use crate::project::{
    Capability, IpeDep, ProjectManifest, RustDep, WasmConfig, is_denylisted_public_env_name,
};

/// The manifest filename read by this reader.
pub const PACKAGE_IPE: &str = "package.ipe";

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
    let text = std::fs::read_to_string(manifest_path).map_err(|e| CliError::Io {
        path: manifest_path.to_path_buf(),
        source: e,
    })?;
    read_package_manifest(&text, &root, manifest_path)
}

/// The total core: `&str -> Result<ProjectManifest, CliError>`, given the
/// project `root` (for path fields) and the manifest path (for diagnostics).
///
/// This is the security boundary. It parses `src` and walks the AST of the sole
/// `package` binding; it never evaluates any expression.
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

/// The fields accumulated while walking the pipeline, before assembly into a
/// [`ProjectManifest`]. Every field defaults to the same absent-section default
/// the `ipe.toml` path uses, so a minimal `package = Package.named "x"` yields an
/// identical struct.
#[derive(Default)]
struct ManifestFields {
    name: Option<String>,
    version: Option<semver::Version>,
    src_rel: Option<String>,
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
}

impl ManifestFields {
    /// Assemble the accumulated fields into a [`ProjectManifest`], applying the
    /// two remaining whole-manifest validations: `name` is required, and the
    /// source-root directory must exist.
    fn into_manifest(self, root: &Path) -> Result<ProjectManifest, CliError> {
        let name = self.name.ok_or(CliError::Usage(
            "package.ipe: missing a `Package.named \"…\"` stage — a package must be named",
        ))?;
        let src_root = root.join(self.src_rel.as_deref().unwrap_or("src"));
        if !src_root.is_dir() {
            return Err(CliError::Usage(
                "package.ipe: the source root directory does not exist",
            ));
        }
        Ok(ProjectManifest {
            name,
            version: self.version,
            root: root.to_path_buf(),
            src_root,
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
    /// enforcing the module-shape rules: no imports, exactly one top-level
    /// `package` value binding with no parameters, and no other declarations.
    fn read_module(&self, module: &Module) -> Result<ManifestFields, CliError> {
        if let Some(import) = module.imports.first() {
            return Err(self.reject(
                import.name.span,
                "a package.ipe may not `import` anything — the manifest is read before \
                 dependencies are resolved, and the vocabulary is recognised by name, never \
                 imported",
            ));
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

        let stages = self.linearise_pipeline(&package.value.body)?;
        let mut fields = ManifestFields::default();
        for stage in stages {
            self.apply_package_stage(stage, &mut fields)?;
        }
        Ok(fields)
    }

    /// Linearise the `|>` spine of the `package` body into an ordered list of
    /// stage expressions: the pipeline head plus each right-hand call the value
    /// is piped into. A body that is a bare `Package.named "…"` (no pipeline) is
    /// a single-stage list. Any operator other than `|>` is rejected.
    fn linearise_pipeline<'e>(&self, body: &'e Expr) -> Result<Vec<&'e Expr>, CliError> {
        match &body.value {
            Expr_::Binops(ops, last) => {
                let mut stages = Vec::with_capacity(ops.len() + 1);
                for (operand, op) in ops {
                    if self.text(op.value) != "|>" {
                        return Err(self.reject(
                            op.span,
                            "the package pipeline may only be threaded with `|>` — no other \
                             operator is allowed in a package.ipe",
                        ));
                    }
                    stages.push(operand);
                }
                stages.push(last.as_ref());
                Ok(stages)
            }
            // A bare head with no pipeline (`package = Package.named "x"`).
            _ => Ok(vec![body]),
        }
    }

    /// Apply one top-level pipeline stage to the accumulated fields. Every stage
    /// must be a blessed `Package.*` / `Wasm.*` / `Rust.*` call (or the bare
    /// `Package.named "…"` head); the callee names the field and the literal
    /// arguments carry its value.
    fn apply_package_stage(
        &self,
        stage: &Expr,
        fields: &mut ManifestFields,
    ) -> Result<(), CliError> {
        let (module, name, args) = self.expect_blessed_call(stage)?;
        match (module, name) {
            ("Package", "named") => {
                fields.name = Some(self.one_string(stage.span, name, args)?);
            }
            ("Package", "version") => {
                let raw = self.one_string(stage.span, name, args)?;
                let version = semver::Version::parse(&raw).map_err(|e| {
                    self.reject(
                        stage.span,
                        &format!("`Package.version {raw:?}` is not valid semver: {e}"),
                    )
                })?;
                fields.version = Some(version);
            }
            ("Package", "sourceRoot") => {
                fields.src_rel = Some(self.one_string(stage.span, name, args)?);
            }
            ("Package", "database") => {
                fields.driver = Some(self.read_driver(self.nth_arg(stage.span, name, args, 0)?)?);
            }
            ("Package", "dependencies") => {
                fields.dependencies =
                    self.read_dependencies(self.nth_arg(stage.span, name, args, 0)?)?;
            }
            ("Package", "rustDependencies") => {
                fields.rust_dependencies =
                    self.read_rust_dependencies(self.nth_arg(stage.span, name, args, 0)?)?;
            }
            ("Package", "wrapper") => {
                self.read_wrapper(self.nth_arg(stage.span, name, args, 0)?)?;
                fields.has_rust_wrapper = true;
            }
            ("Package", "static") => {
                fields.static_build =
                    Some(self.read_static_bool(self.nth_arg(stage.span, name, args, 0)?)?);
            }
            ("Package", "target") => {
                fields.target = Some(self.one_string(stage.span, name, args)?);
            }
            ("Package", "allocator") => {
                fields.allocator =
                    Some(self.read_allocator(self.nth_arg(stage.span, name, args, 0)?)?);
            }
            ("Package", "allowSlowAllocator") => {
                fields.allow_slow_allocator =
                    Some(self.read_static_bool(self.nth_arg(stage.span, name, args, 0)?)?);
            }
            ("Package", "cFree") => {
                fields.c_free =
                    Some(self.read_static_bool(self.nth_arg(stage.span, name, args, 0)?)?);
            }
            ("Package", "declares") => {
                fields.capabilities =
                    self.read_capabilities(self.nth_arg(stage.span, name, args, 0)?)?;
            }
            ("Package", "accepts") => {
                fields.capabilities_accept =
                    self.read_capabilities(self.nth_arg(stage.span, name, args, 0)?)?;
            }
            ("Package", "wasm") => {
                fields.wasm = self.read_wasm(self.nth_arg(stage.span, name, args, 0)?)?;
            }
            _ => {
                return Err(self.reject(
                    stage.span,
                    &format!(
                        "`{module}.{name}` is not a package-pipeline stage — expected a \
                         `Package.*` builder"
                    ),
                ));
            }
        }
        Ok(())
    }

    /// Require `expr` to be a call whose callee is a qualified blessed name
    /// (`Module.name`), returning `(module, name, args)`. A callee that is a
    /// local name, a lambda application, or anything but a `Module.name`
    /// qualifier is rejected — no user-defined function can appear in a stage.
    fn expect_blessed_call<'e>(
        &self,
        expr: &'e Expr,
    ) -> Result<(&str, &str, &'e [Expr]), CliError> {
        match &expr.value {
            Expr_::Call(callee, args) => match &callee.value {
                Expr_::VarQual(m, n) => Ok((self.text(*m), self.text(*n), args.as_slice())),
                _ => Err(self.reject(
                    callee.span,
                    "a package stage's callee must be a blessed `Module.builder` name, never a \
                     local binding, lambda, or computed function",
                )),
            },
            // A bare nullary constructor written where a call was expected
            // (`Package.postgres` with no application) is a `VarQual` atom, not a
            // Call. It is handled by the nullary-constructor readers, never here.
            Expr_::VarQual(m, n) => Ok((self.text(*m), self.text(*n), &[])),
            _ => Err(self.reject(
                expr.span,
                "expected a blessed builder call in the package pipeline",
            )),
        }
    }

    /// Require exactly one argument and read it as a string literal.
    fn one_string(&self, span: Span, builder: &str, args: &[Expr]) -> Result<String, CliError> {
        if args.len() != 1 {
            return Err(self.reject(
                span,
                &format!(
                    "`{builder}` takes exactly one string argument, got {}",
                    args.len()
                ),
            ));
        }
        self.expect_string(self.nth_arg(span, builder, args, 0)?)
    }

    /// Fetch the `idx`-th argument, rejecting a builder given too few.
    fn nth_arg<'e>(
        &self,
        span: Span,
        builder: &str,
        args: &'e [Expr],
        idx: usize,
    ) -> Result<&'e Expr, CliError> {
        args.get(idx).ok_or_else(|| {
            self.reject(
                span,
                &format!("`{builder}` is missing argument #{}", idx + 1),
            )
        })
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

    /// Read the `Package.database` argument: one of the blessed driver
    /// constructors `Package.sqlite` / `Package.postgres`. The driver is
    /// unrepresentable-by-construction here, strengthening the `ipe.toml`
    /// `parse_db_driver` string check into the vocabulary.
    fn read_driver(&self, expr: &Expr) -> Result<ipe_backend_rust::DbDriver, CliError> {
        let (module, name) = self.expect_nullary(expr, "a database driver")?;
        match (module, name) {
            ("Package", "sqlite") => Ok(ipe_backend_rust::DbDriver::Sqlite),
            ("Package", "postgres") => Ok(ipe_backend_rust::DbDriver::Postgres),
            _ => Err(self.reject(
                expr.span,
                &format!(
                    "`{module}.{name}` is not a database driver — use `Package.sqlite` or \
                     `Package.postgres`"
                ),
            )),
        }
    }

    /// Read a boolean field (`Package.static` / `allowSlowAllocator` / `cFree`)
    /// written as one of the blessed nullary constructors `Static.on` /
    /// `Static.off`. Booleans are dedicated builders rather than bare `True` /
    /// `False`, so the reader never recognises a raw constructor reference.
    fn read_static_bool(&self, expr: &Expr) -> Result<bool, CliError> {
        let (module, name) = self.expect_nullary(expr, "an on/off switch")?;
        match (module, name) {
            ("Static", "on") => Ok(true),
            ("Static", "off") => Ok(false),
            _ => Err(self.reject(
                expr.span,
                &format!("`{module}.{name}` is not a switch — use `Static.on` or `Static.off`"),
            )),
        }
    }

    /// Read the `Package.allocator` argument: one of the blessed allocator
    /// constructors, mapped to its [`crate::build_plan::AllocatorChoice`] wire
    /// name so the closed set stays the single source of truth.
    fn read_allocator(&self, expr: &Expr) -> Result<crate::build_plan::AllocatorChoice, CliError> {
        let (module, name) = self.expect_nullary(expr, "an allocator")?;
        let wire = match (module, name) {
            ("Package", "autoAlloc") => "auto",
            ("Package", "system") => "system",
            ("Package", "dlmalloc") => "dlmalloc",
            _ => {
                return Err(self.reject(
                    expr.span,
                    &format!(
                        "`{module}.{name}` is not an allocator — use `Package.system`, \
                         `Package.dlmalloc`, or `Package.autoAlloc`"
                    ),
                ));
            }
        };
        crate::build_plan::AllocatorChoice::parse(wire)
            .map_err(|e| self.reject(expr.span, &format!("{e}")))
    }

    /// Read the `Package.dependencies [ … ]` list into the typed [`IpeDep`] map.
    /// Each element is a blessed `Package.dep` / `depGit` / `depGitRev` /
    /// `depPath` call — the three-distinct-builders shape mirrors the `IpeDep`
    /// sum, so a dependency is exactly one of index / git / path, never a bag of
    /// optional keys.
    fn read_dependencies(&self, expr: &Expr) -> Result<BTreeMap<String, IpeDep>, CliError> {
        let mut deps = BTreeMap::new();
        for item in self.expect_list(expr)? {
            let (name, dep) = self.read_one_dep(item)?;
            deps.insert(name, dep);
        }
        Ok(deps)
    }

    /// Read one dependency builder into its `(name, IpeDep)`.
    fn read_one_dep(&self, expr: &Expr) -> Result<(String, IpeDep), CliError> {
        let (module, builder, args) = self.expect_blessed_call(expr)?;
        if module != "Package" {
            return Err(self.reject(
                expr.span,
                &format!("`{module}.{builder}` is not a dependency builder"),
            ));
        }
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
            _ => Err(self.reject(
                expr.span,
                &format!(
                    "`Package.{builder}` is not a dependency builder — use `dep`, `depGit`, \
                     `depGitRev`, or `depPath`"
                ),
            )),
        }
    }

    /// Read the `Package.rustDependencies [ … ]` list into the typed
    /// [`RustDep`] map. Each element is a `Package.rustDep name version`,
    /// optionally piped into `Rust.features [ … ]`.
    fn read_rust_dependencies(&self, expr: &Expr) -> Result<BTreeMap<String, RustDep>, CliError> {
        let mut deps = BTreeMap::new();
        for item in self.expect_list(expr)? {
            let (name, dep) = self.read_one_rust_dep(item)?;
            deps.insert(name, dep);
        }
        Ok(deps)
    }

    /// Read one rust dependency: a `Package.rustDep` head, threaded through any
    /// number of `|> Rust.features [ … ]` refinements. Each refinement's `|>`
    /// spine is linearised the same way the top-level pipeline is.
    fn read_one_rust_dep(&self, expr: &Expr) -> Result<(String, RustDep), CliError> {
        let stages = self.linearise_pipeline(expr)?;
        let mut name: Option<String> = None;
        let mut dep = RustDep::default();
        for stage in stages {
            let (module, builder, args) = self.expect_blessed_call(stage)?;
            match (module, builder) {
                ("Package", "rustDep") => {
                    let n = self.expect_string(self.nth_arg(stage.span, builder, args, 0)?)?;
                    dep.version =
                        self.expect_string(self.nth_arg(stage.span, builder, args, 1)?)?;
                    name = Some(n);
                }
                ("Rust", "features") => {
                    dep.features =
                        self.expect_string_list(self.nth_arg(stage.span, builder, args, 0)?)?;
                }
                _ => {
                    return Err(self.reject(
                        stage.span,
                        &format!(
                            "`{module}.{builder}` is not a rust-dependency builder — use \
                             `Package.rustDep` and `Rust.features`"
                        ),
                    ));
                }
            }
        }
        let name = name.ok_or_else(|| {
            self.reject(
                expr.span,
                "a rust dependency must begin with `Package.rustDep name version`",
            )
        })?;
        Ok((name, dep))
    }

    /// Read a `Package.wrapper (Rust.wrapper "…" |> Rust.expose [ … ] |>
    /// Rust.wrapperCaps [ … ])` sub-value, validating it through the SAME
    /// [`ipe_ffi::wrapper::WrapperManifest::parse`] gate the `ipe.toml` path uses:
    /// the wrapper path is package-jailed and every declared capability is
    /// checked against the closed vocabulary. The parsed result is discarded here
    /// (the [`ProjectManifest`] carries only `has_rust_wrapper`); the point is
    /// that a bad wrapper is a read-time error, exactly as in `ipe.toml`.
    fn read_wrapper(&self, expr: &Expr) -> Result<(), CliError> {
        let stages = self.linearise_pipeline(expr)?;
        let mut path: Option<String> = None;
        let mut expose: Vec<String> = Vec::new();
        let mut capabilities: Vec<String> = Vec::new();
        for stage in stages {
            let (module, builder, args) = self.expect_blessed_call(stage)?;
            match (module, builder) {
                ("Rust", "wrapper") => {
                    path = Some(self.expect_string(self.nth_arg(stage.span, builder, args, 0)?)?);
                }
                ("Rust", "expose") => {
                    expose =
                        self.expect_string_list(self.nth_arg(stage.span, builder, args, 0)?)?;
                }
                ("Rust", "wrapperCaps") => {
                    capabilities =
                        self.read_capability_names(self.nth_arg(stage.span, builder, args, 0)?)?;
                }
                _ => {
                    return Err(self.reject(
                        stage.span,
                        &format!(
                            "`{module}.{builder}` is not a wrapper builder — use `Rust.wrapper`, \
                             `Rust.expose`, and `Rust.wrapperCaps`"
                        ),
                    ));
                }
            }
        }
        let path = path.ok_or_else(|| {
            self.reject(
                expr.span,
                "a wrapper must begin with `Rust.wrapper \"<local path>\"`",
            )
        })?;
        // Reuse the wrapper gate verbatim: package-jailed path + closed-vocabulary
        // capabilities. Its Diagnostic is surfaced as the reader's rejection.
        ipe_ffi::wrapper::WrapperManifest::parse(&path, &expose, &capabilities)
            .map_err(|diag| self.reject(expr.span, &format!("wrapper rejected: {diag}")))?;
        Ok(())
    }

    /// Read the `Package.wasm ( Wasm.spa |> … )` sub-value. `Wasm.spa` /
    /// `Wasm.hydrate` set the mode; the refinements set entry / mount / publicEnv
    /// / optLevel. The `publicEnv` list runs the UNCHANGED secret-name denylist.
    fn read_wasm(&self, expr: &Expr) -> Result<WasmConfig, CliError> {
        let stages = self.linearise_pipeline(expr)?;
        let mut wasm = WasmConfig::default();
        for stage in stages {
            // The head may be a bare `Wasm.spa` / `Wasm.hydrate` nullary atom.
            if let Some(mode) = self.wasm_mode_atom(stage) {
                wasm.mode = Some(mode.to_owned());
                continue;
            }
            let (module, builder, args) = self.expect_blessed_call(stage)?;
            match (module, builder) {
                ("Wasm", "spa") => wasm.mode = Some("spa".to_owned()),
                ("Wasm", "hydrate") => wasm.mode = Some("hydrate".to_owned()),
                ("Wasm", "entry") => {
                    wasm.entry =
                        Some(self.expect_string(self.nth_arg(stage.span, builder, args, 0)?)?);
                }
                ("Wasm", "mount") => {
                    wasm.mount =
                        Some(self.expect_string(self.nth_arg(stage.span, builder, args, 0)?)?);
                }
                ("Wasm", "publicEnv") => {
                    let names =
                        self.expect_string_list(self.nth_arg(stage.span, builder, args, 0)?)?;
                    self.check_public_env(stage.span, &names)?;
                    wasm.public_env = names;
                }
                ("Wasm", "optLevel") => {
                    wasm.opt_level =
                        Some(self.expect_string(self.nth_arg(stage.span, builder, args, 0)?)?);
                }
                _ => {
                    return Err(self.reject(
                        stage.span,
                        &format!(
                            "`{module}.{builder}` is not a wasm builder — use `Wasm.spa` / \
                             `Wasm.hydrate` and the `Wasm.*` refinements"
                        ),
                    ));
                }
            }
        }
        Ok(wasm)
    }

    /// The wasm mode named by a bare `Wasm.spa` / `Wasm.hydrate` nullary atom
    /// (not a call), or `None` for anything else.
    fn wasm_mode_atom(&self, expr: &Expr) -> Option<&'static str> {
        if let Expr_::VarQual(m, n) = &expr.value {
            match (self.text(*m), self.text(*n)) {
                ("Wasm", "spa") => return Some("spa"),
                ("Wasm", "hydrate") => return Some("hydrate"),
                _ => {}
            }
        }
        None
    }

    /// Run the UNCHANGED `[wasm] publicEnv` secret-name denylist over the
    /// extracted names. A denylisted name is a read-time build error, identical
    /// to the `ipe.toml` path — the single most security-load-bearing check,
    /// preserved verbatim via the shared [`is_denylisted_public_env_name`].
    fn check_public_env(&self, span: Span, names: &[String]) -> Result<(), CliError> {
        for name in names {
            if is_denylisted_public_env_name(name) {
                return Err(self.reject(
                    span,
                    &format!(
                        "`Wasm.publicEnv` lists {name:?}, which matches the secret-name denylist \
                         (*_SECRET / *_TOKEN / *_KEY / *_PASSWORD / DATABASE_URL / the internal \
                         IPE_* namespace) — a secret environment variable can never be \
                         allowlisted into the public wasm bundle"
                    ),
                ));
            }
        }
        Ok(())
    }

    /// Read a `[ Capability.network, Capability.clock, … ]` list into the typed
    /// capability set, via the shared [`Capability`] `FromStr`. An unknown
    /// capability is a named hard error — the typo-drops-a-capability footgun
    /// stays closed.
    fn read_capabilities(&self, expr: &Expr) -> Result<BTreeSet<Capability>, CliError> {
        let names = self.read_capability_names(expr)?;
        let mut set = BTreeSet::new();
        for name in names {
            let cap = name
                .parse::<Capability>()
                .map_err(|e| self.reject(expr.span, &format!("unknown capability: {e}")))?;
            set.insert(cap);
        }
        Ok(set)
    }

    /// Read a `[ Capability.* , … ]` list into the capability *wire names* the
    /// shared validators consume. Each element must be a blessed `Capability.*`
    /// nullary constructor; its builder suffix is mapped to the wire name (e.g.
    /// `Capability.nativeFfi` → `native-ffi`).
    fn read_capability_names(&self, expr: &Expr) -> Result<Vec<String>, CliError> {
        let mut names = Vec::new();
        for item in self.expect_list(expr)? {
            let (module, name) = self.expect_nullary(item, "a capability")?;
            if module != "Capability" {
                return Err(self.reject(
                    item.span,
                    &format!("`{module}.{name}` is not a capability — use `Capability.*`"),
                ));
            }
            names.push(capability_wire_name(name).to_owned());
        }
        Ok(names)
    }

    /// Require `expr` to be a bare `Module.name` nullary constructor reference
    /// (never a call, never a local name, never a computed value), returning
    /// `(module, name)`. `what` names the expected kind in the error.
    fn expect_nullary<'e>(
        &'e self,
        expr: &Expr,
        what: &str,
    ) -> Result<(&'e str, &'e str), CliError> {
        match &expr.value {
            Expr_::VarQual(m, n) => Ok((self.text(*m), self.text(*n))),
            _ => Err(self.reject(
                expr.span,
                &format!(
                    "expected {what} as a blessed `Module.name` constructor, never a computed \
                     value or a local binding"
                ),
            )),
        }
    }
}

/// The wire name of a `Capability.<builder>` reference. camelCase builder
/// suffixes map to the hyphenated wire spelling the shared [`Capability`]
/// `FromStr` consumes; all others pass through verbatim (they already match).
fn capability_wire_name(builder: &str) -> &str {
    match builder {
        "nativeFfi" => "native-ffi",
        "ffiRaw" => "ffi-raw",
        other => other,
    }
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
            &format!("{HEADER}package =\n    Package.named \"my-app\"\n"),
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
             \x20   Package.named \"my-app\"\n\
             \x20       |> Package.version \"0.3.0\"\n\
             \x20       |> Package.sourceRoot \"src\"\n\
             \x20       |> Package.database Package.postgres\n\
             \x20       |> Package.dependencies\n\
             \x20           [ Package.dep \"ipe-http\" \"^1.2\"\n\
             \x20           , Package.depGitRev \"ipe-widgets\" \"https://example.test/w.git\" \"a1b2c3\"\n\
             \x20           , Package.depGit \"ipe-plain\" \"https://example.test/p.git\"\n\
             \x20           , Package.depPath \"ipe-local\" \"../local\"\n\
             \x20           ]\n\
             \x20       |> Package.rustDependencies\n\
             \x20           [ Package.rustDep \"uuid\" \"1.10\"\n\
             \x20           , Package.rustDep \"image\" \"0.25\" |> Rust.features [ \"png\", \"jpeg\" ]\n\
             \x20           ]\n\
             \x20       |> Package.static Static.on\n\
             \x20       |> Package.target \"x86_64-unknown-linux-musl\"\n\
             \x20       |> Package.allocator Package.dlmalloc\n\
             \x20       |> Package.allowSlowAllocator Static.off\n\
             \x20       |> Package.cFree Static.on\n\
             \x20       |> Package.declares [ Capability.network, Capability.clock ]\n\
             \x20       |> Package.accepts [ Capability.unsafe ]\n\
             \x20       |> Package.wasm\n\
             \x20           (Wasm.spa\n\
             \x20               |> Wasm.entry \"src/Client.ipe\"\n\
             \x20               |> Wasm.mount \"#app\"\n\
             \x20               |> Wasm.publicEnv [ \"API_BASE_URL\", \"APP_VERSION\" ]\n\
             \x20               |> Wasm.optLevel \"z\"\n\
             \x20           )\n"
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
    fn reject_import() {
        let r = read(
            "reject_import",
            &format!("{HEADER}import Ipe.String\n\npackage =\n    Package.named \"x\"\n"),
        );
        assert_rejected(&r);
    }

    #[test]
    fn reject_computed_argument() {
        // A field written as a computed `if` expression, not a literal.
        let r = read(
            "reject_computed",
            &format!("{HEADER}package =\n    Package.named (if True then \"a\" else \"b\")\n"),
        );
        assert_rejected(&r);
    }

    #[test]
    fn reject_reference_to_another_binding() {
        // A second top-level binding referenced from the package value — the
        // reader forbids any binding other than `package`.
        let r = read(
            "reject_ref",
            &format!("{HEADER}n =\n    \"x\"\n\npackage =\n    Package.named n\n"),
        );
        assert_rejected(&r);
    }

    #[test]
    fn reject_non_blessed_callee() {
        // A user-looking `Foo.bar` stage is not a blessed builder.
        let r = read(
            "reject_callee",
            &format!("{HEADER}package =\n    Package.named \"x\"\n        |> Foo.bar \"y\"\n"),
        );
        assert_rejected(&r);
    }

    #[test]
    fn reject_non_pipe_operator() {
        // The spine must be threaded with `|>`, never another operator.
        let r = read(
            "reject_operator",
            &format!("{HEADER}package =\n    Package.named \"x\" ++ \"y\"\n"),
        );
        assert_rejected(&r);
    }

    #[test]
    fn reject_denylisted_public_env() {
        // A secret-name in publicEnv is a read-time build error — the preserved,
        // security-load-bearing denylist.
        let r = read(
            "reject_secret_env",
            &format!(
                "{HEADER}package =\n    Package.named \"x\"\n        |> Package.wasm (Wasm.spa |> Wasm.publicEnv [ \"DATABASE_URL\" ])\n"
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
    fn reject_unknown_driver_constructor() {
        // Only `Package.sqlite` / `Package.postgres` are drivers.
        let r = read(
            "reject_driver",
            &format!(
                "{HEADER}package =\n    Package.named \"x\"\n        |> Package.database Package.mysql\n"
            ),
        );
        assert_rejected(&r);
    }

    #[test]
    fn reject_unknown_capability() {
        let r = read(
            "reject_capability",
            &format!(
                "{HEADER}package =\n    Package.named \"x\"\n        |> Package.declares [ Capability.telepathy ]\n"
            ),
        );
        assert_rejected(&r);
    }

    #[test]
    fn reject_malformed_version() {
        let r = read(
            "reject_version",
            &format!(
                "{HEADER}package =\n    Package.named \"x\"\n        |> Package.version \"not-a-version\"\n"
            ),
        );
        assert_rejected(&r);
    }

    #[test]
    fn reject_missing_name() {
        // A `package` binding that never calls `Package.named`.
        let r = read(
            "reject_no_name",
            &format!("{HEADER}package =\n    Package.version \"1.0.0\"\n"),
        );
        assert_rejected(&r);
    }

    #[test]
    fn reject_package_as_function() {
        let r = read(
            "reject_function",
            &format!("{HEADER}package x =\n    Package.named x\n"),
        );
        assert_rejected(&r);
    }

    #[test]
    fn reject_extra_binding() {
        let r = read(
            "reject_extra",
            &format!(
                "{HEADER}package =\n    Package.named \"x\"\n\nother =\n    Package.named \"y\"\n"
            ),
        );
        assert_rejected(&r);
    }

    #[test]
    fn reject_bad_wrapper_path_escapes_jail() {
        // An absolute wrapper path escapes the package jail (WrapperPath gate).
        let r = read(
            "reject_wrapper",
            &format!(
                "{HEADER}package =\n    Package.named \"x\"\n        |> Package.wrapper (Rust.wrapper \"/etc/passwd\" |> Rust.expose [ \"f\" ])\n"
            ),
        );
        assert_rejected(&r);
    }

    #[test]
    fn accepts_a_valid_wrapper() {
        let r = read(
            "ok_wrapper",
            &format!(
                "{HEADER}package =\n    Package.named \"x\"\n        |> Package.wrapper (Rust.wrapper \"./vendor/mycrate\" |> Rust.expose [ \"encode\" ])\n"
            ),
        );
        let m = r.expect("valid wrapper must parse");
        assert!(m.has_rust_wrapper);
    }

    #[test]
    fn allowed_public_env_passes() {
        let r = read(
            "ok_env",
            &format!(
                "{HEADER}package =\n    Package.named \"x\"\n        |> Package.wasm (Wasm.spa |> Wasm.publicEnv [ \"API_BASE_URL\" ])\n"
            ),
        );
        let m = r.expect("allowed env must parse");
        assert_eq!(m.wasm.public_env, vec!["API_BASE_URL"]);
    }

    #[test]
    fn line_col_is_one_based_and_total() {
        assert_eq!(line_col("abc", 0), (1, 1));
        assert_eq!(line_col("abc", 2), (1, 3));
        assert_eq!(line_col("a\nbc", 2), (2, 1));
        // An out-of-range offset degrades gracefully rather than panicking.
        assert_eq!(line_col("a", 999), (1, 2));
    }
}
