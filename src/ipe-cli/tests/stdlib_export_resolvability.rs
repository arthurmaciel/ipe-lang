#![forbid(unsafe_code)]
//! Anti-drift gate: every EXPORT of every compiled-source stdlib module resolves
//! through the real compiler pipeline.
//!
//! The source-vs-kernel drift class: a compiled-source `Ipe.*` module declares a
//! member in its `exposing (...)` list, but the member has no resolvable home —
//! no local body, no re-export, and (for an `Ffi.kernel "…"` alias) no matching
//! registered kernel. Such a member type-checks nowhere: a `Module.member` call
//! fails name-resolution (IPE-N0005 / IPE-N0028). An earlier `Ipe.Random`
//! shipped exactly this — `shuffle`/`weighted`/the seeded helpers were declared
//! but had no kernel row.
//!
//! This gate canonicalises EVERY compiled-source module (types included, unlike
//! the parse-only `ipe_stdlib::every_exported_value_has_a_home` floor) by
//! importing it into one `Main` and driving the production compile pipeline. A
//! module whose export is a dangling declaration or a broken kernel alias fails
//! its own canonicalisation here — pre-cargo, in the fast (non-E2E) path.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use ipe::project;
use ipe_db::Db as _;
use ipe_intern::Interner;
use ipe_types::{RowTail, Ty, VarNamer};

type UserSources = BTreeMap<Vec<String>, String>;
type PreparedSources = BTreeMap<Vec<String>, (PathBuf, String)>;

fn prepared(user: &UserSources) -> (PreparedSources, BTreeSet<Vec<String>>) {
    let mut sources: PreparedSources = user
        .iter()
        .map(|(p, text)| {
            (
                p.clone(),
                (
                    PathBuf::from(format!("<resolvability>/{}.ipe", p.join("/"))),
                    text.clone(),
                ),
            )
        })
        .collect();
    let mut discovered: Vec<project::DiscoveredModule> = sources
        .iter()
        .map(|(p, (path, _))| project::DiscoveredModule::user(path.clone(), p.clone()))
        .collect();
    let injected = project::inject_compiled_std_closure(&mut sources, &mut discovered);
    (sources, injected)
}

fn entry_path() -> Vec<String> {
    vec!["Main".to_owned()]
}

/// The native, non-E2E build configuration every probe compiles under.
fn native_config(db: &ipe_db::IpeDatabase) -> ipe_db::BuildConfig {
    ipe_db::BuildConfig::new(
        db,
        ipe_backend_rust::DbDriver::Sqlite,
        None,
        ipe_ir::Target::Native,
        Vec::new(),
        false,
        false,
        None,
        false,
        String::new(),
        false,
        false,
    )
}

/// Compile one synthesized `Main` through the production pipeline; `Ok` iff the
/// whole closure — every injected compiled-source module — canonicalises,
/// type-checks, and lowers.
fn compile_main(main: &str) -> Result<(), String> {
    let mut user = UserSources::new();
    user.insert(entry_path(), main.to_owned());
    let (sources, injected) = prepared(&user);
    let db = ipe_db::IpeDatabase::new();
    let root = ipe::create_source_root(&db, &sources, &injected, &BTreeSet::new());
    let config = native_config(&db);
    ipe::compile_prepared(
        &db,
        root,
        &sources,
        &entry_path(),
        Path::new("<resolvability>"),
        config,
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// Like [`compile_main`], but adds extra user modules to the graph. Used to route
/// a `Ipe.Tea.*` shape module through a `main`-less helper (exempt from the
/// IPE-N0033 Program-imports-a-shape gate) while `Main` stays a plain Program.
fn compile_main_with_helper(main: &str, extras: &[(Vec<String>, String)]) -> Result<(), String> {
    let mut user = UserSources::new();
    user.insert(entry_path(), main.to_owned());
    for (path, text) in extras {
        user.insert(path.clone(), text.clone());
    }
    let (sources, injected) = prepared(&user);
    let db = ipe_db::IpeDatabase::new();
    let root = ipe::create_source_root(&db, &sources, &injected, &BTreeSet::new());
    let config = native_config(&db);
    ipe::compile_prepared(
        &db,
        root,
        &sources,
        &entry_path(),
        Path::new("<resolvability>"),
        config,
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// Every compiled-source stdlib module, imported ONE AT A TIME into a `Main`,
/// must resolve.
///
/// Each module is compiled in isolation (a `Main` importing only it) so the gate
/// tests one module's own export resolution — not the incidental cross-module
/// name interactions of importing the whole stdlib into a single graph. A broken
/// export (a dangling declaration or an `Ffi.kernel "…"` alias with no registered
/// kernel) fails the imported module's own canonicalisation, so the compile fails
/// and the culprit is named. Importing with `as` (no member use) is enough: the
/// injected module is canonicalised as a dependency, and a homeless export cannot
/// survive that pass.
#[test]
fn compiled_source_modules_resolve_all_exports() {
    let mut failures: Vec<String> = Vec::new();
    for m in ipe_stdlib::COMPILED_STD_MODULES {
        // A `Ipe.Tea.*` shape module marks any plain-`main` importer a TEA-app
        // contradiction (IPE-N0033). Route it through a `main`-less helper module
        // — exempt from that gate — so the export resolution still runs while
        // `Main` stays a plain Program.
        let is_tea_shape = m
            .dotted
            .strip_prefix("Ipe.Tea.")
            .is_some_and(|rest| rest.contains('.'));
        let result = if is_tea_shape {
            let helper = format!(
                "module Probe exposing (probe)\nimport {} as M\n\nprobe : Int\nprobe =\n    0\n",
                m.dotted,
            );
            compile_main_with_helper(
                "module Main exposing (main)\nimport Ipe.Io as Io\nimport Probe\n\n\
                 main : Task Error ()\nmain =\n    Io.println \"ok\"\n",
                &[(vec!["Probe".to_owned()], helper)],
            )
        } else {
            let main = format!(
                "module Main exposing (main)\nimport Ipe.Io as Io\nimport {} as M\n\n\
                 main : Task Error ()\nmain =\n    Io.println \"ok\"\n",
                m.dotted,
            );
            compile_main(&main)
        };
        if let Err(e) = result {
            failures.push(format!("{}: {e}", m.dotted));
        }
    }
    assert!(
        failures.is_empty(),
        "every compiled-source stdlib module must resolve all its exports \
         through the real pipeline — a failure here is source-vs-kernel drift \
         (a declared-but-homeless export or a broken `Ffi.kernel` alias):\n{}",
        failures.join("\n"),
    );
}

/// Every EXPOSED VALUE of every kernel-veneer `MODULES` fixture resolves through
/// the real pipeline against the qualifier catalog.
///
/// The veneers (`Ipe.System`, `Ipe.Http`, `Ipe.Process`, …) are NEVER injected as
/// source — `inject_compiled_std_closure` consults `COMPILED_STD_MODULES` alone,
/// so their `Kernel.kernel "…"` bodies never compile. A `Module.member` call
/// instead resolves against the pre-installed kernel-qualifier catalog. A member
/// listed in the veneer's `exposing (...)` but absent from that catalog (and from
/// the kernel registry) type-checks nowhere: the call is IPE-N0005, yet `ipe doc`
/// still advertises it (docs derive from the fixtures). This gate references
/// every exposed value at a real call site so a catalog-homeless veneer export
/// fails the compile here, pre-cargo — closing the same drift class the
/// compiled-source gate closes for `COMPILED_STD_MODULES`.
#[test]
fn kernel_veneer_modules_resolve_all_exports() {
    use ipe_syntax::{Exposed, Exposing};

    let mut failures: Vec<String> = Vec::new();
    for m in ipe_stdlib::MODULES {
        let mut interner = Interner::new();
        let parsed = match ipe_parse::parse_module(m.source, &mut interner) {
            Ok(p) => p,
            Err(e) => {
                failures.push(format!("{}: veneer failed to parse: {e:?}", m.name));
                continue;
            }
        };
        let value_names: Vec<String> = match &parsed.exposing.value {
            // An export-all veneer has nothing to cross-check name-by-name.
            Exposing::All => continue,
            Exposing::List(items) => items
                .iter()
                .filter_map(|item| match &item.value {
                    Exposed::Value(n) => interner.resolve(*n).map(str::to_owned),
                    Exposed::Type(_, _) => None,
                })
                .collect(),
        };
        if value_names.is_empty() {
            continue;
        }

        // Reference every exposed value at a real call site: one bare top-level
        // binding per value forces name resolution (and HM inference of its
        // scheme) independently, so a catalog-homeless member fails resolution
        // and names itself. No application is needed — a dangling `Module.member`
        // reference already resolves against the catalog.
        let mut probes = String::new();
        for (i, name) in value_names.iter().enumerate() {
            let _ = writeln!(probes, "probe{i} = M.{name}");
        }
        let main = format!(
            "module Main exposing (main)\nimport Ipe.Io as Io\nimport {} as M\n\n\
             {probes}\nmain : Task Error ()\nmain =\n    Io.println \"ok\"\n",
            m.name,
        );

        if let Err(e) = compile_main(&main) {
            failures.push(format!("{}: {e}", m.name));
        }
    }
    assert!(
        failures.is_empty(),
        "every kernel-veneer stdlib module must resolve all its exposed values \
         through the real pipeline — a failure here is a veneer that advertises \
         (and documents) a member with no catalog home (IPE-N0005 at any call \
         site):\n{}",
        failures.join("\n"),
    );
}

/// Focused regression: the whole `Ipe.Random` surface resolves + type-checks at
/// real call sites — the exact members that were homeless (`shuffle`,
/// `weighted`, `choice`, and the seeded `seed`/`seededInt`/`seededFloat`/
/// `seededChoice` over the opaque `Seed`).
#[test]
fn random_full_surface_resolves() {
    let main = concat!(
        "module Main exposing (main)\n",
        "import Ipe.Io as Io\n",
        "import Ipe.String as String\n",
        "import Ipe.Task as Task\n",
        "import Ipe.Random as Random\n\n",
        "seededLine : String\n",
        "seededLine =\n",
        "    let\n",
        "        s0 = Random.seed 7\n",
        "        i = Random.seededInt s0 1 10\n",
        "        f = Random.seededFloat s0\n",
        "        c = Random.seededChoice s0 [ 1, 2, 3 ]\n",
        "    in\n",
        "    case i of\n",
        "        ( v, _ ) -> String.fromInt v\n\n",
        "draw : Task Error String\n",
        "draw =\n",
        "    Random.int 1 6\n",
        "        |> Task.andThen (\\_ -> Random.float 0.0 1.0)\n",
        "        |> Task.andThen (\\_ -> Random.range 1 6)\n",
        "        |> Task.andThen (\\_ -> Random.choice [ 10, 20 ])\n",
        "        |> Task.andThen (\\_ -> Random.shuffle [ 1, 2, 3 ])\n",
        "        |> Task.andThen (\\_ -> Random.weighted [ ( 1.0, \"a\" ) ])\n",
        "        |> Task.map (\\_ -> seededLine)\n\n",
        "main : Task Error ()\n",
        "main = Task.andThen (\\line -> Io.println line) draw\n",
    );

    let outcome = compile_main(main);
    assert!(
        outcome.is_ok(),
        "the whole Ipe.Random surface must resolve + type-check: {:?}",
        outcome.err(),
    );
}

// ── Kernel-alias annotation ≡ enforced scheme ───────────────────────────────
//
// A `Module.member` that resolves to a kernel (every veneer value, and every
// point-free `x = Kernel.kernel "…"` alias of a compiled-source module) is typed
// by the kernel's registered scheme — its source annotation is never checked.
// The annotation is nonetheless the documented contract (`ipe doc` renders it),
// so it must EQUAL the enforced scheme. The probe below proves that equality
// through the real pipeline: `annN : <annotation>` / `annN = M.member` fails the
// compile when the annotation contradicts or over-generalises the scheme, and
// the accepted `annN` vs the scheme of the kernel `infN = M.member` resolves to
// differ (up to type-variable renaming) when the annotation is merely more
// specific.

/// Which exposed values of a module resolve to a kernel scheme.
#[derive(Clone, Copy)]
enum AliasScope {
    /// A veneer: every exposed value resolves through the qualifier catalog.
    EveryExposedValue,
    /// A compiled-source module: only point-free `Kernel.kernel "…"` aliases
    /// skip checking; every other body is compiled against its annotation.
    KernelAliasesOnly,
}

/// One exposed kernel-resolved member and its source annotation text.
struct AliasMember {
    name: String,
    annotation: String,
}

/// A module's kernel-resolved members plus the import context their
/// annotations are written in.
struct ProbeModule {
    dotted: String,
    /// The module's own `import` lines (the kernel-alias import excluded).
    imports: String,
    /// The exposed types the module itself owns (not re-exposed imports); the
    /// probe writes them `M.T` (a veneer's types are reachable only qualified).
    own_types: BTreeSet<String>,
    members: Vec<AliasMember>,
}

/// The pair-consistent renaming built while comparing two types.
#[derive(Default)]
struct VarPairs {
    left: BTreeMap<u32, u32>,
    right: BTreeMap<u32, u32>,
}

impl VarPairs {
    /// Record `a ↔ b`; `false` when either side is already paired elsewhere.
    fn pair(&mut self, a: u32, b: u32) -> bool {
        let l = *self.left.entry(a).or_insert(b);
        let r = *self.right.entry(b).or_insert(a);
        l == b && r == a
    }
}

/// Alpha-equivalence: equal up to a bijective renaming of type (and row)
/// variables.
fn alpha_eq(a: &Ty, b: &Ty, interner: &Interner, vars: &mut VarPairs) -> bool {
    match (a, b) {
        (Ty::Var(x), Ty::Var(y)) => vars.pair(*x, *y),
        (Ty::Fun(a1, r1), Ty::Fun(a2, r2)) => {
            alpha_eq(a1, a2, interner, vars) && alpha_eq(r1, r2, interner, vars)
        }
        (
            Ty::Con {
                module: m1,
                name: n1,
                args: x1,
            },
            Ty::Con {
                module: m2,
                name: n2,
                args: x2,
            },
        ) => {
            ipe_types::con_heads_compatible(m1, *n1, m2, *n2, interner)
                && x1.len() == x2.len()
                && x1
                    .iter()
                    .zip(x2)
                    .all(|(p, q)| alpha_eq(p, q, interner, vars))
        }
        (Ty::Unit, Ty::Unit) => true,
        (Ty::Tuple(x1), Ty::Tuple(x2)) => {
            x1.len() == x2.len()
                && x1
                    .iter()
                    .zip(x2)
                    .all(|(p, q)| alpha_eq(p, q, interner, vars))
        }
        (Ty::Record(f1, t1), Ty::Record(f2, t2)) => {
            let tails = match (t1, t2) {
                (RowTail::Closed, RowTail::Closed) => true,
                (RowTail::Open(x), RowTail::Open(y)) => vars.pair(*x, *y),
                (RowTail::Closed, RowTail::Open(_)) | (RowTail::Open(_), RowTail::Closed) => false,
            };
            tails
                && f1.len() == f2.len()
                && f1
                    .iter()
                    .zip(f2)
                    .all(|((k1, v1), (k2, v2))| k1 == k2 && alpha_eq(v1, v2, interner, vars))
        }
        _ => false,
    }
}

fn render(ty: &Ty, interner: &Interner) -> String {
    ipe_types::ty_to_doc(ty, interner, &mut VarNamer::new()).map_or_else(
        |e| format!("{ty:?} (unrenderable: {e:?})"),
        |doc| ipe_diagnostics::render_ty(&doc),
    )
}

fn span_text<'s>(source: &'s str, span: ipe_diagnostics::Span) -> Option<&'s str> {
    let lo = usize::try_from(span.lo).ok()?;
    let hi = usize::try_from(span.hi).ok()?;
    source.get(lo..hi)
}

/// Collect a module's exposed kernel-resolved members; `Err` names a member
/// the gate cannot check (unparsable module, missing annotation).
fn alias_members(dotted: &str, source: &str, scope: AliasScope) -> Result<ProbeModule, String> {
    use ipe_syntax::{Exposed, Exposing, Expr_};

    let mut interner = Interner::new();
    let parsed = ipe_parse::parse_module(source, &mut interner)
        .map_err(|e| format!("{dotted}: failed to parse: {e:?}"))?;
    let name = |s| interner.resolve(s).unwrap_or_default().to_owned();

    let exposed: Option<Vec<&Exposed>> = match &parsed.exposing.value {
        Exposing::All => None,
        Exposing::List(items) => Some(items.iter().map(|item| &item.value).collect()),
    };
    let exposed_values: Option<BTreeSet<String>> = exposed.as_ref().map(|items| {
        items
            .iter()
            .filter_map(|e| match e {
                Exposed::Value(n) => Some(name(*n)),
                Exposed::Type(_, _) => None,
            })
            .collect()
    });
    let exposed_type_names: Option<BTreeSet<String>> = exposed.as_ref().map(|items| {
        items
            .iter()
            .filter_map(|e| match e {
                Exposed::Type(n, _) => Some(name(*n)),
                Exposed::Value(_) => None,
            })
            .collect()
    });
    let imported_types: BTreeSet<String> = parsed
        .imports
        .iter()
        .filter_map(|import| match &import.exposing.value {
            Exposing::List(items) => Some(items),
            Exposing::All => None,
        })
        .flatten()
        .filter_map(|item| match &item.value {
            Exposed::Type(n, _) => Some(name(*n)),
            Exposed::Value(_) => None,
        })
        .collect();
    // An exposed type the module does not import is its own, whether declared
    // in source or supplied by the kernel (`Ipe.Ui.Tui.Attribute`).
    let own_types: BTreeSet<String> = match exposed_type_names {
        Some(set) => set.difference(&imported_types).cloned().collect(),
        None => parsed
            .unions
            .iter()
            .map(|u| name(u.value.name.value))
            .chain(parsed.aliases.iter().map(|a| name(a.value.name.value)))
            .collect(),
    };

    let mut kernel_alias: Option<ipe_intern::Symbol> = None;
    let mut imports = String::new();
    for import in &parsed.imports {
        let path: Vec<String> = import.name.value.iter().map(|s| name(*s)).collect();
        if path == ["Ipe", "Ffi", "Kernel"] {
            kernel_alias = import.alias;
            continue;
        }
        let text = span_text(source, import.span)
            .ok_or_else(|| format!("{dotted}: import span out of range"))?;
        imports.push_str(text);
        imports.push('\n');
    }

    let mut members = Vec::new();
    for value in &parsed.values {
        let value = &value.value;
        let member = name(value.name.value);
        if !exposed_values
            .as_ref()
            .is_none_or(|set| set.contains(&member))
        {
            continue;
        }
        let is_kernel_alias = value.patterns.is_empty()
            && matches!(
                &value.body.value,
                Expr_::Call(callee, args)
                    if args.len() == 1
                        && matches!(
                            &callee.value,
                            Expr_::VarQual(q, k)
                                if Some(*q) == kernel_alias && interner.resolve(*k) == Some("kernel")
                        )
            );
        if matches!(scope, AliasScope::KernelAliasesOnly) && !is_kernel_alias {
            continue;
        }
        let annotation = value
            .type_annotation
            .as_ref()
            .and_then(|a| span_text(source, a.span))
            .ok_or_else(|| {
                format!("{dotted}.{member}: exposed kernel-resolved value has no annotation")
            })?;
        members.push(AliasMember {
            name: member,
            annotation: annotation.to_owned(),
        });
    }
    Ok(ProbeModule {
        dotted: dotted.to_owned(),
        imports,
        own_types,
        members,
    })
}

const PROBE_MAIN: &str = "module Main exposing (main)\nimport Ipe.Io as Io\nimport Probe\n\n\
                          main : Task Error ()\nmain =\n    Io.println \"ok\"\n";

fn probe_path() -> Vec<String> {
    vec!["Probe".to_owned()]
}

/// `annotation` with every bare, unqualified `own_types` name written `M.T`.
fn qualify(annotation: &str, own_types: &BTreeSet<String>) -> String {
    let mut out = String::with_capacity(annotation.len());
    let mut word = String::new();
    let mut prev: Option<char> = None;
    let mut before_word: Option<char> = None;
    let flush = |word: &mut String, before: Option<char>, next: Option<char>, out: &mut String| {
        if !word.is_empty() {
            if before != Some('.') && next != Some('.') && own_types.contains(word.as_str()) {
                out.push_str("M.");
            }
            out.push_str(word);
            word.clear();
        }
    };
    for c in annotation.chars() {
        if c.is_alphanumeric() || c == '_' {
            if word.is_empty() {
                before_word = prev;
            }
            word.push(c);
        } else {
            flush(&mut word, before_word, Some(c), &mut out);
            out.push(c);
        }
        prev = Some(c);
    }
    flush(&mut word, before_word, None, &mut out);
    out
}

fn probe_source(m: &ProbeModule, members: &[AliasMember]) -> String {
    let mut s = format!(
        "module Probe exposing (..)\n{}import {} as M\n",
        m.imports, m.dotted
    );
    for (i, member) in members.iter().enumerate() {
        let _ = write!(
            s,
            "\n\nann{i} : {}\nann{i} =\n    M.{}\n\n\ninf{i} =\n    M.{}\n",
            qualify(&member.annotation, &m.own_types),
            member.name,
            member.name
        );
    }
    s
}

/// A drift: the member index plus why its annotation is not the scheme.
type Drift = (usize, String);

/// Type-check one probe (routed through a `main`-less helper, so `Ipe.Tea.*`
/// shapes stay legal) and compare each accepted `annN` against the scheme of
/// the kernel the pipeline resolves `infN = M.member` to; `Err` is the
/// rendered resolution or type error.
///
/// The scheme is read from the kernel table inference instantiates, not from
/// `infN`'s solved type: an unannotated top-level binding is numeric-,
/// interpolation- and UI-msg-defaulted, which would pin `number`/`msg` to a
/// ground type the scheme never states.
fn run_probe(probe: &str, count: usize) -> Result<Vec<Drift>, String> {
    let mut user = UserSources::new();
    user.insert(entry_path(), PROBE_MAIN.to_owned());
    user.insert(probe_path(), probe.to_owned());
    let (sources, injected) = prepared(&user);
    let db = ipe_db::IpeDatabase::new();
    let root = ipe::create_source_root(&db, &sources, &injected, &BTreeSet::new());
    let files = root.files(&db);
    let (Some(entry), Some(module)) = (files.get(&entry_path()), files.get(&probe_path())) else {
        return Err("probe graph lacks `Main` or `Probe`".to_owned());
    };
    let render_diag = |d: &ipe_diagnostics::Diagnostic| {
        ipe_diagnostics::render(d, "<resolvability>/Probe.ipe", probe)
    };
    let types = ipe_db::typecheck_module(&db, root, *entry, *module)
        .clone()
        .map_err(|(d, _)| render_diag(&d))?;
    let canon = ipe_db::canonicalize(&db, root, *module)
        .clone()
        .map_err(|d| render_diag(&d))?;
    let kernel_table = ipe_db::kernel_types(&db, root)
        .clone()
        .map_err(|d| render_diag(&d))?;
    let interner = db.interner().lock();
    let annotated: BTreeMap<String, &Ty> = types
        .env
        .iter()
        .filter_map(|(k, t)| interner.resolve(*k).map(|n| (n.to_owned(), t)))
        .collect();
    // `infN = M.member` names the kernel the pipeline routes `M.member` to.
    let resolved: BTreeMap<String, &ipe_canon::ast::Expr_> = canon
        .module
        .defs
        .iter()
        .filter_map(|def| match def {
            ipe_canon::ast::Def::Untyped { name, body, .. } => interner
                .resolve(name.value)
                .map(|n| (n.to_owned(), &body.value)),
            ipe_canon::ast::Def::Typed { .. } => None,
        })
        .collect();
    let mut drifts = Vec::new();
    for i in 0..count {
        let (Some(ann), Some(route)) = (
            annotated.get(&format!("ann{i}")),
            resolved.get(&format!("inf{i}")),
        ) else {
            drifts.push((
                i,
                "probe binding missing from the solved program".to_owned(),
            ));
            continue;
        };
        let ipe_canon::ast::Expr_::VarKernel {
            id: Some(kernel), ..
        } = route
        else {
            drifts.push((i, "does not resolve to a registered kernel".to_owned()));
            continue;
        };
        let Some((_, inf)) = kernel_table.iter().find(|(k, _)| k == kernel) else {
            drifts.push((i, format!("kernel `{kernel:?}` carries no type scheme")));
            continue;
        };
        if !alpha_eq(ann, inf, &interner, &mut VarPairs::default()) {
            drifts.push((
                i,
                format!(
                    "annotated `{}` but the compiler assigns `{}`",
                    render(ann, &interner),
                    render(inf, &interner)
                ),
            ));
        }
    }
    Ok(drifts)
}

/// Every drifted member of `m`, each described as `Module.member : ann — why`.
fn module_drifts(m: &ProbeModule) -> Vec<String> {
    if m.members.is_empty() {
        return Vec::new();
    }
    let describe = |member: &AliasMember, why: &str| {
        format!(
            "{}.{} : {} — {why}",
            m.dotted, member.name, member.annotation
        )
    };
    match run_probe(&probe_source(m, &m.members), m.members.len()) {
        Ok(drifts) => drifts
            .iter()
            .filter_map(|(i, why)| m.members.get(*i).map(|member| describe(member, why)))
            .collect(),
        Err(whole) => {
            // Localise: re-probe each member alone so the culprit names itself.
            let singles: Vec<String> = m
                .members
                .iter()
                .filter_map(|member| {
                    match run_probe(&probe_source(m, std::slice::from_ref(member)), 1) {
                        Ok(drifts) => drifts.first().map(|(_, why)| describe(member, why)),
                        Err(e) => Some(describe(
                            member,
                            &format!("annotation rejected against the enforced scheme: {e}"),
                        )),
                    }
                })
                .collect();
            if singles.is_empty() {
                vec![format!("{}: probe failed as a whole: {whole}", m.dotted)]
            } else {
                singles
            }
        }
    }
}

/// Members whose annotation a probe cannot restate, each with the refusal that
/// forbids it. The gate re-probes each one and requires that exact refusal, so
/// an entry that stops being necessary fails the gate instead of lingering.
const UNPROBEABLE: &[(&str, &str)] = &[(
    // A user annotation may not leave a `CustomElement` seal polymorphic; only
    // the stdlib boundary member itself is generic over `down` / `up`.
    "Ipe.Ffi.Js.CustomElement.node",
    "IPE-N0039",
)];

/// Every exposed kernel-resolved value's annotation EQUALS the scheme the
/// compiler enforces for `Module.member`.
///
/// Covers every veneer value (`MODULES`) and every exposed point-free
/// `Kernel.kernel "…"` alias of a compiled-source module — the two places an
/// annotation is documentation the type checker never reads. Floors pin that
/// every veneer and the compiled-source aliases are actually probed, so the
/// gate cannot pass by checking nothing.
#[test]
fn kernel_alias_annotations_equal_enforced_schemes() {
    let modules = ipe_stdlib::MODULES
        .iter()
        .map(|m| (m.name, m.source, AliasScope::EveryExposedValue))
        .chain(
            ipe_stdlib::COMPILED_STD_MODULES
                .iter()
                .map(|m| (m.dotted, m.source, AliasScope::KernelAliasesOnly)),
        );
    let mut failures: Vec<String> = Vec::new();
    let mut probed: BTreeSet<String> = BTreeSet::new();
    let mut exempted: BTreeSet<String> = BTreeSet::new();
    for (dotted, source, scope) in modules {
        match alias_members(dotted, source, scope) {
            Ok(mut m) => {
                let (exempt, checked): (Vec<AliasMember>, Vec<AliasMember>) =
                    m.members.into_iter().partition(|member| {
                        let path = format!("{}.{}", m.dotted, member.name);
                        UNPROBEABLE.iter().any(|(p, _)| *p == path)
                    });
                m.members = checked;
                for member in &exempt {
                    let path = format!("{}.{}", m.dotted, member.name);
                    let code = UNPROBEABLE
                        .iter()
                        .find(|(p, _)| *p == path)
                        .map_or("", |(_, c)| c);
                    match run_probe(&probe_source(&m, std::slice::from_ref(member)), 1) {
                        Err(e) if e.contains(code) => {
                            exempted.insert(path);
                        }
                        other => failures.push(format!(
                            "{path}: listed UNPROBEABLE for {code} but the probe gave {other:?}"
                        )),
                    }
                }
                for member in &m.members {
                    probed.insert(format!("{}.{}", m.dotted, member.name));
                }
                failures.extend(module_drifts(&m));
            }
            Err(e) => failures.push(e),
        }
    }
    for veneer in ipe_stdlib::MODULES {
        let prefix = format!("{}.", veneer.name);
        assert!(
            probed.iter().any(|p| p.starts_with(&prefix)),
            "veneer {} contributed no probed member — the gate would skip it",
            veneer.name,
        );
    }
    for (path, _) in UNPROBEABLE {
        assert!(
            exempted.contains(*path),
            "UNPROBEABLE entry `{path}` names no exposed kernel alias"
        );
    }
    for pinned in ["Ipe.File.readFile", "Ipe.Random.int", "Ipe.System.exit"] {
        assert!(
            probed.contains(pinned),
            "`{pinned}` must be probed; probed = {probed:?}"
        );
    }
    assert!(
        failures.is_empty(),
        "every exposed kernel-resolved stdlib value's annotation must equal the \
         scheme the compiler enforces — a drift documents a contract the \
         compiler does not keep:\n{}",
        failures.join("\n"),
    );
}

/// Refusal: an annotation MORE SPECIFIC than the scheme type-checks (it is an
/// instance) yet documents the wrong contract — the comparator must flag it.
#[test]
fn alias_gate_flags_an_over_specific_annotation() {
    let m = ProbeModule {
        dotted: "Ipe.System".to_owned(),
        imports: String::new(),
        own_types: BTreeSet::new(),
        members: vec![AliasMember {
            name: "exit".to_owned(),
            annotation: "Int -> Int".to_owned(),
        }],
    };
    let drifts = module_drifts(&m);
    assert!(
        drifts.len() == 1 && drifts.iter().all(|d| d.contains("Ipe.System.exit")),
        "`exit : Int -> Int` over `Int -> a` must be one drift: {drifts:?}",
    );
}

/// Refusal: an annotation CONTRADICTING the scheme fails the probe compile and
/// is reported against the member.
#[test]
fn alias_gate_flags_a_contradicting_annotation() {
    let m = ProbeModule {
        dotted: "Ipe.File".to_owned(),
        imports: "import Ipe.Error exposing (Error)\n".to_owned(),
        own_types: BTreeSet::new(),
        members: vec![AliasMember {
            name: "readFile".to_owned(),
            annotation: "String -> Task Error String".to_owned(),
        }],
    };
    let drifts = module_drifts(&m);
    assert!(
        drifts.len() == 1 && drifts.iter().all(|d| d.contains("Ipe.File.readFile")),
        "`readFile : String -> …` over `Path -> …` must be one drift: {drifts:?}",
    );
}

/// Control: a type-variable renaming is the same scheme, not a drift.
#[test]
fn alias_gate_accepts_an_alpha_renamed_annotation() {
    let m = ProbeModule {
        dotted: "Ipe.System".to_owned(),
        imports: String::new(),
        own_types: BTreeSet::new(),
        members: vec![AliasMember {
            name: "exit".to_owned(),
            annotation: "Int -> zzz".to_owned(),
        }],
    };
    let drifts = module_drifts(&m);
    assert!(drifts.is_empty(), "`Int -> zzz` is `Int -> a`: {drifts:?}");
}

/// `File.readFileLimit` takes the documented `ByteSize` ceiling.
#[test]
fn file_read_file_limit_takes_a_byte_size_ceiling() {
    let main = concat!(
        "module Main exposing (main)\n",
        "import Ipe.ByteSize as ByteSize exposing (ByteSize)\n",
        "import Ipe.File as File\n",
        "import Ipe.Io as Io\n",
        "import Ipe.Path exposing (Path)\n",
        "import Ipe.Task as Task\n\n",
        "sourceCeiling : ByteSize\n",
        "sourceCeiling =\n",
        "    ByteSize.mib 16\n\n",
        "readCapped : Path -> Task Error String\n",
        "readCapped path =\n",
        "    File.readFileLimit path sourceCeiling\n\n",
        "main : Task Error ()\n",
        "main =\n",
        "    readCapped (path \"/tmp/ipe-probe\") |> Task.andThen Io.println\n",
    );
    let outcome = compile_main(main);
    assert!(
        outcome.is_ok(),
        "`File.readFileLimit path (ByteSize.mib 16)` must type-check: {:?}",
        outcome.err(),
    );
}

/// Refusal: a bare `Int` ceiling (unit-ambiguous) is rejected.
#[test]
fn file_read_file_limit_rejects_a_bare_int_ceiling() {
    let main = concat!(
        "module Main exposing (main)\n",
        "import Ipe.File as File\n",
        "import Ipe.Io as Io\n",
        "import Ipe.Task as Task\n\n",
        "main : Task Error ()\n",
        "main =\n",
        "    File.readFileLimit (path \"/tmp/ipe-probe\") 16 |> Task.andThen Io.println\n",
    );
    let outcome = compile_main(main);
    assert!(
        outcome.as_ref().is_err_and(|e| e.contains("ByteSize")),
        "a bare-`Int` ceiling must be a type error naming `ByteSize`: {outcome:?}",
    );
}

/// A `Main` that imports the `Styles` helper, so the helper is type-checked.
const STYLES_MAIN: &str = "module Main exposing (main)\nimport Ipe.Io as Io\nimport Styles\n\n\
                           main : Task Error ()\nmain =\n    Io.println \"ok\"\n";

fn compile_styles(styles: &str) -> Result<(), String> {
    compile_main_with_helper(
        STYLES_MAIN,
        &[(vec!["Styles".to_owned()], styles.to_owned())],
    )
}

/// The terminal engines' documented `Attribute msg` is nameable: qualified,
/// exposed bare, and through the `Ipe.Ui.Cells` re-export, each unifying with
/// the attributes the engine's builders mint.
#[test]
fn terminal_attribute_types_are_nameable() {
    let styles = concat!(
        "module Styles exposing (banner, bare, legacy, legacyView, line)\n",
        "import Ipe.Color.Ansi as Ansi\n",
        "import Ipe.Ui.Cells as Cells\n",
        "import Ipe.Ui.Cli as Cli\n",
        "import Ipe.Ui.Tui as Tui exposing (Attribute, Screen)\n\n",
        "emphasis : List (Tui.Attribute msg)\n",
        "emphasis =\n    [ Tui.bold, Tui.color Ansi.red ]\n\n",
        "banner : Screen msg\n",
        "banner =\n    Tui.el emphasis (Tui.text \"hi\")\n\n",
        "bare : List (Attribute msg)\n",
        "bare =\n    [ Tui.dim ]\n\n",
        "legacy : List (Cells.Attribute msg)\n",
        "legacy =\n    [ Tui.underline ]\n\n",
        "legacyView : Cells.Screen msg\n",
        "legacyView =\n    Cells.el legacy (Cells.text \"x\")\n\n",
        "lineAttrs : List (Cli.Attribute msg)\n",
        "lineAttrs =\n    [ Cli.bold ]\n\n",
        "line : Cli.Lines msg\n",
        "line =\n    Cli.line lineAttrs \"x\"\n",
    );
    let outcome = compile_styles(styles);
    assert!(
        outcome.is_ok(),
        "terminal `Attribute msg` annotations must type-check: {:?}",
        outcome.err(),
    );
}

/// Refusal: the two terminal engines' attributes stay distinct types — a Cli
/// attribute in a Tui builder is a type error, not a silent coercion.
#[test]
fn terminal_attribute_types_stay_distinct() {
    let styles = concat!(
        "module Styles exposing (banner)\n",
        "import Ipe.Ui.Cli as Cli\n",
        "import Ipe.Ui.Tui as Tui\n\n",
        "banner : Tui.Screen msg\n",
        "banner =\n    Tui.el [ Cli.bold ] (Tui.text \"hi\")\n",
    );
    let outcome = compile_styles(styles);
    assert!(
        outcome.as_ref().is_err_and(|e| e.contains("IPE-T0001")),
        "a Cli attribute in a Tui builder must be a type mismatch: {outcome:?}",
    );
}

/// Refusal: a Tui colour attribute takes a terminal-palette `AnsiColor`, so the
/// sRGB `Tui.Color` is rejected rather than rendered as a colour the terminal
/// may not have.
#[test]
fn tui_color_attribute_rejects_an_srgb_color() {
    let styles = concat!(
        "module Styles exposing (tint)\n",
        "import Ipe.Ui.Tui as Tui\n\n",
        "tint : Tui.Attribute msg\n",
        "tint =\n    Tui.color Tui.white\n",
    );
    let outcome = compile_styles(styles);
    assert!(
        outcome.as_ref().is_err_and(|e| e.contains("IPE-T0001")),
        "an sRGB `Tui.Color` in `Tui.color` must be a type mismatch: {outcome:?}",
    );
}
