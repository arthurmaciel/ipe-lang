//! Every runtime `cfg` region is linted by a CI clippy leg, or is pinned.
//!
//! A lint ban (`disallowed-methods`, `unwrap_used`, …) is only as wide as the
//! code clippy compiles, and clippy compiles only the `cfg` configurations a CI
//! leg selects. This test reads the clippy legs out of `.github/workflows/ci.yml`
//! (the `clippy` and `runtime-feature-combos-run` jobs), resolves each leg's
//! feature set through the runtime's `[features]` graph, and evaluates every
//! `#[cfg]` / `#[cfg_attr]` region of the runtime's sources (the library, every
//! integration test, and `build.rs`) with `cfg-expr` against those legs. A
//! region is covered when some leg compiles the target that holds it with a
//! configuration under which the region's whole ancestor `cfg` conjunction
//! holds.
//!
//! An uncovered region must be compiled by one of the named candidate legs in
//! `CANDIDATES` (the first that compiles it is charged), and the per-candidate,
//! per-file charge must equal `RESIDUAL` exactly: a new uncovered region, a
//! region no candidate compiles, or a region a CI leg came to cover each turn
//! the test red, and the failure prints the computed table as a Rust literal.
//!
//! The model is closed, so an unrecognised input fails rather than shrinking
//! what is checked: a clippy flag outside the modelled grammar, a `cfg` name or
//! feature the model does not know, a `cfg` on a syntax position the walk does
//! not track, an ambiguous or missing module file, a source file no module
//! reaches, and a runtime clippy line outside the modelled jobs are refused.
#![cfg(not(target_arch = "wasm32"))]

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::rc::Rc;

use cfg_expr::targets::{Env, TargetInfo, Triple, get_builtin_target_by_triple};
use cfg_expr::{Expression, Predicate};
use proc_macro2::{Delimiter, TokenStream, TokenTree};
use syn::ext::IdentExt;
use syn::visit::{self, Visit};
use syn::{
    Arm, Attribute, Expr, Field, FieldPat, FieldValue, FnPtrVariadic, ForeignItem, GenericParam,
    ImplItem, Item, ItemMod, Lit, LitStr, Local, Meta, NamedArg, Pat, PatType, Receiver, StmtMacro,
    TraitItem, Type, Variadic, Variant, WherePredicate,
};

#[path = "support/source_tree.rs"]
mod source_tree;

/// The package whose `cfg` regions are checked.
const RUNTIME_PACKAGE: &str = "ipe-runtime-rust";

/// The triple a CI clippy leg without `--target` compiles for.
///
/// Every job in `CLIPPY_JOBS` must run on an `ubuntu-` runner, so a leg's
/// default target is this one.
const HOST_TRIPLE: &str = "x86_64-unknown-linux-gnu";

/// The CI jobs whose `cargo clippy` lines are the real legs.
///
/// A `cargo clippy` line that lints the runtime from any other job is refused,
/// so a leg cannot be counted from a job the required set does not run.
const CLIPPY_JOBS: &[&str] = &["clippy", "runtime-feature-combos-run"];

/// The bare `cfg` names a region may test; each is unset unless a candidate sets it.
const FIXED_FLAGS: &[&str] = &["ipe_asan", "miri", "doc"];

/// The most module files one walk opens.
const MAX_MODULE_FILES: usize = 4096;

/// The deepest chain of out-of-line modules the walk follows.
const MAX_MODULE_DEPTH: usize = 64;

/// The most features one closure visits.
const MAX_FEATURE_STEPS: usize = 4096;

/// The deepest macro token-group nesting the scan descends.
const MAX_TOKEN_DEPTH: usize = 256;

/// A clippy leg CI does not run, named so an uncovered region is charged to it.
struct Candidate {
    name: &'static str,
    command: &'static str,
    flags: &'static [&'static str],
}

/// The legs that would cover today's uncovered regions, in charge order.
const CANDIDATES: &[Candidate] = &[
    Candidate {
        name: "wasm32-unknown-unknown",
        command: "cargo clippy -p ipe-runtime-rust --target wasm32-unknown-unknown \
                  --no-default-features --features wasm-client,debugger --all-targets \
                  -- -D warnings",
        flags: &[],
    },
    Candidate {
        name: "wasm32-wasip1",
        command: "cargo clippy -p ipe-runtime-rust --target wasm32-wasip1 \
                  --no-default-features --features \
                  time,log,json,uuid,random,decimal,regex,crypto-core,secret,char-category \
                  --lib -- -D warnings",
        flags: &[],
    },
    Candidate {
        name: "webview",
        command: "cargo clippy -p ipe-runtime-rust --features webview --all-targets -- -D warnings",
        flags: &[],
    },
    Candidate {
        name: "x86_64-apple-darwin",
        command: "cargo clippy -p ipe-runtime-rust --target x86_64-apple-darwin \
                  --features full --all-targets -- -D warnings",
        flags: &[],
    },
    Candidate {
        name: "x86_64-unknown-freebsd",
        command: "cargo clippy -p ipe-runtime-rust --target x86_64-unknown-freebsd \
                  --features full --all-targets -- -D warnings",
        flags: &[],
    },
    Candidate {
        name: "aarch64-linux-android",
        command: "cargo clippy -p ipe-runtime-rust --target aarch64-linux-android \
                  --features full --all-targets -- -D warnings",
        flags: &[],
    },
    Candidate {
        name: "x86_64-pc-windows-msvc",
        command: "cargo clippy -p ipe-runtime-rust --target x86_64-pc-windows-msvc \
                  --features full --all-targets -- -D warnings",
        flags: &[],
    },
    Candidate {
        name: "x86_64-unknown-netbsd",
        command: "cargo clippy -p ipe-runtime-rust --target x86_64-unknown-netbsd \
                  --features full --all-targets -- -D warnings",
        flags: &[],
    },
    Candidate {
        name: "asan",
        command: "cargo clippy -p ipe-runtime-rust --features full,debugger --all-targets \
                  -- -D warnings",
        flags: &["ipe_asan"],
    },
];

/// Rows of a residual table: a candidate, then each file's charged region count.
type ResidualRows<'a> = &'a [(&'a str, &'a [(&'a str, usize)])];

/// The uncovered `cfg` regions, per charged candidate and per file.
///
/// Exact: a missing, extra, or miscounted row fails the test, which prints the
/// computed table to paste here.
const RESIDUAL: ResidualRows<'static> = &[];

/// A residual table keyed by candidate, then by file.
type Residual = BTreeMap<String, BTreeMap<String, usize>>;

/// The runtime's `[features]` graph.
#[derive(Debug)]
struct FeatureGraph {
    features: BTreeMap<String, Vec<String>>,
    implicit: BTreeSet<String>,
}

impl FeatureGraph {
    /// The graph declared by `manifest`.
    ///
    /// An optional dependency never named as `dep:<name>` is an implicit
    /// feature of the same name.
    fn from_manifest(manifest: &toml::Table) -> Result<Self, String> {
        let mut features = BTreeMap::new();
        if let Some(table) = manifest.get("features") {
            let table = table.as_table().ok_or("`[features]` is not a table")?;
            for (name, members) in table {
                let members = members
                    .as_array()
                    .ok_or_else(|| format!("feature `{name}` is not an array"))?
                    .iter()
                    .map(|member| {
                        member
                            .as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| format!("feature `{name}` lists a non-string"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                features.insert(name.clone(), members);
            }
        }
        let mut optional = BTreeSet::new();
        for kind in ["dependencies", "build-dependencies"] {
            for (_, table) in dependency_tables(manifest, kind)? {
                for (name, spec) in table {
                    if spec.get("optional").and_then(toml::Value::as_bool) == Some(true) {
                        optional.insert(name.clone());
                    }
                }
            }
        }
        let referenced: BTreeSet<&str> = features
            .values()
            .flatten()
            .filter_map(|member| member.strip_prefix("dep:"))
            .collect();
        let implicit = optional
            .into_iter()
            .filter(|dep| !referenced.contains(dep.as_str()) && !features.contains_key(dep))
            .collect();
        Ok(Self { features, implicit })
    }

    /// Whether `name` is a feature of the package.
    fn is_known(&self, name: &str) -> bool {
        name == "default" || self.features.contains_key(name) || self.implicit.contains(name)
    }

    /// Every feature `seeds` enable.
    ///
    /// `dep:x` and the weak `x?/f` enable no feature; `x/f` enables `x` only
    /// when `x` is an implicit feature. A seed or member naming no feature is
    /// refused, as cargo refuses it.
    fn closure(&self, seeds: &BTreeSet<String>) -> Result<BTreeSet<String>, String> {
        let mut enabled = BTreeSet::new();
        let mut queue: Vec<String> = seeds.iter().cloned().collect();
        let mut steps = 0usize;
        while let Some(name) = queue.pop() {
            steps = steps.saturating_add(1);
            if steps > MAX_FEATURE_STEPS {
                return Err(format!(
                    "the feature closure passed {MAX_FEATURE_STEPS} steps"
                ));
            }
            if !self.is_known(&name) {
                return Err(format!("unknown feature `{name}`"));
            }
            if !enabled.insert(name.clone()) {
                continue;
            }
            for member in self.features.get(&name).into_iter().flatten() {
                if member.starts_with("dep:") {
                    continue;
                }
                match member.split_once('/') {
                    Some((dep, _)) => {
                        if self.implicit.contains(dep) {
                            queue.push(dep.to_owned());
                        }
                    }
                    None => queue.push(member.clone()),
                }
            }
        }
        Ok(enabled)
    }
}

/// The parts of the runtime manifest the model reads.
#[derive(Debug)]
struct Package {
    graph: FeatureGraph,
    lib_root: String,
}

impl Package {
    /// The package declared by `text`.
    ///
    /// Explicit target tables and turned-off target discovery are refused: the
    /// walk models only the library root, `tests/` discovery, and `build.rs`.
    fn from_manifest(text: &str) -> Result<Self, String> {
        let manifest: toml::Table =
            toml::from_str(text).map_err(|e| format!("runtime manifest: {e}"))?;
        for key in ["bin", "test", "bench", "example"] {
            if manifest.contains_key(key) {
                return Err(format!("`[[{key}]]` targets are not modelled"));
            }
        }
        if let Some(package) = manifest.get("package") {
            for key in [
                "autotests",
                "autobins",
                "autobenches",
                "autoexamples",
                "build",
            ] {
                if package.get(key).is_some() {
                    return Err(format!("`package.{key}` is not modelled"));
                }
            }
        }
        let lib_root = manifest
            .get("lib")
            .and_then(|lib| lib.get("path"))
            .and_then(toml::Value::as_str)
            .unwrap_or("src/lib.rs")
            .to_owned();
        Ok(Self {
            graph: FeatureGraph::from_manifest(&manifest)?,
            lib_root,
        })
    }
}

/// A dependency table of `manifest` and its `target.<key>` qualifier, if any.
type DependencyTable<'m> = (Option<&'m str>, &'m toml::Table);

/// The `kind` dependency tables of `manifest`: the top-level one and every `target.<key>` one.
fn dependency_tables<'m>(
    manifest: &'m toml::Table,
    kind: &str,
) -> Result<Vec<DependencyTable<'m>>, String> {
    let mut tables = Vec::new();
    if let Some(table) = manifest.get(kind) {
        tables.push((
            None,
            table
                .as_table()
                .ok_or_else(|| format!("`[{kind}]` is not a table"))?,
        ));
    }
    if let Some(targets) = manifest.get("target") {
        let targets = targets.as_table().ok_or("`[target]` is not a table")?;
        for (key, target) in targets {
            if let Some(table) = target.get(kind) {
                tables.push((
                    Some(key.as_str()),
                    table
                        .as_table()
                        .ok_or_else(|| format!("`[target.{key}.{kind}]` is not a table"))?,
                ));
            }
        }
    }
    Ok(tables)
}

/// Whether the `[target.<key>]` qualifier `key` holds on the CI host.
fn host_target_key(key: &str, host: &TargetInfo) -> Result<bool, String> {
    let Some(inner) = key.strip_prefix("cfg(").and_then(|k| k.strip_suffix(')')) else {
        return Ok(key == host.triple.as_str());
    };
    let expr = Expression::parse(inner).map_err(|e| format!("target key `{key}`: {e}"))?;
    if expr
        .predicates()
        .any(|predicate| !matches!(predicate, Predicate::Target(_)))
    {
        return Err(format!("target key `{key}` tests more than the target"));
    }
    Ok(expr.eval(|predicate| match predicate {
        Predicate::Target(target) => target.matches(host),
        _ => false,
    }))
}

/// The runtime features a `--workspace` build unifies.
///
/// Every member that depends on the runtime adds its `features` (and
/// `default`, unless it turns default features off). A dependency the model
/// cannot read exactly is refused.
fn workspace_seeds(
    root_manifest: &str,
    host: &TargetInfo,
    read_member: impl Fn(&str) -> Result<String, String>,
) -> Result<BTreeSet<String>, String> {
    let root: toml::Table =
        toml::from_str(root_manifest).map_err(|e| format!("root manifest: {e}"))?;
    if root.contains_key("package") {
        return Err("the root manifest is not a virtual workspace".to_owned());
    }
    let members = root
        .get("workspace")
        .and_then(|workspace| workspace.get("members"))
        .and_then(toml::Value::as_array)
        .ok_or("the root manifest lists no `workspace.members`")?;
    let mut seeds = BTreeSet::from(["default".to_owned()]);
    let mut runtime_is_member = false;
    for member in members {
        let member = member
            .as_str()
            .ok_or("a workspace member is not a string")?;
        if member.contains(['*', '?', '[']) {
            return Err(format!("workspace member glob `{member}` is not modelled"));
        }
        let manifest: toml::Table = toml::from_str(&read_member(member)?)
            .map_err(|e| format!("{member}/Cargo.toml: {e}"))?;
        let name = manifest
            .get("package")
            .and_then(|package| package.get("name"))
            .and_then(toml::Value::as_str);
        if name == Some(RUNTIME_PACKAGE) {
            runtime_is_member = true;
            continue;
        }
        member_seeds(member, &manifest, host, &mut seeds)?;
    }
    if !runtime_is_member {
        return Err(format!("`{RUNTIME_PACKAGE}` is not a workspace member"));
    }
    Ok(seeds)
}

/// Adds the runtime features one workspace member's dependencies request to `seeds`.
fn member_seeds(
    member: &str,
    manifest: &toml::Table,
    host: &TargetInfo,
    seeds: &mut BTreeSet<String>,
) -> Result<(), String> {
    let kinds = [
        "dependencies",
        "dev-dependencies",
        "dev_dependencies",
        "build-dependencies",
        "build_dependencies",
    ];
    for kind in kinds {
        for (target, table) in dependency_tables(manifest, kind)? {
            for (key, spec) in table {
                let package = spec
                    .get("package")
                    .and_then(toml::Value::as_str)
                    .unwrap_or(key);
                if package != RUNTIME_PACKAGE {
                    continue;
                }
                let at = format!("{member}: `{kind}.{key}`");
                if kind.starts_with("build") {
                    return Err(format!(
                        "{at}: a build dependency on the runtime is not modelled"
                    ));
                }
                if let Some(target) = target {
                    if !host_target_key(target, host)? {
                        continue;
                    }
                }
                let spec = spec
                    .as_table()
                    .ok_or_else(|| format!("{at} is not a table"))?;
                if spec.contains_key("workspace") || spec.contains_key("optional") {
                    return Err(format!("{at}: `workspace` / `optional` are not modelled"));
                }
                let defaults = spec
                    .get("default-features")
                    .or_else(|| spec.get("default_features"))
                    .and_then(toml::Value::as_bool);
                if defaults != Some(false) {
                    seeds.insert("default".to_owned());
                }
                for feature in spec
                    .get("features")
                    .and_then(toml::Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let feature = feature
                        .as_str()
                        .ok_or_else(|| format!("{at} lists a non-string feature"))?;
                    seeds.insert(feature.to_owned());
                }
            }
        }
    }
    Ok(())
}

/// Which integration tests a leg compiles.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Tests {
    Skipped,
    Every,
    Only(BTreeSet<String>),
}

/// One clippy invocation, as the configurations it compiles.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Leg {
    target: TargetInfo,
    features: BTreeSet<String>,
    /// The `test` values the library is compiled with; empty when the leg skips it.
    lib_tests: BTreeSet<bool>,
    tests: Tests,
    flags: BTreeSet<String>,
}

impl Leg {
    /// Whether the leg compiles the integration test `name`.
    fn compiles_test(&self, name: &str) -> bool {
        match &self.tests {
            Tests::Skipped => false,
            Tests::Every => true,
            Tests::Only(names) => names.contains(name),
        }
    }
}

/// The modelled target `triple`.
///
/// `cfg-expr` has no `wasm32-wasip1`; it is `wasm32-wasi` with the `p1` environment.
fn target_info(triple: &str) -> Result<TargetInfo, String> {
    if triple == "wasm32-wasip1" {
        let mut wasi = get_builtin_target_by_triple("wasm32-wasi")
            .ok_or("`cfg-expr` has no `wasm32-wasi`")?
            .clone();
        wasi.triple = Triple::new_const("wasm32-wasip1");
        wasi.env = Some(Env::new_const("p1"));
        return Ok(wasi);
    }
    get_builtin_target_by_triple(triple)
        .cloned()
        .ok_or_else(|| format!("unknown target `{triple}`"))
}

/// The leg `line` runs, read through a closed grammar of `cargo clippy` flags.
///
/// `workspace` is the feature set a `--workspace` build unifies.
fn parse_clippy_command(
    line: &str,
    graph: &FeatureGraph,
    workspace: &BTreeSet<String>,
) -> Result<Leg, String> {
    let mut tokens = line.split_whitespace();
    if tokens.next() != Some("cargo") || tokens.next() != Some("clippy") {
        return Err(format!("`{line}` is not a `cargo clippy` line"));
    }
    let (mut all_targets, mut lib, mut no_default, mut package, mut whole) =
        (false, false, false, false, false);
    let mut features_given = false;
    let mut named_tests = BTreeSet::new();
    let mut seeds = BTreeSet::new();
    let mut triple: Option<&str> = None;
    while let Some(token) = tokens.next() {
        match token {
            "--offline" | "--locked" => {}
            "--all-targets" => all_targets = true,
            "--lib" => lib = true,
            "--no-default-features" => no_default = true,
            "--workspace" => whole = true,
            "--" => break,
            "--test" => {
                let name = tokens.next().ok_or("`--test` without a name")?;
                named_tests.insert(name.to_owned());
            }
            "--features" => {
                let list = tokens.next().ok_or("`--features` without a list")?;
                for feature in list.split(',') {
                    if feature.is_empty() || feature.contains('/') {
                        return Err(format!("unmodelled feature `{feature}` in `{line}`"));
                    }
                    seeds.insert(feature.to_owned());
                }
                features_given = true;
            }
            "--target" => {
                let name = tokens.next().ok_or("`--target` without a triple")?;
                if triple.replace(name).is_some() {
                    return Err(format!("`--target` given twice in `{line}`"));
                }
            }
            "-p" | "--package" => {
                let name = tokens.next().ok_or("`-p` without a package")?;
                if name != RUNTIME_PACKAGE {
                    return Err(format!(
                        "a clippy leg for another package `{name}`: `{line}`"
                    ));
                }
                package = true;
            }
            other => return Err(format!("unmodelled clippy flag `{other}` in `{line}`")),
        }
    }
    if package == whole {
        return Err(format!(
            "`{line}` needs exactly one of `-p` and `--workspace`"
        ));
    }
    if whole && (features_given || no_default) {
        return Err(format!(
            "`{line}`: feature flags on a workspace leg are not modelled"
        ));
    }
    if all_targets && (lib || !named_tests.is_empty()) {
        return Err(format!("`{line}`: `--all-targets` with a target selector"));
    }
    if whole {
        seeds.clone_from(workspace);
    } else if !no_default {
        seeds.insert("default".to_owned());
    }
    let (lib_tests, tests) = if all_targets {
        (BTreeSet::from([false, true]), Tests::Every)
    } else if lib || !named_tests.is_empty() {
        let lib_tests = if lib {
            BTreeSet::from([false])
        } else {
            BTreeSet::new()
        };
        let tests = if named_tests.is_empty() {
            Tests::Skipped
        } else {
            Tests::Only(named_tests)
        };
        (lib_tests, tests)
    } else {
        (BTreeSet::from([false]), Tests::Skipped)
    };
    Ok(Leg {
        target: target_info(triple.unwrap_or(HOST_TRIPLE))?,
        features: graph.closure(&seeds)?,
        lib_tests,
        tests,
        flags: BTreeSet::new(),
    })
}

/// A labelled leg: where it came from, and what it compiles.
type NamedLeg = (String, Leg);

/// Every clippy leg the jobs in `CLIPPY_JOBS` run.
///
/// Each job in the set must exist, run on an `ubuntu-` runner, and run at
/// least one leg; a runtime clippy line in any other job is refused.
fn ci_legs(
    yaml: &str,
    graph: &FeatureGraph,
    workspace: &BTreeSet<String>,
) -> Result<Vec<NamedLeg>, String> {
    let doc: serde_yaml::Value = serde_yaml::from_str(yaml).map_err(|e| format!("ci.yml: {e}"))?;
    let jobs = doc
        .get("jobs")
        .and_then(serde_yaml::Value::as_mapping)
        .ok_or("ci.yml has no `jobs` mapping")?;
    let mut legs = Vec::new();
    let mut found = BTreeSet::new();
    for (name, job) in jobs {
        let name = name.as_str().ok_or("a job name is not a string")?;
        let in_set = CLIPPY_JOBS.contains(&name);
        if in_set {
            found.insert(name);
            let runner = job
                .get("runs-on")
                .and_then(serde_yaml::Value::as_str)
                .unwrap_or_default();
            if !runner.starts_with("ubuntu-") {
                return Err(format!(
                    "job `{name}` runs on `{runner}`, not the modelled host"
                ));
            }
        }
        let before = legs.len();
        let runs = job
            .get("steps")
            .and_then(serde_yaml::Value::as_sequence)
            .into_iter()
            .flatten()
            .filter_map(|step| step.get("run").and_then(serde_yaml::Value::as_str));
        for run in runs {
            for line in run.lines().map(str::trim).filter(|l| !l.starts_with('#')) {
                if !in_set {
                    outside_line(name, line)?;
                } else if line.contains("clippy") {
                    if !line.starts_with("cargo clippy ") || line.ends_with('\\') {
                        return Err(format!("job `{name}`: unmodelled clippy line `{line}`"));
                    }
                    let leg = parse_clippy_command(line, graph, workspace)?;
                    legs.push((format!("{name}: {line}"), leg));
                }
            }
        }
        if in_set && legs.len() == before {
            return Err(format!("job `{name}` runs no clippy leg"));
        }
    }
    for job in CLIPPY_JOBS {
        if !found.contains(job) {
            return Err(format!("ci.yml has no job `{job}`"));
        }
    }
    Ok(legs)
}

/// Refuses a `cargo clippy` line outside `CLIPPY_JOBS` unless it names another package.
fn outside_line(job: &str, line: &str) -> Result<(), String> {
    let words: Vec<&str> = line.split_whitespace().collect();
    let Some(cargo) = words.iter().position(|word| *word == "cargo") else {
        return Ok(());
    };
    if !words.iter().skip(cargo).any(|word| *word == "clippy") {
        return Ok(());
    }
    let package = words
        .iter()
        .zip(words.iter().skip(1))
        .find(|(flag, _)| matches!(**flag, "-p" | "--package"))
        .map(|(_, name)| *name);
    match package {
        Some(name) if name != RUNTIME_PACKAGE => Ok(()),
        _ => Err(format!(
            "job `{job}` lints `{RUNTIME_PACKAGE}` outside the clippy job set: `{line}`"
        )),
    }
}

/// A compilation target of the runtime package.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Unit {
    Lib,
    Test(String),
    Build,
}

/// The configuration a `cfg` predicate is evaluated under.
struct Config<'a> {
    target: &'a TargetInfo,
    features: &'a BTreeSet<String>,
    test: bool,
    flags: &'a BTreeSet<String>,
}

/// Whether `expr` holds under `config`; `debug_assertions` holds in clippy's dev profile.
fn holds(expr: &Expression, config: &Config<'_>) -> bool {
    expr.eval(|predicate| match predicate {
        Predicate::Target(target) => target.matches(config.target),
        Predicate::Test => config.test,
        Predicate::DebugAssertions => true,
        Predicate::Feature(name) => config.features.contains(*name),
        Predicate::Flag(name) => config.flags.contains(*name),
        Predicate::ProcMacro | Predicate::TargetFeature(_) | Predicate::KeyValue { .. } => false,
    })
}

/// Whether `leg` compiles `unit` under a configuration where all of `conj` holds.
///
/// A build script is compiled for the host with `test` unset.
fn covers(leg: &Leg, unit: &Unit, conj: &[Rc<Expression>], host: &TargetInfo) -> bool {
    let all = |target: &TargetInfo, test: bool| {
        let config = Config {
            target,
            features: &leg.features,
            test,
            flags: &leg.flags,
        };
        conj.iter().all(|expr| holds(expr, &config))
    };
    match unit {
        Unit::Lib => leg.lib_tests.iter().any(|&test| all(&leg.target, test)),
        Unit::Test(name) => leg.compiles_test(name) && all(&leg.target, true),
        Unit::Build => all(host, false),
    }
}

/// A `cfg` region: the conjunction under which its code compiles.
struct Node {
    unit: Unit,
    file: String,
    conj: Vec<Rc<Expression>>,
    /// A `compile_error!` guard, which no CI leg may satisfy.
    guard: bool,
}

/// Where the walk is in the module tree.
#[derive(Clone, Debug, Default)]
struct Ctx {
    file: String,
    file_dir: String,
    child_dir: String,
    inline: bool,
}

/// The module-tree walk that collects every `cfg` region of one unit.
struct Walk<'a> {
    sources: &'a BTreeMap<String, String>,
    graph: &'a FeatureGraph,
    test_expr: Rc<Expression>,
    unit: Unit,
    stack: Vec<Rc<Expression>>,
    ctx: Ctx,
    /// `cfg` / `cfg_attr` attributes syn's traversal reached in the current file.
    seen: usize,
    /// `cfg` / `cfg_attr` attributes the walk evaluated in the current file.
    tracked: usize,
    nodes: Vec<Node>,
    refusals: BTreeSet<String>,
    reached: BTreeSet<String>,
    files: usize,
    depth: usize,
}

/// Whether `attr` is a `cfg` or `cfg_attr`.
fn is_cfg(attr: &Attribute) -> bool {
    attr.path().is_ident("cfg") || attr.path().is_ident("cfg_attr")
}

/// The directory part of the `/`-separated `key`.
fn parent(key: &str) -> String {
    key.rsplit_once('/')
        .map_or_else(String::new, |(dir, _)| dir.to_owned())
}

/// `dir` joined with `name`.
fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

/// `rel` resolved lexically against `dir`; `None` for an absolute path or one above the root.
fn normalize(dir: &str, rel: &str) -> Option<String> {
    if rel.starts_with('/') || rel.contains('\\') {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    for part in dir.split('/').chain(rel.split('/')) {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            name => parts.push(name),
        }
    }
    Some(parts.join("/"))
}

/// `tokens` split at its top-level commas.
fn split_commas(tokens: TokenStream) -> Vec<TokenStream> {
    let mut parts = vec![TokenStream::new()];
    for tree in tokens {
        if matches!(&tree, TokenTree::Punct(p) if p.as_char() == ',') {
            parts.push(TokenStream::new());
        } else if let Some(last) = parts.last_mut() {
            last.extend([tree]);
        }
    }
    parts
}

/// The attribute `cfg` or `cfg_attr` heads a macro-token `#[…]` with.
enum TokenAttr {
    Cfg,
    CfgAttr,
}

impl<'a> Walk<'a> {
    fn new(
        sources: &'a BTreeMap<String, String>,
        graph: &'a FeatureGraph,
        test_expr: Rc<Expression>,
    ) -> Self {
        Self {
            sources,
            graph,
            test_expr,
            unit: Unit::Lib,
            stack: Vec::new(),
            ctx: Ctx::default(),
            seen: 0,
            tracked: 0,
            nodes: Vec::new(),
            refusals: BTreeSet::new(),
            reached: BTreeSet::new(),
            files: 0,
            depth: 0,
        }
    }

    /// Walks `unit` from its root file `key`.
    fn run(&mut self, unit: Unit, key: &str, child_dir: String) {
        self.unit = unit;
        self.stack.clear();
        self.files = 0;
        self.depth = 0;
        self.walk_file(key, child_dir, false);
    }

    fn refuse(&mut self, message: &str) {
        let at: &str = if self.ctx.file.is_empty() {
            "<walk>"
        } else {
            &self.ctx.file
        };
        self.refusals.insert(format!("{at}: {message}"));
    }

    /// The current ancestor conjunction extended by `extra`.
    fn conj<'e>(&self, extra: impl IntoIterator<Item = &'e Rc<Expression>>) -> Vec<Rc<Expression>> {
        self.stack.iter().chain(extra).cloned().collect()
    }

    fn record(&mut self, conj: Vec<Rc<Expression>>, guard: bool) {
        self.nodes.push(Node {
            unit: self.unit.clone(),
            file: self.ctx.file.clone(),
            conj,
            guard,
        });
    }

    fn walk_file(&mut self, key: &str, child_dir: String, inline: bool) {
        if self.depth >= MAX_MODULE_DEPTH {
            self.refuse(&format!(
                "`{key}` nests past {MAX_MODULE_DEPTH} module files"
            ));
            return;
        }
        self.files = self.files.saturating_add(1);
        if self.files > MAX_MODULE_FILES {
            self.refuse(&format!(
                "the walk opened more than {MAX_MODULE_FILES} files"
            ));
            return;
        }
        let sources = self.sources;
        let Some(source) = sources.get(key) else {
            self.refuse(&format!("module file `{key}` does not exist"));
            return;
        };
        self.reached.insert(key.to_owned());
        let file = match syn::parse_file(source) {
            Ok(file) => file,
            Err(e) => {
                self.refuse(&format!("`{key}` does not parse: {e}"));
                return;
            }
        };
        let next = Ctx {
            file: key.to_owned(),
            file_dir: parent(key),
            child_dir,
            inline,
        };
        let saved_ctx = std::mem::replace(&mut self.ctx, next);
        let saved_counts = (
            std::mem::take(&mut self.seen),
            std::mem::take(&mut self.tracked),
        );
        self.depth = self.depth.saturating_add(1);
        let pushed = self.enter(&file.attrs, false);
        visit::visit_file(self, &file);
        self.leave(pushed);
        self.depth = self.depth.saturating_sub(1);
        if self.seen != self.tracked {
            let gap = self.seen.abs_diff(self.tracked);
            self.refuse(&format!(
                "{gap} cfg attribute(s) on an unmodelled syntax position"
            ));
        }
        self.ctx = saved_ctx;
        (self.seen, self.tracked) = saved_counts;
    }

    /// Records the regions `attrs` open and pushes their `cfg`s; returns how many were pushed.
    ///
    /// Each `cfg` is a region under the ancestors and all of the node's own
    /// `cfg`s; each `cfg_attr` is one more under its predicate. A `#[test]`
    /// item compiles only under `test`. A `guard` node is a `compile_error!`.
    fn enter(&mut self, attrs: &[Attribute], guard: bool) -> usize {
        let mut cfgs = Vec::new();
        let mut predicates = Vec::new();
        let mut test = false;
        for attr in attrs {
            if attr.path().is_ident("cfg") {
                self.tracked = self.tracked.saturating_add(1);
                if let Some(expr) = self
                    .list_tokens(attr)
                    .and_then(|t| self.parse_cfg(&t.to_string()))
                {
                    cfgs.push(Rc::new(expr));
                }
            } else if attr.path().is_ident("cfg_attr") {
                self.tracked = self.tracked.saturating_add(1);
                if let Some(expr) = self
                    .list_tokens(attr)
                    .and_then(|t| self.cfg_attr_predicate(t))
                {
                    predicates.push(Rc::new(expr));
                }
            } else if attr
                .path()
                .segments
                .last()
                .is_some_and(|s| s.ident == "test")
            {
                test = true;
            }
        }
        let base = self.conj(&cfgs);
        if guard && cfgs.is_empty() {
            self.record(base.clone(), true);
        }
        for _ in &cfgs {
            self.record(base.clone(), guard);
        }
        for predicate in predicates {
            let mut conj = base.clone();
            conj.push(predicate);
            self.record(conj, false);
        }
        let pushed = cfgs.len().saturating_add(usize::from(test));
        self.stack.extend(cfgs);
        if test {
            self.stack.push(Rc::clone(&self.test_expr));
        }
        pushed
    }

    fn leave(&mut self, pushed: usize) {
        self.stack.truncate(self.stack.len().saturating_sub(pushed));
    }

    fn gated(&mut self, attrs: &[Attribute], inner: impl FnOnce(&mut Self)) {
        let pushed = self.enter(attrs, false);
        inner(self);
        self.leave(pushed);
    }

    fn list_tokens(&mut self, attr: &Attribute) -> Option<TokenStream> {
        if let Meta::List(list) = &attr.meta {
            Some(list.tokens.clone())
        } else {
            self.refuse("a `cfg` / `cfg_attr` without a parenthesised list");
            None
        }
    }

    /// The predicate `text` parses to, when every name in it is modelled.
    fn parse_cfg(&mut self, text: &str) -> Option<Expression> {
        let expr = match Expression::parse(text) {
            Ok(expr) => expr,
            Err(e) => {
                self.refuse(&format!("`cfg({text})` does not parse: {e}"));
                return None;
            }
        };
        let problems: Vec<String> = expr
            .predicates()
            .filter_map(|predicate| self.predicate_problem(&predicate))
            .collect();
        if problems.is_empty() {
            return Some(expr);
        }
        for problem in problems {
            self.refuse(&problem);
        }
        None
    }

    fn predicate_problem(&self, predicate: &Predicate<'_>) -> Option<String> {
        match predicate {
            Predicate::Target(_) | Predicate::Test | Predicate::DebugAssertions => None,
            Predicate::Feature(name) => {
                (!self.graph.is_known(name)).then(|| format!("unknown feature `{name}` in a cfg"))
            }
            Predicate::Flag(name) => {
                (!FIXED_FLAGS.contains(name)).then(|| format!("unknown cfg name `{name}`"))
            }
            Predicate::ProcMacro | Predicate::TargetFeature(_) | Predicate::KeyValue { .. } => {
                Some(format!("unmodelled cfg predicate `{predicate:?}`"))
            }
        }
    }

    /// The predicate of a `cfg_attr(…)`; adding an attribute that changes what compiles is refused.
    fn cfg_attr_predicate(&mut self, tokens: TokenStream) -> Option<Expression> {
        let mut parts = split_commas(tokens).into_iter();
        let predicate = parts.next().unwrap_or_default();
        for part in parts {
            let names: Vec<String> = part
                .into_iter()
                .take_while(|tree| matches!(tree, TokenTree::Ident(_) | TokenTree::Punct(_)))
                .filter_map(|tree| match tree {
                    TokenTree::Ident(ident) => Some(ident.to_string()),
                    _ => None,
                })
                .collect();
            let changes = |name: Option<&String>| {
                name.is_some_and(|n| matches!(n.as_str(), "cfg" | "cfg_attr" | "path" | "test"))
            };
            if changes(names.first()) || changes(names.last()) {
                self.refuse(&format!(
                    "`cfg_attr` adds `{}`, which changes what compiles",
                    names.join("::")
                ));
                return None;
            }
        }
        self.parse_cfg(&predicate.to_string())
    }

    /// Follows an item-position `include!` as if its file's items were here.
    fn follow_include(&mut self, mac: &syn::Macro) {
        let Ok(literal) = mac.parse_body::<LitStr>() else {
            self.refuse("an `include!` whose argument is not a string literal");
            return;
        };
        let Some(key) = normalize(&self.ctx.file_dir, &literal.value()) else {
            self.refuse(&format!(
                "`include!(\"{}\")` leaves the crate",
                literal.value()
            ));
            return;
        };
        let (child_dir, inline) = (self.ctx.child_dir.clone(), self.ctx.inline);
        self.walk_file(&key, child_dir, inline);
    }

    /// Resolves `module` the way rustc does and walks its items.
    fn walk_mod(&mut self, module: &ItemMod) {
        let paths: Vec<&Attribute> = module
            .attrs
            .iter()
            .filter(|attr| attr.path().is_ident("path"))
            .collect();
        if paths.len() > 1 {
            self.refuse("a module with more than one `#[path]`");
            return;
        }
        let name = module.ident.unraw().to_string();
        if let Some((_, items)) = &module.content {
            if !paths.is_empty() {
                self.refuse("an inline module with `#[path]`");
                return;
            }
            let saved = self.ctx.clone();
            self.ctx.child_dir = join(&saved.child_dir, &name);
            self.ctx.inline = true;
            for item in items {
                self.visit_item(item);
            }
            self.ctx = saved;
            return;
        }
        if let Some(attr) = paths.first() {
            self.walk_path_mod(attr);
            return;
        }
        let base = join(&self.ctx.child_dir, &name);
        let flat = format!("{base}.rs");
        let nested = format!("{base}/mod.rs");
        match (
            self.sources.contains_key(&flat),
            self.sources.contains_key(&nested),
        ) {
            (true, false) => self.walk_file(&flat, base, false),
            (false, true) => self.walk_file(&nested, base, false),
            (true, true) => self.refuse(&format!(
                "`mod {name};` matches both `{flat}` and `{nested}`"
            )),
            (false, false) => self.refuse(&format!("`mod {name};` has no file")),
        }
    }

    /// Walks the file a non-inline `#[path = "…"]` module names, relative to the declaring file.
    fn walk_path_mod(&mut self, attr: &Attribute) {
        if self.ctx.inline {
            self.refuse("`#[path]` on a module inside an inline module");
            return;
        }
        let Meta::NameValue(value) = &attr.meta else {
            self.refuse("a `#[path]` without a value");
            return;
        };
        let Expr::Lit(syn::ExprLit {
            lit: Lit::Str(path),
            ..
        }) = &value.value
        else {
            self.refuse("a `#[path]` whose value is not a string");
            return;
        };
        let Some(key) = normalize(&self.ctx.file_dir, &path.value()) else {
            self.refuse(&format!(
                "`#[path = \"{}\"]` leaves the crate",
                path.value()
            ));
            return;
        };
        let child_dir = parent(&key);
        self.walk_file(&key, child_dir, false);
    }

    /// Records every `#[cfg]` / `#[cfg_attr]` inside macro tokens as a region.
    ///
    /// A `cfg` applies to the tokens after it up to the next `;` or `,`, or
    /// through the next brace group; nested groups inherit the `cfg`s open
    /// around them.
    fn scan_tokens(&mut self, tokens: TokenStream, outer: &[Rc<Expression>], depth: usize) {
        if depth > MAX_TOKEN_DEPTH {
            self.refuse(&format!("macro tokens nest past {MAX_TOKEN_DEPTH} groups"));
            return;
        }
        let mut pending: Vec<Rc<Expression>> = Vec::new();
        let mut trees = tokens.into_iter().peekable();
        while let Some(tree) = trees.next() {
            match tree {
                TokenTree::Punct(punct) if punct.as_char() == '#' => {
                    if matches!(trees.peek(), Some(TokenTree::Punct(bang)) if bang.as_char() == '!')
                    {
                        trees.next();
                    }
                    let Some(TokenTree::Group(group)) = trees.peek() else {
                        continue;
                    };
                    if group.delimiter() != Delimiter::Bracket {
                        continue;
                    }
                    let mut inner = group.stream().into_iter();
                    let kind = match inner.next() {
                        Some(TokenTree::Ident(ident)) if ident == "cfg" => TokenAttr::Cfg,
                        Some(TokenTree::Ident(ident)) if ident == "cfg_attr" => TokenAttr::CfgAttr,
                        _ => continue,
                    };
                    let args = inner.next();
                    trees.next();
                    let Some(TokenTree::Group(args)) = args else {
                        self.refuse("a macro-token `cfg` without a list");
                        continue;
                    };
                    self.token_attr(&kind, args.stream(), outer, &mut pending);
                }
                TokenTree::Punct(punct) if matches!(punct.as_char(), ';' | ',') => pending.clear(),
                TokenTree::Group(group) => {
                    let nested: Vec<Rc<Expression>> =
                        outer.iter().chain(&pending).cloned().collect();
                    self.scan_tokens(group.stream(), &nested, depth.saturating_add(1));
                    if group.delimiter() == Delimiter::Brace {
                        pending.clear();
                    }
                }
                TokenTree::Punct(_) | TokenTree::Ident(_) | TokenTree::Literal(_) => {}
            }
        }
    }

    fn token_attr(
        &mut self,
        kind: &TokenAttr,
        args: TokenStream,
        outer: &[Rc<Expression>],
        pending: &mut Vec<Rc<Expression>>,
    ) {
        match kind {
            TokenAttr::Cfg => {
                if let Some(expr) = self.parse_cfg(&args.to_string()) {
                    let expr = Rc::new(expr);
                    let conj = self.conj(outer.iter().chain(pending.iter()).chain([&expr]));
                    self.record(conj, false);
                    pending.push(expr);
                }
            }
            TokenAttr::CfgAttr => {
                if let Some(expr) = self.cfg_attr_predicate(args) {
                    let expr = Rc::new(expr);
                    let conj = self.conj(outer.iter().chain(pending.iter()).chain([&expr]));
                    self.record(conj, false);
                }
            }
        }
    }
}

/// The outer attributes of `item`.
fn item_attrs(item: &Item) -> &[Attribute] {
    match item {
        Item::Const(i) => &i.attrs,
        Item::Enum(i) => &i.attrs,
        Item::ExternCrate(i) => &i.attrs,
        Item::Fn(i) => &i.attrs,
        Item::ForeignMod(i) => &i.attrs,
        Item::Impl(i) => &i.attrs,
        Item::Macro(i) => &i.attrs,
        Item::Mod(i) => &i.attrs,
        Item::Static(i) => &i.attrs,
        Item::Struct(i) => &i.attrs,
        Item::Trait(i) => &i.attrs,
        Item::TraitAlias(i) => &i.attrs,
        Item::Type(i) => &i.attrs,
        Item::Union(i) => &i.attrs,
        Item::Use(i) => &i.attrs,
        _ => &[],
    }
}

/// The outer attributes of `expr`.
///
/// A kind missing here leaves its `cfg`s untracked, which fails the walk.
fn expr_attrs(expr: &Expr) -> &[Attribute] {
    match expr {
        Expr::Array(e) => &e.attrs,
        Expr::Assign(e) => &e.attrs,
        Expr::Async(e) => &e.attrs,
        Expr::Await(e) => &e.attrs,
        Expr::Binary(e) => &e.attrs,
        Expr::Block(e) => &e.attrs,
        Expr::Break(e) => &e.attrs,
        Expr::Call(e) => &e.attrs,
        Expr::Cast(e) => &e.attrs,
        Expr::Closure(e) => &e.attrs,
        Expr::Const(e) => &e.attrs,
        Expr::Continue(e) => &e.attrs,
        Expr::Field(e) => &e.attrs,
        Expr::ForLoop(e) => &e.attrs,
        Expr::Group(e) => &e.attrs,
        Expr::If(e) => &e.attrs,
        Expr::Index(e) => &e.attrs,
        Expr::Infer(e) => &e.attrs,
        Expr::Let(e) => &e.attrs,
        Expr::Lit(e) => &e.attrs,
        Expr::Loop(e) => &e.attrs,
        Expr::Macro(e) => &e.attrs,
        Expr::Match(e) => &e.attrs,
        Expr::MethodCall(e) => &e.attrs,
        Expr::Paren(e) => &e.attrs,
        Expr::Path(e) => &e.attrs,
        Expr::Range(e) => &e.attrs,
        Expr::RawAddr(e) => &e.attrs,
        Expr::Reference(e) => &e.attrs,
        Expr::Repeat(e) => &e.attrs,
        Expr::Return(e) => &e.attrs,
        Expr::Struct(e) => &e.attrs,
        Expr::Try(e) => &e.attrs,
        Expr::TryBlock(e) => &e.attrs,
        Expr::Tuple(e) => &e.attrs,
        Expr::Unary(e) => &e.attrs,
        Expr::Unsafe(e) => &e.attrs,
        Expr::While(e) => &e.attrs,
        Expr::Yield(e) => &e.attrs,
        _ => &[],
    }
}

/// The outer attributes of `pat`; `Pat::Type` is read through `visit_pat_type`.
fn pat_attrs(pat: &Pat) -> &[Attribute] {
    match pat {
        Pat::Const(p) => &p.attrs,
        Pat::Guard(p) => &p.attrs,
        Pat::Ident(p) => &p.attrs,
        Pat::Lit(p) => &p.attrs,
        Pat::Macro(p) => &p.attrs,
        Pat::Or(p) => &p.attrs,
        Pat::Paren(p) => &p.attrs,
        Pat::Path(p) => &p.attrs,
        Pat::Range(p) => &p.attrs,
        Pat::Reference(p) => &p.attrs,
        Pat::Rest(p) => &p.attrs,
        Pat::Slice(p) => &p.attrs,
        Pat::Struct(p) => &p.attrs,
        Pat::Tuple(p) => &p.attrs,
        Pat::TupleStruct(p) => &p.attrs,
        Pat::Wild(p) => &p.attrs,
        _ => &[],
    }
}

/// The outer attributes of `ty`.
fn type_attrs(ty: &Type) -> &[Attribute] {
    match ty {
        Type::Array(t) => &t.attrs,
        Type::FnPtr(t) => &t.attrs,
        Type::Group(t) => &t.attrs,
        Type::ImplTrait(t) => &t.attrs,
        Type::Infer(t) => &t.attrs,
        Type::Macro(t) => &t.attrs,
        Type::Never(t) => &t.attrs,
        Type::Paren(t) => &t.attrs,
        Type::Path(t) => &t.attrs,
        Type::Ptr(t) => &t.attrs,
        Type::Reference(t) => &t.attrs,
        Type::Slice(t) => &t.attrs,
        Type::TraitObject(t) => &t.attrs,
        Type::Tuple(t) => &t.attrs,
        _ => &[],
    }
}

impl<'ast> Visit<'ast> for Walk<'_> {
    fn visit_attribute(&mut self, attr: &'ast Attribute) {
        if is_cfg(attr) {
            self.seen = self.seen.saturating_add(1);
        }
    }

    fn visit_item(&mut self, item: &'ast Item) {
        match item {
            Item::Mod(module) => {
                let pushed = self.enter(&module.attrs, false);
                for attr in &module.attrs {
                    self.visit_attribute(attr);
                }
                self.walk_mod(module);
                self.leave(pushed);
            }
            Item::Macro(mac) if mac.mac.path.is_ident("include") => {
                let pushed = self.enter(&mac.attrs, false);
                for attr in &mac.attrs {
                    self.visit_attribute(attr);
                }
                self.follow_include(&mac.mac);
                self.leave(pushed);
            }
            Item::Macro(mac) if mac.mac.path.is_ident("compile_error") => {
                let pushed = self.enter(&mac.attrs, true);
                visit::visit_item(self, item);
                self.leave(pushed);
            }
            Item::Verbatim(_) => self.refuse("an item syn leaves unparsed"),
            _ => self.gated(item_attrs(item), |walk| visit::visit_item(walk, item)),
        }
    }

    fn visit_impl_item(&mut self, item: &'ast ImplItem) {
        let attrs: &[Attribute] = match item {
            ImplItem::Const(i) => &i.attrs,
            ImplItem::Fn(i) => &i.attrs,
            ImplItem::Type(i) => &i.attrs,
            ImplItem::Macro(i) => &i.attrs,
            _ => {
                self.refuse("an impl item syn leaves unparsed");
                return;
            }
        };
        self.gated(attrs, |walk| visit::visit_impl_item(walk, item));
    }

    fn visit_trait_item(&mut self, item: &'ast TraitItem) {
        let attrs: &[Attribute] = match item {
            TraitItem::Const(i) => &i.attrs,
            TraitItem::Fn(i) => &i.attrs,
            TraitItem::Type(i) => &i.attrs,
            TraitItem::Macro(i) => &i.attrs,
            _ => {
                self.refuse("a trait item syn leaves unparsed");
                return;
            }
        };
        self.gated(attrs, |walk| visit::visit_trait_item(walk, item));
    }

    fn visit_foreign_item(&mut self, item: &'ast ForeignItem) {
        let attrs: &[Attribute] = match item {
            ForeignItem::Fn(i) => &i.attrs,
            ForeignItem::Static(i) => &i.attrs,
            ForeignItem::Type(i) => &i.attrs,
            ForeignItem::Macro(i) => &i.attrs,
            _ => {
                self.refuse("a foreign item syn leaves unparsed");
                return;
            }
        };
        self.gated(attrs, |walk| visit::visit_foreign_item(walk, item));
    }

    fn visit_expr(&mut self, expr: &'ast Expr) {
        if matches!(expr, Expr::Verbatim(_)) {
            self.refuse("an expression syn leaves unparsed");
            return;
        }
        self.gated(expr_attrs(expr), |walk| visit::visit_expr(walk, expr));
    }

    fn visit_pat(&mut self, pat: &'ast Pat) {
        if matches!(pat, Pat::Verbatim(_)) {
            self.refuse("a pattern syn leaves unparsed");
            return;
        }
        self.gated(pat_attrs(pat), |walk| visit::visit_pat(walk, pat));
    }

    fn visit_pat_type(&mut self, pat: &'ast PatType) {
        self.gated(&pat.attrs, |walk| visit::visit_pat_type(walk, pat));
    }

    fn visit_type(&mut self, ty: &'ast Type) {
        if matches!(ty, Type::Verbatim(_)) {
            self.refuse("a type syn leaves unparsed");
            return;
        }
        self.gated(type_attrs(ty), |walk| visit::visit_type(walk, ty));
    }

    fn visit_where_predicate(&mut self, predicate: &'ast WherePredicate) {
        let attrs: &[Attribute] = match predicate {
            WherePredicate::Lifetime(p) => &p.attrs,
            WherePredicate::Type(p) => &p.attrs,
            _ => &[],
        };
        self.gated(attrs, |walk| visit::visit_where_predicate(walk, predicate));
    }

    fn visit_generic_param(&mut self, param: &'ast GenericParam) {
        let attrs: &[Attribute] = match param {
            GenericParam::Lifetime(p) => &p.attrs,
            GenericParam::Type(p) => &p.attrs,
            GenericParam::Const(p) => &p.attrs,
        };
        self.gated(attrs, |walk| visit::visit_generic_param(walk, param));
    }

    fn visit_local(&mut self, local: &'ast Local) {
        self.gated(&local.attrs, |walk| visit::visit_local(walk, local));
    }

    fn visit_stmt_macro(&mut self, mac: &'ast StmtMacro) {
        self.gated(&mac.attrs, |walk| visit::visit_stmt_macro(walk, mac));
    }

    fn visit_field(&mut self, field: &'ast Field) {
        self.gated(&field.attrs, |walk| visit::visit_field(walk, field));
    }

    fn visit_variant(&mut self, variant: &'ast Variant) {
        self.gated(&variant.attrs, |walk| visit::visit_variant(walk, variant));
    }

    fn visit_arm(&mut self, arm: &'ast Arm) {
        self.gated(&arm.attrs, |walk| visit::visit_arm(walk, arm));
    }

    fn visit_field_value(&mut self, value: &'ast FieldValue) {
        self.gated(&value.attrs, |walk| visit::visit_field_value(walk, value));
    }

    fn visit_field_pat(&mut self, pat: &'ast FieldPat) {
        self.gated(&pat.attrs, |walk| visit::visit_field_pat(walk, pat));
    }

    fn visit_receiver(&mut self, receiver: &'ast Receiver) {
        self.gated(&receiver.attrs, |walk| {
            visit::visit_receiver(walk, receiver)
        });
    }

    fn visit_variadic(&mut self, variadic: &'ast Variadic) {
        self.gated(&variadic.attrs, |walk| {
            visit::visit_variadic(walk, variadic)
        });
    }

    fn visit_named_arg(&mut self, arg: &'ast NamedArg) {
        self.gated(&arg.attrs, |walk| visit::visit_named_arg(walk, arg));
    }

    fn visit_fn_ptr_variadic(&mut self, variadic: &'ast FnPtrVariadic) {
        self.gated(&variadic.attrs, |walk| {
            visit::visit_fn_ptr_variadic(walk, variadic);
        });
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        if mac.path.is_ident("include") {
            self.refuse("an `include!` outside item position");
            return;
        }
        self.scan_tokens(mac.tokens.clone(), &[], 0);
    }
}

/// Every integration-test root among `sources`, with its test name.
fn test_roots(sources: &BTreeMap<String, String>) -> Vec<(String, String)> {
    sources
        .keys()
        .filter_map(|key| {
            let rest = key.strip_prefix("tests/")?;
            let name = match rest.split_once('/') {
                None => rest.strip_suffix(".rs")?,
                Some((dir, "main.rs")) => dir,
                Some(_) => return None,
            };
            Some((key.clone(), name.to_owned()))
        })
        .collect()
}

/// `rows` as a table; a repeated candidate or file is refused.
fn residual_table(rows: ResidualRows<'_>) -> Result<Residual, String> {
    let mut table = Residual::new();
    for (candidate, files) in rows {
        let mut counts = BTreeMap::new();
        for (file, count) in *files {
            if counts.insert((*file).to_owned(), *count).is_some() {
                return Err(format!("RESIDUAL repeats `{file}` under `{candidate}`"));
            }
        }
        if table.insert((*candidate).to_owned(), counts).is_some() {
            return Err(format!("RESIDUAL repeats candidate `{candidate}`"));
        }
    }
    Ok(table)
}

/// `table` as the Rust literal `RESIDUAL` is written in.
fn render_residual(table: &Residual) -> String {
    let rows: String = table
        .iter()
        .map(|(candidate, files)| {
            let files: String = files
                .iter()
                .map(|(file, count)| format!("            (\"{file}\", {count}),\n"))
                .collect();
            format!("    (\n        \"{candidate}\",\n        &[\n{files}        ],\n    ),\n")
        })
        .collect();
    format!("const RESIDUAL: ResidualRows<'static> = &[\n{rows}];\n")
}

/// The conjunction of `conj`, for a message.
fn render_conj(conj: &[Rc<Expression>]) -> String {
    if conj.is_empty() {
        return "<always>".to_owned();
    }
    conj.iter()
        .map(|expr| expr.original().to_owned())
        .collect::<Vec<_>>()
        .join(" && ")
}

/// Charges every region no CI leg compiles to the first candidate that does.
///
/// A candidate identical to a CI leg, a `compile_error!` guard a CI leg
/// satisfies, and a region no leg or candidate compiles are refused.
fn charge(
    nodes: &[Node],
    legs: &[NamedLeg],
    candidates: &[NamedLeg],
    host: &TargetInfo,
) -> Result<Residual, String> {
    for (name, candidate) in candidates {
        if let Some((leg, _)) = legs.iter().find(|(_, leg)| leg == candidate) {
            return Err(format!("candidate `{name}` equals the CI leg `{leg}`"));
        }
    }
    let mut errors = BTreeSet::new();
    let mut table = Residual::new();
    for node in nodes {
        let covered = legs
            .iter()
            .any(|(_, leg)| covers(leg, &node.unit, &node.conj, host));
        if node.guard {
            if covered {
                errors.insert(format!(
                    "{}: a `compile_error!` guard holds under a CI clippy leg: {}",
                    node.file,
                    render_conj(&node.conj)
                ));
            }
            continue;
        }
        if covered {
            continue;
        }
        let charged = candidates
            .iter()
            .find(|(_, leg)| covers(leg, &node.unit, &node.conj, host));
        if let Some((name, _)) = charged {
            let count = table
                .entry(name.clone())
                .or_default()
                .entry(node.file.clone())
                .or_default();
            *count = count.saturating_add(1);
        } else {
            errors.insert(format!(
                "{} ({:?}): a cfg region no clippy leg or candidate compiles: {}",
                node.file,
                node.unit,
                render_conj(&node.conj)
            ));
        }
    }
    if errors.is_empty() {
        Ok(table)
    } else {
        Err(errors.into_iter().collect::<Vec<_>>().join("\n"))
    }
}

/// Checks every `cfg` region of `sources` against `legs`, `candidates`, and `residual`.
fn coverage(
    sources: &BTreeMap<String, String>,
    package: &Package,
    legs: &[NamedLeg],
    candidates: &[Candidate],
    residual: ResidualRows<'_>,
) -> Result<(), String> {
    let expected = residual_table(residual)?;
    let candidate_legs = candidates
        .iter()
        .map(|candidate| {
            let mut leg =
                parse_clippy_command(candidate.command, &package.graph, &BTreeSet::new())?;
            leg.flags = candidate.flags.iter().map(|f| (*f).to_owned()).collect();
            Ok((candidate.name.to_owned(), leg))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let test_expr = Rc::new(Expression::parse("test").map_err(|e| e.to_string())?);
    let mut walk = Walk::new(sources, &package.graph, test_expr);
    walk.run(Unit::Lib, &package.lib_root, parent(&package.lib_root));
    for (key, name) in test_roots(sources) {
        walk.run(Unit::Test(name), &key, parent(&key));
    }
    if sources.contains_key("build.rs") {
        walk.run(Unit::Build, "build.rs", String::new());
    }
    let orphans: Vec<&String> = sources
        .keys()
        .filter(|key| !key.starts_with("tests/fixtures/") && !walk.reached.contains(*key))
        .collect();
    for orphan in orphans {
        walk.refusals
            .insert(format!("{orphan}: a source file reached by no module"));
    }
    if !walk.refusals.is_empty() {
        return Err(walk.refusals.into_iter().collect::<Vec<_>>().join("\n"));
    }
    let host = target_info(HOST_TRIPLE)?;
    let computed = charge(&walk.nodes, legs, &candidate_legs, &host)?;
    if computed == expected {
        Ok(())
    } else {
        Err(format!(
            "RESIDUAL does not match the computed uncovered regions; the computed table:\n{}",
            render_residual(&computed)
        ))
    }
}

#[test]
fn every_cfg_region_is_linted_by_a_ci_clippy_leg_or_pinned() {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo = crate_dir.join("../../..");
    let read = |path: &Path| {
        std::fs::read_to_string(path)
            .unwrap_or_else(|e| format!("<unreadable {}: {e}>", path.display()))
    };
    let package = Package::from_manifest(&read(&crate_dir.join("Cargo.toml"))).unwrap();
    let host = target_info(HOST_TRIPLE).unwrap();
    let workspace = workspace_seeds(&read(&repo.join("Cargo.toml")), &host, |member| {
        std::fs::read_to_string(repo.join(member).join("Cargo.toml"))
            .map_err(|e| format!("{member}/Cargo.toml: {e}"))
    })
    .unwrap();
    let legs = ci_legs(
        &read(&repo.join(".github/workflows/ci.yml")),
        &package.graph,
        &workspace,
    )
    .unwrap();
    let mut sources = BTreeMap::new();
    for dir in ["src", "tests"] {
        for (rel, text) in source_tree::rust_sources(&crate_dir.join(dir)) {
            sources.insert(format!("{dir}/{rel}"), text);
        }
    }
    sources.insert("build.rs".to_owned(), read(&crate_dir.join("build.rs")));
    let report = coverage(&sources, &package, &legs, CANDIDATES, RESIDUAL);
    assert!(report.is_ok(), "{}", report.err().unwrap_or_default());
}

/// The manifest of the synthetic package the refusal tests check.
const SYNTH_MANIFEST: &str = r#"
[package]
name = "ipe-runtime-rust"

[lib]
path = "src/mod.rs"

[features]
default = []
a = []
b = []
f = []
"#;

/// A synthetic `ci.yml` whose two clippy jobs run `clippy` and `combos`.
fn synth_ci(clippy: &str, combos: &str) -> String {
    format!(
        "jobs:\n  clippy:\n    runs-on: ubuntu-latest\n    steps:\n      - run: {clippy}\n  \
         runtime-feature-combos-run:\n    runs-on: ubuntu-latest\n    steps:\n      - run: {combos}\n"
    )
}

/// The clippy command for the synthetic package with `extra` flags.
fn leg(extra: &str) -> String {
    format!("cargo clippy -p ipe-runtime-rust {extra} --all-targets -- -D warnings")
}

/// Runs the coverage check over a synthetic package.
fn synth(
    files: &[(&str, &str)],
    ci: &str,
    candidates: &[Candidate],
    residual: ResidualRows<'_>,
) -> Result<(), String> {
    let package = Package::from_manifest(SYNTH_MANIFEST)?;
    let legs = ci_legs(ci, &package.graph, &BTreeSet::from(["default".to_owned()]))?;
    let sources = files
        .iter()
        .map(|(key, text)| ((*key).to_owned(), (*text).to_owned()))
        .collect();
    coverage(&sources, &package, &legs, candidates, residual)
}

/// The refusal message of a synthetic run, empty when it passed.
fn refusal(
    files: &[(&str, &str)],
    ci: &str,
    candidates: &[Candidate],
    residual: ResidualRows<'_>,
) -> String {
    synth(files, ci, candidates, residual)
        .err()
        .unwrap_or_default()
}

#[test]
fn a_region_a_leg_compiles_passes() {
    let ci = synth_ci(&leg("--features a"), &leg("--features f"));
    let files = [("src/mod.rs", "#[cfg(feature = \"a\")] fn x() {}")];
    let result = synth(&files, &ci, &[], &[]);
    assert!(result.is_ok(), "{result:?}");
}

#[test]
fn a_region_no_leg_enables_fails() {
    let ci = synth_ci(&leg("--features a"), &leg("--features f"));
    let files = [("src/mod.rs", "#[cfg(feature = \"b\")] fn x() {}")];
    let err = refusal(&files, &ci, &[], &[]);
    assert!(err.contains("no clippy leg or candidate compiles"), "{err}");
}

#[test]
fn a_negated_feature_every_leg_enables_fails() {
    let ci = synth_ci(&leg("--features a,f"), &leg("--features f"));
    let files = [("src/mod.rs", "#[cfg(not(feature = \"f\"))] fn x() {}")];
    let err = refusal(&files, &ci, &[], &[]);
    assert!(err.contains("no clippy leg or candidate compiles"), "{err}");
}

#[test]
fn an_unmodelled_clippy_flag_fails() {
    let ci = synth_ci(&leg("--all-features"), &leg("--features f"));
    let err = refusal(&[("src/mod.rs", "")], &ci, &[], &[]);
    assert!(
        err.contains("unmodelled clippy flag `--all-features`"),
        "{err}"
    );
}

#[test]
fn an_unknown_cfg_name_fails() {
    let ci = synth_ci(&leg("--features a"), &leg("--features f"));
    let files = [("src/mod.rs", "#[cfg(sneaky)] fn x() {}")];
    let err = refusal(&files, &ci, &[], &[]);
    assert!(err.contains("unknown cfg name `sneaky`"), "{err}");
}

#[test]
fn an_unknown_cfg_feature_fails() {
    let ci = synth_ci(&leg("--features a"), &leg("--features f"));
    let files = [("src/mod.rs", "#[cfg(feature = \"nope\")] fn x() {}")];
    let err = refusal(&files, &ci, &[], &[]);
    assert!(err.contains("unknown feature `nope` in a cfg"), "{err}");
}

#[test]
fn an_unknown_leg_feature_fails() {
    let ci = synth_ci(&leg("--features nope"), &leg("--features f"));
    let err = refusal(&[("src/mod.rs", "")], &ci, &[], &[]);
    assert!(err.contains("unknown feature `nope`"), "{err}");
}

#[test]
fn a_stale_residual_row_fails() {
    let ci = synth_ci(&leg("--features a"), &leg("--features f"));
    let files = [("src/mod.rs", "#[cfg(feature = \"a\")] fn x() {}")];
    let stale: ResidualRows<'_> = &[("b-leg", &[("src/mod.rs", 1)])];
    let err = refusal(&files, &ci, &[], stale);
    assert!(err.contains("RESIDUAL does not match"), "{err}");
}

#[test]
fn a_charged_region_with_its_exact_residual_passes() {
    let ci = synth_ci(&leg("--features a"), &leg("--features f"));
    let files = [("src/mod.rs", "#[cfg(feature = \"b\")] fn x() {}")];
    let candidates = [Candidate {
        name: "b-leg",
        command: "cargo clippy -p ipe-runtime-rust --features b --all-targets -- -D warnings",
        flags: &[],
    }];
    let exact: ResidualRows<'_> = &[("b-leg", &[("src/mod.rs", 1)])];
    let result = synth(&files, &ci, &candidates, exact);
    assert!(result.is_ok(), "{result:?}");
    let err = refusal(&files, &ci, &candidates, &[]);
    assert!(err.contains("(\"src/mod.rs\", 1)"), "{err}");
}

#[test]
fn a_candidate_equal_to_a_ci_leg_fails() {
    let ci = synth_ci(&leg("--features a"), &leg("--features f"));
    let candidates = [Candidate {
        name: "dup",
        command: "cargo clippy -p ipe-runtime-rust --features a --all-targets -- -D warnings",
        flags: &[],
    }];
    let err = refusal(&[("src/mod.rs", "")], &ci, &candidates, &[]);
    assert!(err.contains("candidate `dup` equals the CI leg"), "{err}");
}

#[test]
fn an_orphan_source_fails() {
    let ci = synth_ci(&leg("--features a"), &leg("--features f"));
    let files = [("src/mod.rs", ""), ("src/stray.rs", "fn x() {}")];
    let err = refusal(&files, &ci, &[], &[]);
    assert!(
        err.contains("src/stray.rs: a source file reached by no module"),
        "{err}"
    );
}

#[test]
fn a_compile_error_guard_a_leg_satisfies_fails() {
    let ci = synth_ci(&leg("--features a"), &leg("--features f"));
    let files = [(
        "src/mod.rs",
        "#[cfg(feature = \"a\")] compile_error!(\"no\");",
    )];
    let err = refusal(&files, &ci, &[], &[]);
    assert!(err.contains("a `compile_error!` guard holds"), "{err}");
}

#[test]
fn a_compile_error_guard_no_leg_satisfies_passes() {
    let ci = synth_ci(&leg("--features a"), &leg("--features f"));
    let guard = "#[cfg(all(feature = \"a\", feature = \"f\"))] compile_error!(\"no\");";
    let result = synth(&[("src/mod.rs", guard)], &ci, &[], &[]);
    assert!(result.is_ok(), "{result:?}");
}

#[test]
fn a_runtime_clippy_line_outside_the_job_set_fails() {
    let ci = format!(
        "{}  other:\n    runs-on: ubuntu-latest\n    steps:\n      - run: {}\n",
        synth_ci(&leg("--features a"), &leg("--features f")),
        leg("--features b")
    );
    let err = refusal(&[("src/mod.rs", "")], &ci, &[], &[]);
    assert!(err.contains("outside the clippy job set"), "{err}");
}

#[test]
fn a_nested_region_needs_its_whole_ancestor_conjunction() {
    let ci = synth_ci(&leg("--features a"), &leg("--features b"));
    let files = [(
        "src/mod.rs",
        "#[cfg(feature = \"a\")] mod m { #[cfg(feature = \"b\")] fn g() {} }",
    )];
    let err = refusal(&files, &ci, &[], &[]);
    assert_eq!(
        err.matches("no clippy leg or candidate compiles").count(),
        1,
        "{err}"
    );
    assert!(err.contains("feature = \"a\" && feature = \"b\""), "{err}");
}

#[test]
fn a_region_inside_macro_tokens_is_checked() {
    let ci = synth_ci(&leg("--features a"), &leg("--features f"));
    let files = [(
        "src/mod.rs",
        "thread_local! { #[cfg(feature = \"b\")] static X: u8 = 0; }",
    )];
    let err = refusal(&files, &ci, &[], &[]);
    assert!(err.contains("no clippy leg or candidate compiles"), "{err}");
}

#[test]
fn a_cfg_attr_adding_a_path_fails() {
    let ci = synth_ci(&leg("--features a"), &leg("--features f"));
    let files = [
        (
            "src/mod.rs",
            "#[cfg_attr(feature = \"a\", path = \"x.rs\")] mod y;",
        ),
        ("src/y.rs", ""),
    ];
    let err = refusal(&files, &ci, &[], &[]);
    assert!(err.contains("`cfg_attr` adds `path`"), "{err}");
}

#[test]
fn an_ambiguous_module_file_fails() {
    let ci = synth_ci(&leg("--features a"), &leg("--features f"));
    let files = [
        ("src/mod.rs", "mod y;"),
        ("src/y.rs", ""),
        ("src/y/mod.rs", ""),
    ];
    let err = refusal(&files, &ci, &[], &[]);
    assert!(err.contains("matches both"), "{err}");
}

#[test]
fn a_path_module_above_the_crate_root_fails() {
    let ci = synth_ci(&leg("--features a"), &leg("--features f"));
    let files = [("src/mod.rs", "#[path = \"../../x.rs\"] mod y;")];
    let err = refusal(&files, &ci, &[], &[]);
    assert!(err.contains("leaves the crate"), "{err}");
}

#[test]
fn modules_include_and_test_roots_are_followed() {
    let ci = synth_ci(&leg("--features a"), &leg("--features f"));
    let files = [
        ("src/mod.rs", "mod inner;"),
        ("src/inner.rs", "mod deep { include!(\"data.rs\"); }"),
        ("src/data.rs", "#[cfg(feature = \"b\")] fn d() {}"),
        ("tests/t.rs", "#[path = \"support/s.rs\"] mod s;"),
        ("tests/support/s.rs", "#[cfg(not(test))] fn never() {}"),
    ];
    let err = refusal(&files, &ci, &[], &[]);
    assert!(err.contains("src/data.rs (Lib)"), "{err}");
    assert!(err.contains("tests/support/s.rs (Test(\"t\"))"), "{err}");
    assert_eq!(
        err.matches("no clippy leg or candidate compiles").count(),
        2,
        "{err}"
    );
}

#[test]
fn a_clippy_job_with_no_leg_fails() {
    let ci = synth_ci(&leg("--features a"), "echo none");
    let err = refusal(&[("src/mod.rs", "")], &ci, &[], &[]);
    assert!(
        err.contains("job `runtime-feature-combos-run` runs no clippy leg"),
        "{err}"
    );
}

#[test]
fn a_clippy_job_off_the_modelled_host_fails() {
    let ci = synth_ci(&leg("--features a"), &leg("--features f")).replacen(
        "ubuntu-latest",
        "macos-latest",
        1,
    );
    let err = refusal(&[("src/mod.rs", "")], &ci, &[], &[]);
    assert!(err.contains("not the modelled host"), "{err}");
}

#[test]
fn workspace_seeds_union_every_member_request() {
    let root = "[workspace]\nmembers = [\"rt\", \"one\", \"two\"]\n";
    let host = target_info(HOST_TRIPLE).unwrap();
    let member = |name: &str| -> Result<String, String> {
        Ok(match name {
            "rt" => "[package]\nname = \"ipe-runtime-rust\"\n".to_owned(),
            "one" => "[package]\nname = \"one\"\n[dependencies]\n\
                      ipe-runtime-rust = { path = \"../rt\", default-features = false, features = [\"a\"] }\n"
                .to_owned(),
            _ => "[package]\nname = \"two\"\n[target.'cfg(windows)'.dependencies]\n\
                  ipe-runtime-rust = { path = \"../rt\", features = [\"b\"] }\n"
                .to_owned(),
        })
    };
    let seeds = workspace_seeds(root, &host, member).unwrap();
    assert_eq!(
        seeds,
        BTreeSet::from(["default".to_owned(), "a".to_owned()])
    );
    let optional = |name: &str| -> Result<String, String> {
        Ok(if name == "rt" {
            "[package]\nname = \"ipe-runtime-rust\"\n".to_owned()
        } else {
            "[package]\nname = \"x\"\n[dependencies]\n\
             ipe-runtime-rust = { path = \"../rt\", optional = true }\n"
                .to_owned()
        })
    };
    let err = workspace_seeds(root, &host, optional)
        .err()
        .unwrap_or_default();
    assert!(err.contains("are not modelled"), "{err}");
}

#[test]
fn the_feature_closure_follows_the_cargo_rules() {
    let manifest = "[package]\nname = \"p\"\n[dependencies]\n\
                    imp = { version = \"1\", optional = true }\n\
                    exp = { version = \"1\", optional = true }\n\
                    [features]\nx = [\"dep:exp\", \"imp/f\", \"exp?/g\"]\ny = [\"x\"]\n";
    let package = Package::from_manifest(manifest).unwrap();
    let enabled = package
        .graph
        .closure(&BTreeSet::from(["y".to_owned()]))
        .unwrap();
    let expected: BTreeSet<String> = ["imp", "x", "y"].map(str::to_owned).into();
    assert_eq!(enabled, expected);
    assert!(!package.graph.is_known("exp"));
}

#[test]
fn an_include_outside_item_position_fails() {
    let ci = synth_ci(&leg("--features a"), &leg("--features f"));
    let files = [
        ("src/mod.rs", "fn x() -> u8 { include!(\"v.rs\") }"),
        ("src/v.rs", "0"),
    ];
    let err = refusal(&files, &ci, &[], &[]);
    assert!(err.contains("an `include!` outside item position"), "{err}");
}

#[test]
fn an_inline_module_with_a_path_fails() {
    let ci = synth_ci(&leg("--features a"), &leg("--features f"));
    let files = [("src/mod.rs", "#[path = \"x\"] mod y { }")];
    let err = refusal(&files, &ci, &[], &[]);
    assert!(err.contains("an inline module with `#[path]`"), "{err}");
}
