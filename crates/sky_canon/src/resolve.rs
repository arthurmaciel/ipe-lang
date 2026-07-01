//! Name resolution: `sky_syntax` source tree → canonical AST. Port of the M0
//! subset of `Sky.Canonicalise.{Module,Expression,Pattern,Type}`.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use sky_diagnostics::{DResult, Diagnostic, Located, NameError, Span};
use sky_intern::{Interner, Symbol};
use sky_syntax as src;

use crate::ast as canon;
use crate::env::{CtorHome, Env, VarHome};

/// The maximum number of `did you mean` suggestions attached to an unresolved
/// name. Keeping it small prevents a wall of near-misses drowning the actual
/// error; the list is `(Levenshtein, name)`-sorted so the closest comes first.
const MAX_SUGGESTIONS: usize = 3;

/// The inclusive edit-distance ceiling for a suggestion. Mirrors the Haskell
/// reference (`Sky.Canonicalise.Module.suggestQualifier`): beyond two edits a
/// "did you mean" is more misleading than helpful, so silence wins.
const SUGGESTION_MAX_DISTANCE: usize = 2;

/// A registered `type alias` awaiting expansion at its use sites.
///
/// `params` are the declared type parameters in source order (empty for a
/// non-parametric alias). `body` is the right-hand-side annotation, kept in
/// source form so each use site can substitute its own arguments for `params`
/// and then expand — no later stage ever observes the alias name.
struct AliasDef {
    params: Vec<Symbol>,
    body: src::TypeAnnotation,
}

/// The immutable context threaded through [`canonicalise_type`]. Bundling the
/// read-only references keeps the recursive call under clippy's argument-count
/// ceiling while leaving the per-call mutable state (`free_vars`, `visited`,
/// `subst`) explicit at each call site.
struct TypeCtx<'a> {
    env: &'a Env,
    /// Maps every type name in scope (local unions + imported types) to its
    /// home module path. Local types map to `env.home`; imported types map to
    /// the dep's path. An absent entry means the name is a builtin (empty home)
    /// or a free type variable — `canonicalise_type` falls through to `Type::Var`.
    type_home_map: &'a BTreeMap<Symbol, Vec<Symbol>>,
    aliases: &'a BTreeMap<Symbol, AliasDef>,
    interner: &'a Interner,
    /// Span of the enclosing value annotation, used as the location for an
    /// alias-arity error (the type AST itself carries no inner spans).
    ann_span: Span,
}

/// Canonicalise a parsed module into its name-resolved form.
///
/// # Errors
/// Returns [`Diagnostic::Name`] for any name that resolves to neither a
/// constructor, a bound variable, a top-level binding, nor a kernel function
/// ([`NameError::ValueNotFound`] / [`NameError::ConstructorNotFound`] /
/// [`NameError::UnknownModule`] / [`NameError::NoSuchMember`], each with a
/// deterministic did-you-mean), or for a duplicated value/constructor/type name
/// ([`NameError::DuplicateValue`] / [`NameError::DuplicateConstructor`] /
/// [`NameError::DuplicateType`]). [`Diagnostic::CompilerBug`] if the interner's
/// symbol table is exhausted or a name symbol is not interned.
pub fn canonicalise(m: &src::Module, interner: &mut Interner) -> DResult<canon::Module> {
    let home = m.name.value.clone();
    let mut env = Env::initial(home, interner)?;
    // type_home_map and extra_aliases both start empty; canonicalise_with_env
    // populates type_home_map from this module's unions and merges extra_aliases
    // (empty here) with the module's own aliases.
    let mut type_home_map: BTreeMap<Symbol, Vec<Symbol>> = BTreeMap::new();
    let extra_aliases: BTreeMap<Symbol, AliasDef> = BTreeMap::new();
    canonicalise_with_env(m, &mut env, &mut type_home_map, extra_aliases, interner)
}

/// Canonicalise a module in a multi-module project context.
///
/// Called by [`crate::canonicalise_module`].
///
/// # Errors
/// [`NameError::ModulePathMismatch`] — declared module name does not match
/// `expected_path`.
/// [`NameError::ReservedNamespace`] — first path segment is `Sky` or `Std`.
/// [`NameError::ModuleNotFound`] — an `import` names a module absent from
/// `deps`.
/// [`NameError::NameNotExposed`] — an unqualified `exposing (name)` imports a
/// name the dep module does not export.
/// [`NameError::AmbiguousImport`] — two different dep modules both expose the
/// same unqualified name.
/// Any error that [`canonicalise`] can return.
pub fn canonicalise_module(
    m: &src::Module,
    expected_path: &[Symbol],
    deps: &BTreeMap<Vec<Symbol>, crate::ModuleExports>,
    interner: &mut Interner,
) -> DResult<(canon::Module, crate::ModuleExports)> {
    let home = m.name.value.clone();

    // SKY-N0023: declared module name must match the path the build driver
    // computed from the file's location.
    if home.as_slice() != expected_path {
        let declared = path_to_dot_string(interner, &home);
        let expected = path_to_dot_string(interner, expected_path);
        return Err(Diagnostic::Name {
            span: m.name.span,
            msg: NameError::ModulePathMismatch { declared, expected },
        });
    }

    // SKY-N0025: `Sky` and `Std` are reserved for the compiler's own stdlib.
    // User modules whose first path segment matches either are rejected here so
    // they never shadow prelude symbols downstream.
    let sky_sym = interner.intern("Sky")?;
    let std_sym = interner.intern("Std")?;
    if home
        .first()
        .copied()
        .is_some_and(|s| s == sky_sym || s == std_sym)
    {
        let name = path_to_dot_string(interner, &home);
        return Err(Diagnostic::Name {
            span: m.name.span,
            msg: NameError::ReservedNamespace { name },
        });
    }

    let mut env = Env::initial(home.clone(), interner)?;
    // type_home_map is extended first by dep-imported types, then by this
    // module's own unions in canonicalise_with_env. Having deps in the map first
    // means this module can reference imported types in its own type annotations.
    let mut type_home_map: BTreeMap<Symbol, Vec<Symbol>> = BTreeMap::new();
    // Aliases from dep modules (injected via `import … exposing (..)`).
    let mut injected_aliases: BTreeMap<Symbol, AliasDef> = BTreeMap::new();
    // Tracks which names have been brought into unqualified scope and from which
    // dep module path (for AmbiguousImport detection).
    let mut unqual_origins: BTreeMap<Symbol, Vec<Symbol>> = BTreeMap::new();

    // Process each import declaration, injecting the dep module's exports into
    // the current env.
    let mut unqual_ctor_origins: BTreeMap<Symbol, Vec<Symbol>> = BTreeMap::new();
    for import in &m.imports {
        let dep_path = &import.name.value;
        // SKY-kernel: Sky.* and Std.* are compiler stdlib modules whose
        // qualifiers are pre-installed by Env::initial via
        // install_prelude_qualifiers.  They are not in the user `deps` map and
        // need no injection — attempting a deps.get on them would produce a
        // spurious SKY-N0020 for every real module that imports Sky.Core.Prelude.
        if dep_path
            .first()
            .copied()
            .is_some_and(|s| s == sky_sym || s == std_sym)
        {
            continue;
        }
        // SKY-N0020: dep module must have been discovered + canonicalised before
        // this module in topological order.
        let dep = deps.get(dep_path).ok_or_else(|| {
            let name = path_to_dot_string(interner, dep_path);
            // Offer did-you-mean by collecting all known dep paths as dot strings.
            let sugg: Box<[Box<str>]> = deps
                .keys()
                .map(|p| path_to_dot_string(interner, p))
                .collect();
            Diagnostic::Name {
                span: import.name.span,
                msg: NameError::ModuleNotFound {
                    name,
                    suggestions: sugg,
                },
            }
        })?;

        inject_dep_exports(
            import,
            dep,
            &mut env,
            &mut type_home_map,
            &mut injected_aliases,
            &mut unqual_origins,
            &mut unqual_ctor_origins,
            interner,
        )?;
    }

    let canon_mod =
        canonicalise_with_env(m, &mut env, &mut type_home_map, injected_aliases, interner)?;
    // Build the export surface from the module's own `exposing (…)` clause.
    let exports = build_module_exports(&home, m, &env);
    Ok((canon_mod, exports))
}

/// Shared resolution body used by both [`canonicalise`] and
/// [`canonicalise_module`].
///
/// Both callers provide a pre-initialised `env` (with builtins already
/// installed) and an optionally pre-populated `type_home_map` (dep-imported
/// types) and `extra_aliases` (dep-imported type aliases). This function:
///
/// 1. Registers this module's own union types (with duplicate rejection).
/// 2. Merges `extra_aliases` with this module's own `type alias` declarations.
/// 3. Canonicalises constructor payload field types (second union pass).
/// 4. Registers top-level value names (with duplicate rejection).
/// 5. Canonicalises each value declaration body.
///
/// # Errors
/// Same set as [`canonicalise`].
fn canonicalise_with_env(
    m: &src::Module,
    env: &mut Env,
    type_home_map: &mut BTreeMap<Symbol, Vec<Symbol>>,
    extra_aliases: BTreeMap<Symbol, AliasDef>,
    interner: &mut Interner,
) -> DResult<canon::Module> {
    let home = env.home.clone();

    // Add this module's own types to the type-home map. Dep-imported types were
    // already added by inject_dep_exports before this call; using `entry/or_insert`
    // means a local type that shadows an import's type is registered as LOCAL.
    for u in &m.unions {
        type_home_map
            .entry(u.value.name.value)
            .or_insert_with(|| home.clone());
    }

    // Build the type-home map: every type name in scope mapped to its home
    // module path. For the single-module path, only this module's own unions are
    // registered (each maps to this module's home). The multi-module path also
    // adds imported types via `canonicalise_module`. The map is consulted by
    // `canonicalise_type` to set the `home` field of a `Type::Con`.
    //
    // Register unions + their constructors into the environment, rejecting any
    // duplicate type or constructor name (closes the silent last-wins that the
    // bare `insert` used to hide). The canonical `canon::Union` records (with
    // their canonicalised payload field types) are built in a second pass below,
    // once type aliases are collected — a field type may reference an alias.
    let mut seen_types: BTreeMap<Symbol, Span> = BTreeMap::new();
    let mut seen_ctors: BTreeMap<Symbol, Span> = BTreeMap::new();
    for u in &m.unions {
        let type_name = u.value.name.value;
        let type_span = u.value.name.span;
        if let Some(&first) = seen_types.get(&type_name) {
            return Err(Diagnostic::Name {
                span: type_span,
                msg: NameError::DuplicateType {
                    name: name_str(interner, type_name)?,
                    first,
                },
            });
        }
        seen_types.insert(type_name, type_span);
        register_union(&u.value, &home, env, &mut seen_ctors, interner)?;
    }

    // Collect type aliases. Both the non-parametric form (`type alias Count =
    // Int`) and the parametric form (`type alias Pair a = ( a, a )`) are
    // supported: a parametric alias records its declared parameters and is
    // expanded by substituting each use site's type arguments for the parameters
    // in the body (M2B). An alias name that collides with a union (or another
    // alias) is a duplicate type name. The aliased bodies are kept as source
    // annotations and expanded in-place at every use site by `canonicalise_type`,
    // so no later stage ever sees an alias.
    //
    // Start from `extra_aliases` (dep aliases injected by imports); local
    // definitions are added below and may shadow dep aliases of the same name.
    let mut aliases: BTreeMap<Symbol, AliasDef> = extra_aliases;
    for a in &m.aliases {
        let alias_name = a.value.name.value;
        let alias_span = a.value.name.span;
        if let Some(&first) = seen_types.get(&alias_name) {
            return Err(Diagnostic::Name {
                span: alias_span,
                msg: NameError::DuplicateType {
                    name: name_str(interner, alias_name)?,
                    first,
                },
            });
        }
        seen_types.insert(alias_name, alias_span);
        aliases.insert(
            alias_name,
            AliasDef {
                params: a.value.vars.iter().map(|v| v.value).collect(),
                body: a.value.body.value.clone(),
            },
        );
    }

    // Second union pass: now that aliases are collected, canonicalise every
    // constructor's payload field types (a field may reference an alias or
    // another local union) and build the canonical union records.
    let mut unions = Vec::with_capacity(m.unions.len());
    for u in &m.unions {
        unions.push(canonicalise_union(
            &u.value,
            env,
            type_home_map,
            &aliases,
            interner,
        )?);
    }

    // Register every top-level value name so bindings can be referenced before
    // their definition (mutual / forward references), rejecting duplicates.
    let mut seen_values: BTreeMap<Symbol, Span> = BTreeMap::new();
    for v in &m.values {
        let name = v.value.name.value;
        let name_span = v.value.name.span;
        if let Some(&first) = seen_values.get(&name) {
            return Err(Diagnostic::Name {
                span: name_span,
                msg: NameError::DuplicateValue {
                    name: name_str(interner, name)?,
                    first,
                },
            });
        }
        seen_values.insert(name, name_span);
        env.vars.insert(name, VarHome::TopLevel(home.clone()));
    }

    // Canonicalise each value declaration.
    let mut defs = Vec::with_capacity(m.values.len());
    for v in &m.values {
        defs.push(canonicalise_value(
            &v.value,
            env,
            type_home_map,
            &aliases,
            interner,
        )?);
    }

    Ok(canon::Module {
        name: home,
        unions,
        defs,
    })
}

/// Inject a dep module's exports into the resolving module's environment.
///
/// Called once per `import` declaration, in source order. Handles:
///
/// * Unqualified value/type/ctor injection for `exposing (..)` or an explicit
///   `exposing (name, Type(..))` list.
/// * SKY-N0022 (`NameNotExposed`) when the exposing list names a value or type
///   the dep does not export.
/// * SKY-N0024 (`AmbiguousImport`) when the same unqualified name was already
///   injected from a different dep module (applies to both VALUES and
///   CONSTRUCTORS — previously only values were checked).
/// * Qualifier registration (`import M as Q` or auto-qualifier = last segment).
///
/// # Errors
/// [`NameError::NameNotExposed`] / [`NameError::AmbiguousImport`]; or
/// [`Diagnostic::CompilerBug`] on an unresolvable symbol.
#[allow(clippy::too_many_lines, clippy::too_many_arguments)] // declarative injection table — splitting would obscure flow
fn inject_dep_exports(
    import: &src::Import,
    dep: &crate::ModuleExports,
    env: &mut Env,
    type_home_map: &mut BTreeMap<Symbol, Vec<Symbol>>,
    injected_aliases: &mut BTreeMap<Symbol, AliasDef>,
    unqual_origins: &mut BTreeMap<Symbol, Vec<Symbol>>,
    unqual_ctor_origins: &mut BTreeMap<Symbol, Vec<Symbol>>,
    interner: &Interner,
) -> DResult<()> {
    let dep_path = &dep.path;

    match &import.exposing.value {
        src::Exposing::All => {
            // Inject all dep values unqualified.
            for &name in &dep.values {
                check_and_inject_value(
                    name,
                    dep_path,
                    import.name.span,
                    env,
                    unqual_origins,
                    interner,
                )?;
            }
            // Inject all dep types (union homes) + all ctors.
            for (&type_name, home) in &dep.types {
                type_home_map.insert(type_name, home.clone());
                inject_ctors_for_type(
                    type_name,
                    dep,
                    env,
                    import.name.span,
                    unqual_ctor_origins,
                    interner,
                )?;
            }
            // Inject all dep aliases.
            for (&alias_name, ea) in &dep.aliases {
                injected_aliases
                    .entry(alias_name)
                    .or_insert_with(|| AliasDef {
                        params: ea.params.clone(),
                        body: ea.body.clone(),
                    });
            }
        }
        src::Exposing::List(items) => {
            for item in items {
                match &item.value {
                    src::Exposed::Value(name) => {
                        if !dep.values.contains(name) {
                            let name_s = name_str(interner, *name)?;
                            let module_s = path_to_dot_string(interner, dep_path);
                            let sugg = suggestions(*name, dep.values.iter().copied(), interner);
                            return Err(Diagnostic::Name {
                                span: item.span,
                                msg: NameError::NameNotExposed {
                                    module: module_s,
                                    name: name_s,
                                    suggestions: sugg,
                                },
                            });
                        }
                        check_and_inject_value(
                            *name,
                            dep_path,
                            item.span,
                            env,
                            unqual_origins,
                            interner,
                        )?;
                    }
                    src::Exposed::Type(type_name, privacy) => {
                        let is_union = dep.types.contains_key(type_name);
                        let is_alias = dep.aliases.contains_key(type_name);
                        if !is_union && !is_alias {
                            let name_s = name_str(interner, *type_name)?;
                            let module_s = path_to_dot_string(interner, dep_path);
                            let all_names = dep.types.keys().chain(dep.aliases.keys()).copied();
                            let sugg = suggestions(*type_name, all_names, interner);
                            return Err(Diagnostic::Name {
                                span: item.span,
                                msg: NameError::NameNotExposed {
                                    module: module_s,
                                    name: name_s,
                                    suggestions: sugg,
                                },
                            });
                        }
                        if is_union {
                            if let Some(home) = dep.types.get(type_name) {
                                type_home_map.insert(*type_name, home.clone());
                            }
                            let expose_ctors = !matches!(privacy, src::Privacy::Private);
                            if expose_ctors {
                                inject_ctors_for_type(
                                    *type_name,
                                    dep,
                                    env,
                                    item.span,
                                    unqual_ctor_origins,
                                    interner,
                                )?;
                            }
                        }
                        if is_alias && let Some(ea) = dep.aliases.get(type_name) {
                            injected_aliases
                                .entry(*type_name)
                                .or_insert_with(|| AliasDef {
                                    params: ea.params.clone(),
                                    body: ea.body.clone(),
                                });
                        }
                    }
                }
            }
        }
    }

    // Register the module qualifier. Explicit `as Alias` takes priority;
    // otherwise the last segment of the module path is the default qualifier
    // (Elm convention: `import Lib.Utils` makes `Utils.foo` available).
    let qualifier = import
        .alias
        .unwrap_or_else(|| dep_path.last().copied().unwrap_or_else(name_zero));
    let qual_map = env.qual_vars.entry(qualifier).or_default();
    for &v in &dep.values {
        qual_map.insert(v, VarHome::TopLevel(dep_path.clone()));
    }

    Ok(())
}

/// Bring a single unqualified value name from `dep_path` into scope, checking
/// for ambiguity with a prior import from a different module.
///
/// # Errors
/// [`NameError::AmbiguousImport`] when the name was already exposed unqualified
/// by a different dep module.
fn check_and_inject_value(
    name: Symbol,
    dep_path: &[Symbol],
    span: Span,
    env: &mut Env,
    unqual_origins: &mut BTreeMap<Symbol, Vec<Symbol>>,
    interner: &Interner,
) -> DResult<()> {
    if let Some(prior_path) = unqual_origins.get(&name) {
        if prior_path.as_slice() != dep_path {
            let name_s = name_str(interner, name)?;
            let prior_s = path_to_dot_string(interner, prior_path);
            let dep_s = path_to_dot_string(interner, dep_path);
            return Err(Diagnostic::Name {
                span,
                msg: NameError::AmbiguousImport {
                    name: name_s,
                    modules: Box::new([prior_s, dep_s]),
                },
            });
        }
        // Same module exposed again — harmless.
        return Ok(());
    }
    unqual_origins.insert(name, dep_path.to_vec());
    env.vars.insert(name, VarHome::TopLevel(dep_path.to_vec()));
    Ok(())
}

/// Inject all constructors belonging to `type_name` from `dep` into `env`,
/// applying the same SKY-N0024 ambiguity check that [`check_and_inject_value`]
/// applies to values.
///
/// `unqual_ctor_origins` is the parallel tracking map for constructors: each
/// constructor name maps to the dep-module path that first exposed it
/// unqualified.  A second dep exposing the same unqualified constructor name
/// triggers [`NameError::AmbiguousImport`].
///
/// # Errors
/// [`NameError::AmbiguousImport`] when a constructor name was already exposed
/// unqualified by a different dep module.
fn inject_ctors_for_type(
    type_name: Symbol,
    dep: &crate::ModuleExports,
    env: &mut Env,
    span: sky_diagnostics::Span,
    unqual_ctor_origins: &mut BTreeMap<Symbol, Vec<Symbol>>,
    interner: &Interner,
) -> DResult<()> {
    let dep_path = &dep.path;
    for ctor_home in dep.ctors.values() {
        if ctor_home.type_name == type_name {
            if let Some(prior_path) = unqual_ctor_origins.get(&ctor_home.name) {
                if prior_path.as_slice() != dep_path.as_slice() {
                    // Two different dep modules both expose this constructor
                    // unqualified — SKY-N0024 (same code as for value ambiguity).
                    let name_s = name_str(interner, ctor_home.name)?;
                    let prior_s = path_to_dot_string(interner, prior_path);
                    let dep_s = path_to_dot_string(interner, dep_path);
                    return Err(Diagnostic::Name {
                        span,
                        msg: NameError::AmbiguousImport {
                            name: name_s,
                            modules: Box::new([prior_s, dep_s]),
                        },
                    });
                }
                // Same module exposed again — harmless, no-op.
            } else {
                unqual_ctor_origins.insert(ctor_home.name, dep_path.clone());
                env.ctors.insert(ctor_home.name, ctor_home.clone());
            }
        }
    }
    Ok(())
}

/// Build a [`crate::ModuleExports`] from the module's own declarations filtered
/// by its `exposing (…)` clause.
///
/// Only names declared in THIS module are considered as exportable; re-exporting
/// an imported name is not yet supported (and would require a separate pass after
/// imports are resolved, which the current design defers to a later milestone).
fn build_module_exports(home: &[Symbol], m: &src::Module, env: &Env) -> crate::ModuleExports {
    let mut exports = crate::ModuleExports {
        path: home.to_owned(),
        ..crate::ModuleExports::default()
    };

    // Sets of names defined by THIS module (not imported from deps).
    let own_values: BTreeSet<Symbol> = m.values.iter().map(|v| v.value.name.value).collect();
    let own_types: BTreeSet<Symbol> = m.unions.iter().map(|u| u.value.name.value).collect();
    let own_alias_names: BTreeSet<Symbol> = m.aliases.iter().map(|a| a.value.name.value).collect();

    match &m.exposing.value {
        src::Exposing::All => {
            exports.values = own_values;
            for &type_name in &own_types {
                exports.types.insert(type_name, home.to_owned());
                for ctor_home in env.ctors.values() {
                    if ctor_home.type_name == type_name && ctor_home.home == home {
                        exports.ctors.insert(ctor_home.name, ctor_home.clone());
                    }
                }
            }
            for a in &m.aliases {
                exports.aliases.insert(
                    a.value.name.value,
                    crate::ExportedAlias {
                        params: a.value.vars.iter().map(|v| v.value).collect(),
                        body: a.value.body.value.clone(),
                    },
                );
            }
        }
        src::Exposing::List(items) => {
            for item in items {
                match &item.value {
                    src::Exposed::Value(name) => {
                        if own_values.contains(name) {
                            exports.values.insert(*name);
                        }
                    }
                    src::Exposed::Type(type_name, privacy) => {
                        if own_types.contains(type_name) {
                            exports.types.insert(*type_name, home.to_owned());
                            let expose_ctors = !matches!(privacy, src::Privacy::Private);
                            if expose_ctors {
                                for ctor_home in env.ctors.values() {
                                    if ctor_home.type_name == *type_name && ctor_home.home == home {
                                        exports.ctors.insert(ctor_home.name, ctor_home.clone());
                                    }
                                }
                            }
                        } else if own_alias_names.contains(type_name) {
                            for a in &m.aliases {
                                if a.value.name.value == *type_name {
                                    exports.aliases.insert(
                                        *type_name,
                                        crate::ExportedAlias {
                                            params: a.value.vars.iter().map(|v| v.value).collect(),
                                            body: a.value.body.value.clone(),
                                        },
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    exports
}

/// Build a dot-joined module path string from interned symbols for use in
/// diagnostic payloads. Segments that are not backed by the interner are
/// rendered as `?` (lossy) — this is acceptable because the result is only
/// used in `Box<str>` diagnostic fields, never as a key.
fn path_to_dot_string(interner: &Interner, path: &[Symbol]) -> Box<str> {
    path.iter()
        .map(|&s| interner.resolve(s).unwrap_or("?"))
        .collect::<Vec<_>>()
        .join(".")
        .into()
}

/// Register a union's constructors into the environment.
///
/// This is the *name-resolution* half: each constructor's home / type / index /
/// arity is recorded in `env.ctors` so a `VarCtor` reference or a constructor
/// pattern can resolve before any value body is canonicalised. The payload field
/// *types* are canonicalised separately in [`canonicalise_union`] (after type
/// aliases are collected, so a field type may itself reference an alias).
///
/// `seen_ctors` carries every constructor name already registered across the
/// module so a name reused in the same or a different union is rejected
/// ([`NameError::DuplicateConstructor`]) instead of silently overwriting the
/// earlier `env.ctors` entry.
///
/// # Errors
/// [`NameError::DuplicateConstructor`] on a repeated constructor name;
/// [`Diagnostic::CompilerBug`] if a constructor symbol is not interned.
fn register_union(
    u: &src::Union,
    home: &[Symbol],
    env: &mut Env,
    seen_ctors: &mut BTreeMap<Symbol, Span>,
    interner: &Interner,
) -> DResult<()> {
    let type_name = u.name.value;
    for (index, c) in u.ctors.iter().enumerate() {
        let name = c.value.name;
        let arity = c.value.args.len();
        if let Some(&first) = seen_ctors.get(&name) {
            return Err(Diagnostic::Name {
                span: c.span,
                msg: NameError::DuplicateConstructor {
                    name: name_str(interner, name)?,
                    first,
                },
            });
        }
        seen_ctors.insert(name, c.span);
        env.ctors.insert(
            name,
            CtorHome {
                home: home.to_vec(),
                type_name,
                name,
                index,
                arity,
            },
        );
    }
    Ok(())
}

/// Build the canonical [`canon::Union`] record for a source union, canonicalising
/// every constructor's payload field types under the union's type-variable scope.
///
/// Each field type variable (`a` in `Just a`) is a free variable of the type
/// annotation and resolves to a [`canon::Type::Var`]; a field that names the
/// union itself (`Tree` in `Node Tree Int Tree`) resolves to a local
/// [`canon::Type::Con`]. The union's declared `vars` are carried through in
/// declaration order — the lowerer's IR `type_params` order depends on it.
///
/// # Errors
/// [`NameError::AliasArity`] when a field type applies an alias with the wrong
/// argument count; [`Diagnostic::CompilerBug`] on a forged symbol.
fn canonicalise_union(
    u: &src::Union,
    env: &Env,
    type_home_map: &BTreeMap<Symbol, Vec<Symbol>>,
    aliases: &BTreeMap<Symbol, AliasDef>,
    interner: &Interner,
) -> DResult<canon::Union> {
    let type_name = u.name.value;
    let vars: Vec<Symbol> = u.vars.iter().map(|v| v.value).collect();
    let mut ctors = Vec::with_capacity(u.ctors.len());
    for (index, c) in u.ctors.iter().enumerate() {
        let name = c.value.name;
        let arity = c.value.args.len();
        let ctx = TypeCtx {
            env,
            type_home_map,
            aliases,
            interner,
            ann_span: c.span,
        };
        let mut args = Vec::with_capacity(c.value.args.len());
        for a in &c.value.args {
            // A constructor field type is canonicalised under an empty
            // substitution: each free type variable it mentions is one of the
            // union's `vars` and resolves to a `Type::Var`. The `free_vars` set is
            // local (the union's quantification, not a binding's), so it is
            // discarded — the declared `vars` are the authoritative parameter list.
            let mut free_vars = BTreeSet::new();
            let mut visited = Vec::new();
            let subst = BTreeMap::new();
            args.push(canonicalise_type(
                a,
                &ctx,
                &subst,
                &mut free_vars,
                &mut visited,
            )?);
        }
        ctors.push(canon::Ctor {
            name,
            index,
            arity,
            args,
            span: c.span,
        });
    }
    Ok(canon::Union {
        home: env.home.clone(),
        name: type_name,
        vars,
        ctors,
    })
}

/// Canonicalise a single top-level value declaration.
fn canonicalise_value(
    val: &src::Value,
    env: &Env,
    type_home_map: &BTreeMap<Symbol, Vec<Symbol>>,
    aliases: &BTreeMap<Symbol, AliasDef>,
    interner: &mut Interner,
) -> DResult<canon::Def> {
    // Add parameter-bound names to a body-local environment.
    let mut body_env = env.clone();
    for p in &val.patterns {
        bind_pattern_names(&p.value, &mut body_env);
    }

    let mut patterns = Vec::with_capacity(val.patterns.len());
    for p in &val.patterns {
        patterns.push(canonicalise_pattern(p, env, interner)?);
    }
    let body = canonicalise_expr(&val.body, &body_env, interner)?;

    match &val.type_annotation {
        None => Ok(canon::Def::Untyped {
            home: env.home.clone(),
            name: val.name,
            patterns,
            body,
        }),
        Some(ann) => {
            let mut free_vars = BTreeSet::new();
            let mut visited = Vec::new();
            let ctx = TypeCtx {
                env,
                type_home_map,
                aliases,
                interner,
                ann_span: ann.span,
            };
            let subst = BTreeMap::new();
            let ty = canonicalise_type(&ann.value, &ctx, &subst, &mut free_vars, &mut visited)?;
            // Order the quantified type variables by their resolved NAME, not by
            // `Symbol` id (intern order is allocation-dependent, hence not a
            // stable wire order). Determinism gate: a multi-tyvar annotation
            // must yield the same `free_vars` regardless of how the interner
            // happened to number the names.
            let mut free_vars: Vec<Symbol> = free_vars.into_iter().collect();
            free_vars.sort_by(|a, b| interner.resolve(*a).cmp(&interner.resolve(*b)));
            Ok(canon::Def::Typed {
                home: env.home.clone(),
                name: val.name,
                free_vars,
                patterns,
                body,
                ty,
            })
        }
    }
}

/// Bind every variable a pattern introduces as a local in the environment.
fn bind_pattern_names(p: &src::Pattern_, env: &mut Env) {
    match p {
        // The wildcard and the literal leaves all bind nothing.
        src::Pattern_::PAnything
        | src::Pattern_::PInt(_)
        | src::Pattern_::PBool(_)
        | src::Pattern_::PChar(_)
        | src::Pattern_::PStr(_) => {}
        src::Pattern_::PVar(name) => env.add_local(*name),
        src::Pattern_::PCtor(_, _, args) => {
            for a in args {
                bind_pattern_names(&a.value, env);
            }
        }
        // A tuple and a list pattern both bind every element's names.
        src::Pattern_::PTuple(elems) | src::Pattern_::PList(elems) => {
            for e in elems {
                bind_pattern_names(&e.value, env);
            }
        }
        src::Pattern_::PRecord(fields) => {
            // Field-pun: each field name binds a local of the same name.
            for f in fields {
                env.add_local(f.value);
            }
        }
        src::Pattern_::PAlias(inner, name) => {
            // The alias binds its name AND every name its inner pattern binds.
            bind_pattern_names(&inner.value, env);
            env.add_local(name.value);
        }
        src::Pattern_::PCons(head, tail) => {
            bind_pattern_names(&head.value, env);
            bind_pattern_names(&tail.value, env);
        }
    }
}

/// Canonicalise a pattern. M0 supports wildcard, var, and constructor patterns.
fn canonicalise_pattern(
    p: &src::Pattern,
    env: &Env,
    interner: &Interner,
) -> DResult<canon::Pattern> {
    let span = p.span;
    let node = match &p.value {
        src::Pattern_::PAnything => canon::Pattern_::PAnything,
        src::Pattern_::PVar(name) => canon::Pattern_::PVar(*name),
        src::Pattern_::PCtor(name, _, args) => {
            let Some(ctor) = env.lookup_ctor(*name) else {
                return Err(Diagnostic::Name {
                    span,
                    msg: NameError::ConstructorNotFound {
                        name: name_str(interner, *name)?,
                        suggestions: suggestions(*name, env.ctors.keys().copied(), interner),
                    },
                });
            };
            let home = ctor.home.clone();
            let type_name = ctor.type_name;
            let index = ctor.index;
            let mut can_args = Vec::with_capacity(args.len());
            for a in args {
                can_args.push(canonicalise_pattern(a, env, interner)?);
            }
            canon::Pattern_::PCtor {
                home,
                type_name,
                name: *name,
                index,
                args: can_args,
            }
        }
        src::Pattern_::PTuple(elems) => {
            let mut can_elems = Vec::with_capacity(elems.len());
            for e in elems {
                can_elems.push(canonicalise_pattern(e, env, interner)?);
            }
            canon::Pattern_::PTuple(can_elems)
        }
        src::Pattern_::PRecord(fields) => {
            // Field-pun record pattern: the field names carry through verbatim
            // (the binding of each as a local happens in `bind_pattern_names`).
            canon::Pattern_::PRecord(fields.clone())
        }
        src::Pattern_::PInt(n) => canon::Pattern_::PInt(*n),
        src::Pattern_::PBool(b) => canon::Pattern_::PBool(*b),
        src::Pattern_::PChar(c) => canon::Pattern_::PChar(c.clone()),
        src::Pattern_::PStr(s) => canon::Pattern_::PStr(s.clone()),
        src::Pattern_::PAlias(inner, name) => {
            let can_inner = canonicalise_pattern(inner, env, interner)?;
            canon::Pattern_::PAlias(Box::new(can_inner), *name)
        }
        src::Pattern_::PList(elems) => {
            let mut can_elems = Vec::with_capacity(elems.len());
            for e in elems {
                can_elems.push(canonicalise_pattern(e, env, interner)?);
            }
            canon::Pattern_::PList(can_elems)
        }
        src::Pattern_::PCons(head, tail) => {
            let can_head = canonicalise_pattern(head, env, interner)?;
            let can_tail = canonicalise_pattern(tail, env, interner)?;
            canon::Pattern_::PCons(Box::new(can_head), Box::new(can_tail))
        }
    };
    Ok(Located::new(span, node))
}

/// Canonicalise an expression, resolving every name.
fn canonicalise_expr(e: &src::Expr, env: &Env, interner: &mut Interner) -> DResult<canon::Expr> {
    let span = e.span;
    let node = match &e.value {
        src::Expr_::Int(n) => canon::Expr_::Int(*n),
        src::Expr_::Float(f) => canon::Expr_::Float(*f),
        src::Expr_::Str(s) => canon::Expr_::Str(s.clone()),
        src::Expr_::Char(c) => canon::Expr_::Char(c.clone()),
        src::Expr_::Unit => canon::Expr_::Unit,
        src::Expr_::VarLocal(name) => resolve_var(*name, span, env, interner)?,
        src::Expr_::VarQual(qual, name) => resolve_qual_var(*qual, *name, span, env, interner)?,
        src::Expr_::Call(f, args) => {
            let callee = canonicalise_expr(f, env, interner)?;
            let mut can_args = Vec::with_capacity(args.len());
            for a in args {
                can_args.push(canonicalise_expr(a, env, interner)?);
            }
            canon::Expr_::Call(Box::new(callee), can_args)
        }
        src::Expr_::Case(scrut, arms) => {
            let can_scrut = canonicalise_expr(scrut, env, interner)?;
            let mut branches = Vec::with_capacity(arms.len());
            for (pat, body) in arms {
                // Pattern-bound names are local in the arm body.
                let mut arm_env = env.clone();
                bind_pattern_names(&pat.value, &mut arm_env);
                let can_pat = canonicalise_pattern(pat, env, interner)?;
                let can_body = canonicalise_expr(body, &arm_env, interner)?;
                branches.push(canon::CaseBranch {
                    pat: can_pat,
                    body: can_body,
                });
            }
            canon::Expr_::Case(Box::new(can_scrut), branches)
        }
        src::Expr_::Lambda(params, body) => {
            // The lambda's parameters become locals in its body; every other
            // free name resolves against the enclosing scope (an outer local,
            // a top-level binding, or a kernel) exactly as a bare reference
            // would — that is the capture. The parameter patterns themselves
            // are resolved against the *enclosing* env (a constructor pattern
            // must resolve there), matching how `case` arms are handled.
            let mut body_env = env.clone();
            let mut can_params = Vec::with_capacity(params.len());
            for p in params {
                bind_pattern_names(&p.value, &mut body_env);
                can_params.push(canonicalise_pattern(p, env, interner)?);
            }
            let can_body = canonicalise_expr(body, &body_env, interner)?;
            canon::Expr_::Lambda(can_params, Box::new(can_body))
        }
        src::Expr_::Binops(pairs, final_) => canonicalise_binops(pairs, final_, env, interner)?,
        src::Expr_::Let(bindings, body) => {
            // Sequential (`let*`) scoping: each binding's value is resolved
            // against the enclosing scope plus the bindings before it, then its
            // name becomes a local for the bindings that follow and for the
            // `in` body. This matches the non-recursive nested-`Let` the lowerer
            // emits, so a self- or forward-reference resolves to an outer name or
            // fails cleanly (`ValueNotFound`) rather than miscompiling.
            let mut let_env = env.clone();
            let mut can_bindings = Vec::with_capacity(bindings.len());
            for b in bindings {
                let can_body = canonicalise_expr(&b.body, &let_env, interner)?;
                // The binder's value is resolved against the scope so far; then
                // every variable it introduces (a plain name, or each leaf of a
                // tuple / record destructure) becomes a local for the bindings
                // that follow and the `in` body. The binder pattern itself is
                // canonicalised against the enclosing env (consistent with how
                // `case` arms and lambda parameters resolve their patterns).
                let can_pat = canonicalise_pattern(&b.pat, &let_env, interner)?;
                bind_pattern_names(&b.pat.value, &mut let_env);
                can_bindings.push(canon::LetBinding {
                    pat: can_pat,
                    body: can_body,
                });
            }
            let can_in = canonicalise_expr(body, &let_env, interner)?;
            canon::Expr_::Let(can_bindings, Box::new(can_in))
        }
        src::Expr_::If(branches, else_expr) => {
            // `if` introduces no bindings: every condition and branch resolves
            // against the same enclosing scope.
            let mut can_branches = Vec::with_capacity(branches.len());
            for (cond, body) in branches {
                let can_cond = canonicalise_expr(cond, env, interner)?;
                let can_body = canonicalise_expr(body, env, interner)?;
                can_branches.push((can_cond, can_body));
            }
            let can_else = canonicalise_expr(else_expr, env, interner)?;
            canon::Expr_::If(can_branches, Box::new(can_else))
        }
        src::Expr_::Tuple(elems) => {
            // A tuple introduces no bindings: every element resolves against the
            // same enclosing scope.
            let mut can_elems = Vec::with_capacity(elems.len());
            for elem in elems {
                can_elems.push(canonicalise_expr(elem, env, interner)?);
            }
            canon::Expr_::Tuple(can_elems)
        }
        src::Expr_::List(elems) => {
            // A list literal introduces no bindings: every element resolves
            // against the same enclosing scope.
            let mut can_elems = Vec::with_capacity(elems.len());
            for elem in elems {
                can_elems.push(canonicalise_expr(elem, env, interner)?);
            }
            canon::Expr_::List(can_elems)
        }
        src::Expr_::Record(fields) => {
            // A record introduces no bindings: every field value resolves against
            // the same enclosing scope. The field names are labels, carried
            // unresolved; a duplicate is rejected (see `canonicalise_fields`).
            canon::Expr_::Record(canonicalise_fields(fields, env, interner)?)
        }
        src::Expr_::Access(record, field) => {
            // `record.field`: the record sub-expression resolves against the
            // enclosing scope; the field is a label carried through unresolved.
            let can_record = canonicalise_expr(record, env, interner)?;
            canon::Expr_::Access(Box::new(can_record), field.value)
        }
        src::Expr_::Update(base, fields) => {
            // `{ base | field = value, ... }`: the base names a record variable,
            // resolved against the enclosing scope exactly as a bare reference
            // would be (an unknown name is the usual `ValueNotFound`). The updated
            // fields resolve the same way as a literal's, with the same
            // duplicate-field rejection.
            let base_node = resolve_var(base.value, base.span, env, interner)?;
            let can_base = Located::new(base.span, base_node);
            let can_fields = canonicalise_fields(fields, env, interner)?;
            canon::Expr_::Update(Box::new(can_base), can_fields)
        }
    };
    Ok(Located::new(span, node))
}

/// Canonicalise a record field list (shared by the literal `{ f = v, ... }` and
/// the update `{ r | f = v, ... }` forms).
///
/// Each field value resolves against the enclosing scope; the field name is a
/// label carried through unresolved. A field name written twice would otherwise
/// silently collapse to one struct field, so a duplicate is rejected here — a
/// field is, in effect, a value defined more than once in the record.
fn canonicalise_fields(
    fields: &[(Located<Symbol>, src::Expr)],
    env: &Env,
    interner: &mut Interner,
) -> DResult<Vec<(Symbol, canon::Expr)>> {
    let mut seen: BTreeMap<Symbol, Span> = BTreeMap::new();
    let mut can_fields = Vec::with_capacity(fields.len());
    for (name, value) in fields {
        if let Some(first) = seen.get(&name.value) {
            return Err(Diagnostic::Name {
                span: name.span,
                msg: NameError::DuplicateValue {
                    name: name_str(interner, name.value)?,
                    first: *first,
                },
            });
        }
        seen.insert(name.value, name.span);
        let can_value = canonicalise_expr(value, env, interner)?;
        can_fields.push((name.value, can_value));
    }
    Ok(can_fields)
}

/// Resolve a bare name: constructor first, then variable. Unknown → error.
fn resolve_var(name: Symbol, span: Span, env: &Env, interner: &Interner) -> DResult<canon::Expr_> {
    if let Some(ctor) = env.lookup_ctor(name) {
        return Ok(canon::Expr_::VarCtor {
            home: ctor.home.clone(),
            type_name: ctor.type_name,
            name: ctor.name,
            index: ctor.index,
        });
    }
    match env.lookup_var(name) {
        Some(VarHome::Local) => Ok(canon::Expr_::VarLocal(name)),
        Some(VarHome::TopLevel(module)) => Ok(canon::Expr_::VarTopLevel {
            module: module.clone(),
            name,
        }),
        Some(VarHome::Kernel(m, f)) => Ok(canon::Expr_::VarKernel {
            module: *m,
            name: *f,
        }),
        // A bare value name can resolve to either a value binding or a
        // constructor used as a value, so the suggestion pool spans both
        // namespaces (value bindings first, then constructor names).
        None => Err(Diagnostic::Name {
            span,
            msg: NameError::ValueNotFound {
                name: name_str(interner, name)?,
                suggestions: suggestions(
                    name,
                    env.vars.keys().chain(env.ctors.keys()).copied(),
                    interner,
                ),
            },
        }),
    }
}

/// Resolve a qualified name `Qualifier.name`. Distinguishes an unknown
/// qualifier ([`NameError::UnknownModule`]) from a known qualifier missing the
/// member ([`NameError::NoSuchMember`]).
fn resolve_qual_var(
    qualifier: Symbol,
    name: Symbol,
    span: Span,
    env: &Env,
    interner: &Interner,
) -> DResult<canon::Expr_> {
    let Some(members) = env.qual_members(qualifier) else {
        // The qualifier itself is unknown: suggest from the known qualifiers
        // (kernel modules + import aliases).
        return Err(Diagnostic::Name {
            span,
            msg: NameError::UnknownModule {
                qualifier: name_str(interner, qualifier)?,
                suggestions: suggestions(qualifier, env.qual_vars.keys().copied(), interner),
            },
        });
    };
    match members.get(&name) {
        Some(VarHome::Kernel(m, f)) => Ok(canon::Expr_::VarKernel {
            module: *m,
            name: *f,
        }),
        Some(VarHome::TopLevel(module)) => Ok(canon::Expr_::VarTopLevel {
            module: module.clone(),
            name,
        }),
        Some(VarHome::Local) => Ok(canon::Expr_::VarLocal(name)),
        // The qualifier resolves but the member is absent: suggest from this
        // module's members.
        None => Err(Diagnostic::Name {
            span,
            msg: NameError::NoSuchMember {
                module: name_str(interner, qualifier)?,
                member: name_str(interner, name)?,
                suggestions: suggestions(name, members.keys().copied(), interner),
            },
        }),
    }
}

/// Operator associativity. Mirrors `Sky.Parse.Symbol.Assoc`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Assoc {
    Left,
    Right,
    None,
}

/// The precedence (higher binds tighter) and associativity of `op`.
///
/// Mirror of the Haskell reference `Sky.Parse.Symbol.precedence` for the
/// M1-core operator set; any operator outside the set defaults to `9 L` exactly
/// as the Haskell catch-all does.
const fn op_precedence(op: &str) -> (i32, Assoc) {
    match op.as_bytes() {
        b"*" | b"/" | b"//" | b"%" => (7, Assoc::Left),
        b"+" | b"-" => (6, Assoc::Left),
        b"++" | b"::" => (5, Assoc::Right),
        b"==" | b"/=" | b"<" | b">" | b"<=" | b">=" => (4, Assoc::None),
        b"&&" => (3, Assoc::Right),
        b"||" => (2, Assoc::Right),
        // Elm-exact pipe precedence: loosest operators (prec 0).
        // `|>` is left-associative:  `x |> f |> g` = `(x |> f) |> g`.
        // `<|` is right-associative: `f <| g <| x` = `f <| (g <| x)`.
        b"|>" => (0, Assoc::Left),
        b"<|" => (0, Assoc::Right),
        _ => (9, Assoc::Left),
    }
}

/// Canonicalise a binary-operator chain into a precedence-correct tree.
///
/// The parser records a chain `e0 op0 e1 op1 … opN-1 eN` as a *flat* list of
/// `(operand, operator)` pairs plus a trailing operand, without consulting
/// precedence. Here we re-associate it via precedence climbing (port of
/// `Sky.Canonicalise.Expression.canonicaliseBinops`), reading each operator's
/// precedence + associativity from [`op_precedence`].
///
/// Unlike the Haskell parser — which nests `Src.Binops` pairwise and so needs a
/// flattening pre-pass — the Rust parser already emits one flat chain per
/// syntactic level. A `Binop` *operand* therefore only ever arises from an
/// explicit parenthesised group, which must stay atomic; we never re-flatten
/// it, so the user's grouping is preserved.
fn canonicalise_binops(
    pairs: &[(src::Expr, Located<Symbol>)],
    final_: &src::Expr,
    env: &Env,
    interner: &mut Interner,
) -> DResult<canon::Expr_> {
    // No operators: just the final operand.
    if pairs.is_empty() {
        return Ok(canonicalise_expr(final_, env, interner)?.value);
    }

    let basics = interner.intern("Basics")?;

    // Canonicalise every operand once, left to right, into a front-poppable
    // queue; pair each operator with its precedence + associativity.
    let mut operands: VecDeque<canon::Expr> = VecDeque::with_capacity(pairs.len() + 1);
    let mut ops: VecDeque<(Located<Symbol>, i32, Assoc)> = VecDeque::with_capacity(pairs.len());
    for (operand, op) in pairs {
        operands.push_back(canonicalise_expr(operand, env, interner)?);
        let (prec, assoc) = op_precedence(name_or_empty(interner, op.value));
        ops.push_back((*op, prec, assoc));
    }
    operands.push_back(canonicalise_expr(final_, env, interner)?);

    let left = operands
        .pop_front()
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "sky_canon::canonicalise_binops",
            detail: "binop chain with operators but no operands".to_owned(),
        })?;
    let tree = climb_binops(0, left, &mut operands, &mut ops, basics, interner)?;
    Ok(tree.value)
}

/// Precedence-climbing core. Consumes operators of precedence ≥ `min_prec` from
/// the front of `ops`, each paired with the next operand from `operands`, and
/// folds them around `left`. Direct port of the Haskell `climb` helper.
fn climb_binops(
    min_prec: i32,
    mut left: canon::Expr,
    operands: &mut VecDeque<canon::Expr>,
    ops: &mut VecDeque<(Located<Symbol>, i32, Assoc)>,
    basics: Symbol,
    interner: &mut Interner,
) -> DResult<canon::Expr> {
    while let Some(&(op, prec, assoc)) = ops.front() {
        if prec < min_prec {
            break;
        }
        ops.pop_front();
        let next_operand = operands
            .pop_front()
            .ok_or_else(|| Diagnostic::CompilerBug {
                where_: "sky_canon::climb_binops",
                detail: "operator without a right operand".to_owned(),
            })?;
        // A left- (or non-) associative operator restricts its right subtree to
        // strictly-higher precedence; a right-associative one admits equal
        // precedence so it nests rightward.
        let next_min = match assoc {
            Assoc::Left | Assoc::None => prec + 1,
            Assoc::Right => prec,
        };
        let right = climb_binops(next_min, next_operand, operands, ops, basics, interner)?;
        left = combine_binop(left, op, right, basics, interner)?;
    }
    Ok(left)
}

/// Resolve an operator symbol to its text, or `""` when (impossibly) un-interned
/// — the empty string falls through [`op_precedence`] to the `9 L` default, so a
/// missing symbol degrades gracefully rather than panicking.
fn name_or_empty(interner: &Interner, sym: Symbol) -> &str {
    interner.resolve(sym).unwrap_or("")
}

/// Build a single resolved binary-operation node.
fn combine_binop(
    lhs: canon::Expr,
    op: Located<Symbol>,
    rhs: canon::Expr,
    basics: Symbol,
    interner: &mut Interner,
) -> DResult<canon::Expr> {
    let span = Span::new(lhs.span.lo, rhs.span.hi);
    // The cons operator `::` is not a kernel binop — it builds a list node so
    // the type checker can give it the proper `a -> List a -> List a` discipline
    // and the backend can lower it to the runtime list prepend.
    if interner.resolve(op.value) == Some("::") {
        return Ok(Located::new(
            span,
            canon::Expr_::Cons(Box::new(lhs), Box::new(rhs)),
        ));
    }
    // Pipe operators desugar to function application — no new AST node needed.
    // `x |> f`  ≡  `f x`  ⇒  Call(rhs, [lhs])
    // `f <| x`  ≡  `f x`  ⇒  Call(lhs, [rhs])
    // Correct in a curried language: `(g a) x ≡ g a x`, so a chain
    // `[1,2,3] |> List.map inc` becomes Call(Call(List.map,[inc]),[[1,2,3]]),
    // a shape already handled by the existing Call lowering path.
    if interner.resolve(op.value) == Some("|>") {
        return Ok(Located::new(
            span,
            canon::Expr_::Call(Box::new(rhs), vec![lhs]),
        ));
    }
    if interner.resolve(op.value) == Some("<|") {
        return Ok(Located::new(
            span,
            canon::Expr_::Call(Box::new(lhs), vec![rhs]),
        ));
    }
    let func = resolve_op_func(op.value, interner)?;
    Ok(Located::new(
        span,
        canon::Expr_::Binop {
            op: op.value,
            home: basics,
            func,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        },
    ))
}

/// Map an operator symbol to its kernel function name. M0 subset of
/// `Expression.resolveOpName`.
fn resolve_op_func(op: Symbol, interner: &mut Interner) -> DResult<Symbol> {
    let func: Option<&'static str> = match interner.resolve(op) {
        Some("+") => Some("add"),
        Some("-") => Some("sub"),
        Some("*") => Some("mul"),
        Some("/") => Some("fdiv"),
        Some("//") => Some("idiv"),
        Some("==") => Some("eq"),
        Some("/=") => Some("neq"),
        Some("<") => Some("lt"),
        Some(">") => Some("gt"),
        Some("<=") => Some("le"),
        Some(">=") => Some("ge"),
        Some("&&") => Some("and"),
        Some("||") => Some("or"),
        Some("++") => Some("append"),
        // Unknown operators map to their own name under Basics, matching the
        // Haskell fall-through (`_ -> Can.VarKernel "Basics" op`).
        _ => None,
    };
    // The immutable borrow above ends here, so interning is now permitted.
    func.map_or(Ok(op), |name| interner.intern(name))
}

/// Canonicalise a type annotation. M0 subset of `Canonicalise.Type`, extended
/// with `type alias` expansion (M1 non-parametric, M2B parametric): a `TType`
/// whose unqualified name registers as an alias is replaced in place by its
/// body, with the use site's type arguments substituted for the alias's declared
/// parameters, so no later stage observes the alias name.
///
/// `subst` maps an in-scope alias parameter to the (already canonicalised) type
/// argument bound to it; a `TVar` found in `subst` resolves to that type instead
/// of remaining free. `visited` carries the chain of aliases currently being
/// expanded along this path — a name already in the chain is a recursive alias,
/// whose expansion stops (the name is left as an opaque constructor) rather than
/// recursing forever (soundness over completeness: a cyclic alias is exotic, but
/// must never hang or crash the compiler).
///
/// # Errors
/// [`Diagnostic::Name`] ([`NameError::AliasArity`]) when an alias is applied to a
/// number of type arguments that differs from its declared parameter count; the
/// span is the enclosing annotation (the type AST carries no inner spans).
/// [`Diagnostic::CompilerBug`] if a name symbol is not interned.
fn canonicalise_type(
    t: &src::TypeAnnotation,
    ctx: &TypeCtx,
    subst: &BTreeMap<Symbol, canon::Type>,
    free_vars: &mut BTreeSet<Symbol>,
    visited: &mut Vec<Symbol>,
) -> DResult<canon::Type> {
    match t {
        src::TypeAnnotation::TLambda(a, b) => Ok(canon::Type::Lambda(
            Box::new(canonicalise_type(a, ctx, subst, free_vars, visited)?),
            Box::new(canonicalise_type(b, ctx, subst, free_vars, visited)?),
        )),
        src::TypeAnnotation::TVar(v) => {
            // A variable bound to an alias argument resolves to that argument; its
            // own free variables were recorded when the argument was canonicalised
            // at the use site, so it does not re-enter `free_vars` here. An unbound
            // variable is genuinely free and is quantified by the binding.
            Ok(subst.get(v).map_or_else(
                || {
                    free_vars.insert(*v);
                    canon::Type::Var(*v)
                },
                Clone::clone,
            ))
        }
        src::TypeAnnotation::TUnit => Ok(canon::Type::Unit),
        src::TypeAnnotation::TTuple(elems) => {
            let mut can_elems = Vec::with_capacity(elems.len());
            for e in elems {
                can_elems.push(canonicalise_type(e, ctx, subst, free_vars, visited)?);
            }
            Ok(canon::Type::Tuple(can_elems))
        }
        src::TypeAnnotation::TRecord(fields) => {
            // Each field type is canonicalised under the current substitution, so
            // a field variable bound by an enclosing alias argument resolves to it
            // and an unbound one is collected into `free_vars` (quantified by the
            // binding) — exactly the [`TVar`] handling above, applied per field.
            let mut can_fields = Vec::with_capacity(fields.len());
            for (name, fty) in fields {
                can_fields.push((
                    *name,
                    canonicalise_type(fty, ctx, subst, free_vars, visited)?,
                ));
            }
            Ok(canon::Type::Record(can_fields))
        }
        src::TypeAnnotation::TType(_, segments, args) => {
            let name = segments.last().copied().unwrap_or_else(|| {
                // An unnamed type cannot occur in the M0 grammar; fall back to
                // the home module's name so the node is still well-formed.
                ctx.env.home.last().copied().unwrap_or_else(name_zero)
            });
            // The M1 grammar rejects qualified types in annotations, so every
            // `TType` here is unqualified — the qualifier is always empty. The
            // type arguments are canonicalised under the current substitution
            // (they appear at the use site) regardless of whether `name` is an
            // alias or an ordinary constructor.
            let mut can_args = Vec::with_capacity(args.len());
            for a in args {
                can_args.push(canonicalise_type(a, ctx, subst, free_vars, visited)?);
            }
            // A registered alias not already mid-expansion (cycle) is expanded:
            // its declared parameters are bound to the canonicalised arguments and
            // the body is canonicalised under that fresh substitution. Arity must
            // match exactly — a type alias has to be fully applied.
            if !visited.contains(&name)
                && let Some(alias) = ctx.aliases.get(&name)
            {
                if can_args.len() != alias.params.len() {
                    return Err(Diagnostic::Name {
                        span: ctx.ann_span,
                        msg: NameError::AliasArity {
                            name: name_str(ctx.interner, name)?,
                            expected: alias.params.len(),
                            found: can_args.len(),
                        },
                    });
                }
                let body_subst: BTreeMap<Symbol, canon::Type> =
                    alias.params.iter().copied().zip(can_args).collect();
                visited.push(name);
                let expanded =
                    canonicalise_type(&alias.body, ctx, &body_subst, free_vars, visited)?;
                visited.pop();
                return Ok(expanded);
            }
            // Types in `type_home_map` are either local unions (home = this
            // module) or imported types (home = dep module). Builtins and free
            // type variables are absent from the map → empty home.
            let home = ctx.type_home_map.get(&name).cloned().unwrap_or_default();
            Ok(canon::Type::Con {
                home,
                name,
                args: can_args,
            })
        }
    }
}

/// Resolve a symbol to an owned name for a diagnostic payload.
///
/// # Errors
/// [`Diagnostic::CompilerBug`] (`SKY-I0010`) when the symbol is not backed by
/// the interner — every name here came from the parser via this interner, so a
/// miss is an impossible-invariant violation, surfaced rather than swallowed as
/// an empty identifier.
fn name_str(interner: &Interner, sym: Symbol) -> DResult<Box<str>> {
    interner
        .resolve(sym)
        .map(Box::<str>::from)
        .ok_or_else(|| Diagnostic::CompilerBug {
            where_: "intern.resolve",
            detail: "canonicaliser: name symbol not backed by the interner".to_owned(),
        })
}

/// Build the deterministic `did you mean` suggestion list for an unresolved
/// `typo`, drawn from `candidates`.
///
/// Candidates within [`SUGGESTION_MAX_DISTANCE`] edits (and not identical) are
/// kept, sorted by `(Levenshtein distance, name)` — a total, allocation-order-
/// independent ordering — and truncated to [`MAX_SUGGESTIONS`]. A candidate the
/// interner cannot resolve is skipped (it cannot be rendered) rather than
/// faulting the whole diagnostic.
fn suggestions(
    typo: Symbol,
    candidates: impl Iterator<Item = Symbol>,
    interner: &Interner,
) -> Box<[Box<str>]> {
    let Some(typo_str) = interner.resolve(typo) else {
        return Box::new([]);
    };
    let mut scored: Vec<(usize, Box<str>)> = candidates
        .filter_map(|c| interner.resolve(c))
        .map(|name| (levenshtein(typo_str, name), Box::<str>::from(name)))
        .filter(|(d, _)| *d > 0 && *d <= SUGGESTION_MAX_DISTANCE)
        .collect();
    // (distance, name) is a total order; `Box<str>` compares lexicographically.
    scored.sort();
    scored.dedup();
    scored
        .into_iter()
        .take(MAX_SUGGESTIONS)
        .map(|(_, name)| name)
        .collect()
}

/// Iterative Levenshtein edit distance over Unicode scalar values, computed
/// with two rolling rows and no indexing (the `indexing_slicing` lint is
/// denied workspace-wide). Sky identifiers are short ASCII names, so the
/// O(n·m) cost is negligible. Mirrors the Haskell reference's `levenshtein`.
fn levenshtein(a: &str, b: &str) -> usize {
    let b_chars: Vec<char> = b.chars().collect();
    // Row 0: cost of deleting every prefix of `b` (i.e. inserting into empty a).
    let mut prev: Vec<usize> = (0..=b_chars.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        // `curr[0]` = cost of deleting the first `i + 1` chars of `a`.
        let mut curr: Vec<usize> = Vec::with_capacity(b_chars.len() + 1);
        curr.push(i + 1);
        // `diag` tracks `prev[j - 1]`; for the first column that is `prev[0]`,
        // which always equals the row index `i`.
        let mut diag = i;
        for (cb, &up) in b_chars.iter().zip(prev.iter().skip(1)) {
            let cost = usize::from(ca != *cb);
            let left = curr.last().copied().unwrap_or(i + 1);
            let cell = (up + 1).min(left + 1).min(diag + cost);
            curr.push(cell);
            diag = up;
        }
        prev = curr;
    }
    prev.last().copied().unwrap_or(0)
}

/// The interned symbol for the empty string (symbol id 0 is never guaranteed,
/// so we cannot hardcode it). Used only on the unreachable unnamed-type path.
const fn name_zero() -> Symbol {
    Symbol::from_raw(0)
}
