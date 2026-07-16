//! Name resolution: `sky_syntax` source tree → canonical AST. Port of the M0
//! subset of `Sky.Canonicalise.{Module,Expression,Pattern,Type}`.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use sky_diagnostics::{DResult, Diagnostic, Located, NameError, Span};
use sky_intern::{Interner, Symbol};
use sky_kernels::StdlibKernel;
use sky_syntax as src;

use crate::ast as canon;
use crate::env::{CtorHome, Env, VarHome, WildcardOrigin};

/// The maximum number of `did you mean` suggestions attached to an unresolved
/// name. Keeping it small prevents a wall of near-misses drowning the actual
/// error; the list is `(Levenshtein, name)`-sorted so the closest comes first.
const MAX_SUGGESTIONS: usize = 3;

/// The inclusive edit-distance ceiling for a suggestion. Mirrors the Haskell
/// reference (`Sky.Canonicalise.Module.suggestQualifier`): beyond two edits a
/// "did you mean" is more misleading than helpful, so silence wins.
const SUGGESTION_MAX_DISTANCE: usize = 2;

/// Type-constructor names the compiler reserves for built-ins. A user `type` /
/// `type alias` whose name is one of these is rejected at declaration
/// ([`NameError::ReservedBuiltinType`], SKY-N0026).
///
/// This is the exact set that `sky_lower`'s `ir_type_from_ty` matches *ahead of*
/// its user-enum lookup (`enum_variants` guard). Because that match keys on the
/// type name alone, a user declaration of any of these names would be silently
/// overridden by the built-in IR mapping and miscompile with **no diagnostic** —
/// so the shadow must be rejected here, at the parse/canon boundary, rather than
/// validated downstream (parse-don't-validate; make-invalid-states-
/// unrepresentable).
///
/// Every entry is cited to its `crates/sky_lower/src/lower.rs::ir_type_from_ty`
/// arm (HEAD line numbers):
///
/// ```text
/// Int 2069, Float 2070, Bool 2071, String/Error 2077, Char 2078, Bytes 2081,
/// Task 2084, Maybe 2103, Result 2108, List 2114, Dict 2119, Set 2133,
/// Decoder 2148, Db 2162, Cmd 2167, Sub 2179, SqlValue/SqlField 2195,
/// Request 2203, Response 2204, Route 2205, Cookie 2206, Html 2221,
/// Element 2236, Attribute 2254, Event 2279, Length 2295, HAlign 2297,
/// VAlign 2298, Location 2299, PseudoClass 2300, Description 2301,
/// LayoutContext 2302, LiveReq 2304.
/// ```
///
/// `SqlFragment` (backlog #61) was added after this citation list was drawn
/// up; see its own arm in `ir_type_from_ty` / `ir_type_from_canon`.
///
/// Several names that `ir_type_from_ty` also matches are deliberately EXCLUDED,
/// because #101 moved them BELOW the `enum_variants` guard in BOTH lowering
/// paths (ty + canon), so a program union of that name wins by its
/// `(home, name)` identity and only a genuine opaque builtin (no union entry)
/// reaches the fallback arm:
///   * `Value` — matched *after* the `enum_variants` guard, so a user
///     `type Value` already wins; it is not a silent-override hole.
///   * `Color`, `Length`, `HAlign`, `VAlign`, `Location`, `PseudoClass`,
///     `Description`, `LayoutContext`, `LiveReq` — the nullary Std.Ui / Sky.Live
///     opaque names. Leaving them UNRESERVED is what lets a user ADT — and,
///     crucially, a compiled-source `Std.Css` type (`Color` / `Length` / …) —
///     declare them; the home-aware guard (#100/#101) keeps the genuine Std.Ui
///     builtin resolving to `UiPlain`. Multiple shipped `.sky` fixtures
///     (`m4d_dict_adt_gate`, `m4d_set_adt_fn_gate`, `mm_local_pkg`, …) already
///     declare `type Color` as a benign sample ADT and now lower correctly.
const RESERVED_BUILTIN_TYPES: &[&str] = &[
    "Int",
    "Float",
    "Bool",
    "String",
    "Error",
    "Char",
    "Bytes",
    "Task",
    "Maybe",
    "Result",
    "List",
    "Dict",
    "Set",
    "Decoder",
    "Db",
    "Cmd",
    "Sub",
    "SqlValue",
    "SqlField",
    // `Std.Db.Sql`'s opaque WHERE-fragment type (backlog #61) — reserved (not
    // `EXTRA_BUILTIN_TYPE_NAMES`) so user shadowing of this security-tier type
    // is a hard canon error, matching the `SqlValue`/`SqlField` precedent.
    "SqlFragment",
    // `Sky.Core.Secret`'s opaque sealed secret-string type (backlog #44) —
    // reserved for the same reason as `SqlFragment`: a security-tier type
    // must not be shadowable by user code.
    "Secret",
    "StreamId",
    "ChunkEvent",
    "Request",
    "Response",
    "Route",
    "Cookie",
    "Html",
    "Element",
    "Attribute",
    "Event",
    "Length",
    "HAlign",
    "VAlign",
    "Location",
    "PseudoClass",
    "Description",
    "LayoutContext",
    "LiveReq",
];

/// Extra built-in type names that are handled by the lowerer's explicit arms
/// (`sky_lower::ir_type_from_canon`) but are NOT listed in
/// [`RESERVED_BUILTIN_TYPES`] (and therefore may NOT be user-defined).
///
/// These names must receive the empty-home sentinel (`Vec::new()`) from
/// `canonicalise_type` just like the reserved builtins — omitting them would
/// cause `canonicalise_type` to emit [`NameError::TypeNotFound`] for a
/// legitimate builtin annotation such as `relay : Order` or `ws : WebSocketServer`.
///
/// The names below are absent from `RESERVED_BUILTIN_TYPES` because they are
/// either:
/// * Nullary Std.Ui/Sky.Live opaque names whose lowerer arm sits BELOW the
///   `enum_variants` guard (so a user `type Color` wins by its real home) — and
///   therefore can never be shadowed in a user annotation either; OR
/// * Additional opaque kernel types added after the original reservation list
///   was drawn up.
///
/// Keeping this list in sync with `ir_type_from_canon`'s explicit arms is the
/// only invariant. Any name handled by an explicit arm with `home = []` that
/// is NOT in `RESERVED_BUILTIN_TYPES` belongs here.
const EXTRA_BUILTIN_TYPE_NAMES: &[&str] = &[
    // Three-way comparison result (`lt`/`eq`/`gt`). In the lowerer since #123.
    "Order",
    // Std.Ui plain types — lowerer guard is BELOW `enum_variants` so user ADTs
    // of the same name win via their real home; but annotations that name them
    // without a program-level definition still need the empty-home sentinel.
    "Color",
    "Length",
    "HAlign",
    "VAlign",
    "Location",
    "PseudoClass",
    "Description",
    "LayoutContext",
    "LiveReq",
    // Sky.Live / Sky.Http.Server / Sky.Http.Server.WebSocket opaque types.
    "LiveRoute",
    "StreamWriter",
    "HttpRequest",
    "WebSocketServer",
    "WebSocketServerCfg",
    // Std.Ui.Input parametric label/placeholder types (#124).
    "Label",
    "Placeholder",
    // Std.Decimal opaque arbitrary-precision decimal type.
    // Lowerer arm: `ir_type_from_canon` `"Decimal" => IrType::Decimal`.
    "Decimal",
    // `Std.Db.Migration` record alias `{ name : String, sql : String }`
    // (reference `Std/Db.sky:237`). Structural record — `normalize_annotation_ty`
    // expands the name to the record; the lowerer keeps it a synthesised struct
    // (no opaque arm), so it is user-shadowable-safe like `HttpRequest`.
    "Migration",
    // `Sky.Core.Error`'s `ErrorDetails` union (backlog #85 follow-up).
    // Lowerer arm: `ir_type_from_canon` `"ErrorDetails" => IrType::ErrorDetails`.
    // (`ErrorKind` — registered the same way at the same lowerer site — is a
    // PRE-EXISTING gap in this list, not introduced here; left alone to stay
    // strictly additive.)
    "ErrorDetails",
    // `Sky.Core.Error`'s NOMINAL payload types (SEAL fix 2026-07-11 —
    // `docs/adr/0017-error-payload-nominal-identity.md`).
    // Previously anonymous structural records; now opaque nominal Cons backed
    // by `sky_runtime::error::{SkyPanicInfo, SkyTypeInfo, SkyErrorInfo}`, so
    // annotations such as `describePanic : PanicInfo -> String` must resolve.
    // Lowerer arms: `ir_type_from_canon` / `ir_type_from_ty`
    // `"PanicInfo" => IrType::PanicInfo` (etc.).
    "PanicInfo",
    "TypeInfo",
    "ErrorInfo",
];

/// Kernel-implicit Prelude type names (#576) that are globally in scope in
/// every Sky program but are NOT declared by any compiled `.sky` source file —
/// they are resolved by the runtime as opaque handles.
///
/// These names were previously absent from ALL allowlists, so bare annotations
/// like `handleHome : Handler` failed with `TypeNotFound` / SKY-N0002 even
/// though they are legitimate kernel builtins. Each entry receives the
/// empty-home sentinel (`Vec::new()`) just like `RESERVED_BUILTIN_TYPES` and
/// `EXTRA_BUILTIN_TYPE_NAMES`.
///
/// Note: not all of these have explicit arms in `sky_lower::ir_type_from_canon`
/// yet (tracked as #138 follow-up for `Handler` / `Middleware` / `Session` / `Store` /
/// `VNode`). Adding them here is the canon-level fix; lowerer arms complete the
/// end-to-end path.
const KERNEL_IMPLICIT_PRELUDE_TYPE_NAMES: &[&str] = &[
    // `Request -> Task Error Response` alias from Sky.Http.Server (#576).
    "Handler",
    // `Html msg` — the top-level rendered HTML node type from Std.Html / Std.Ui.
    // Needed so `viewFoo : Model -> Html Msg` annotations typecheck without
    // `import Std.Html exposing (Html)`.
    "Html",
    // Opaque JSON value type (`Value = any` in Sky). The lowerer handles this
    // via an explicit arm placed after the `enum_variants` guard — so a user
    // `type Value` still wins, but a bare annotation compiles.
    "Value",
    // `Handler -> Handler` middleware alias from Sky.Http.Middleware.
    "Middleware",
    // Sky.Live session object.
    "Session",
    // Sky.Live session store.
    "Store",
    // Virtual DOM node (Sky.Live diff engine).
    "VNode",
];

/// The subset of [`RESERVED_BUILTIN_TYPES`] that a trusted
/// [`ModuleOrigin::EmbeddedStdlib`] module is permitted to DEFINE, while a
/// [`ModuleOrigin::User`] module stays rejected (SKY-N0026).
///
/// These are exactly the nullary Std.Ui / Sky.Live opaque names that #101 moved
/// BELOW the home-aware `enum_variants` guard in the lowerer
/// (`sky_lower::ir_type_from_ty` + `ir_type_from_canon`). Because the lowerer
/// now keys a program-defined `type Length` under its real `(home, name)`
/// (#100) and resolves it to its OWN enum, a compiled-source stdlib module
/// (`Std.Css` / the `Std.Palette` spike) can canonically DEFINE these types
/// without the former `UiPlain` hijack — so canon reservation is no longer
/// load-bearing for lowering-soundness on this exact set.
///
/// The reservation is retained for [`ModuleOrigin::User`] as a defence-in-depth
/// *user-facing guarantee* ("you cannot shadow `Length`" → a clean SKY-N0026
/// rather than a confusing dual-`Length` type-boundary error against the
/// built-in `UiPlain::Length`). The carve-out is keyed on the UNFORGEABLE typed
/// [`ModuleOrigin`] (#98/#100), never on module text: a hostile user file named
/// `Std.Css` is discovered as User source and gets NEITHER this exemption NOR
/// the SKY-N0025 namespace exemption.
///
/// NOTE: the load-bearing built-in names whose lowerer arms sit ABOVE the
/// `enum_variants` guard (`Html` / `Attribute` / `Event` / `Element`, plus every
/// primitive like `Int` / `Task` / `List`) are deliberately EXCLUDED — even the
/// trusted stdlib must not redefine them, since a same-named union would be
/// hijacked by the bare-name arm and mis-lower. This set is precisely the
/// below-guard nullary opaque names, and nothing else.
const STDLIB_DEFINABLE_UI_TYPES: &[&str] = &[
    "Length",
    "HAlign",
    "VAlign",
    "Location",
    "PseudoClass",
    "Description",
    "LayoutContext",
    "LiveReq",
];

/// The subset of [`RESERVED_BUILTIN_TYPES`] that a trusted
/// [`ModuleOrigin::EmbeddedStdlib`] module may DEFINE as the source-level
/// re-declaration of a shared opaque BOXED-WRAPPER carrier — as opposed to the
/// nullary Std.Ui plain names in [`STDLIB_DEFINABLE_UI_TYPES`].
///
/// `Decoder` is the shared row-decoder carrier (`IrType::Decoder`, runtime
/// `sky_runtime::json::Decoder<E, T>`). `Sky.Core.Json.Decode` names it as a
/// bare reserved builtin (no source declaration — it is a qualifier-only kernel
/// module). `Std.Config` is a compiled-source module that `exposing (Decoder)`
/// and re-declares `type Decoder a = Decoder` to put the name in its export set,
/// exactly as the Go/Haskell reference does — Config's decoders and JSON's share
/// one carrier, differing only in the parse front-end.
///
/// Unlike [`STDLIB_DEFINABLE_UI_TYPES`] (below-guard nullary names that must
/// lower to the module's OWN enum), this re-declaration is SOUND precisely
/// because `Decoder`'s lowerer arm sits ABOVE the `enum_variants` guard AND
/// `sky_lower::is_opaque_boxed_wrapper` recognises it: every `Decoder a`
/// annotation lowers to the shared `IrType::Decoder(a)` carrier and the
/// `type Decoder a = Decoder` declaration injects no competing enum. A
/// [`ModuleOrigin::User`] module stays rejected (SKY-N0026), so the shared
/// security-tier carrier cannot be shadowed by user code.
const STDLIB_DEFINABLE_CARRIER_TYPES: &[&str] = &["Decoder"];

/// Reject a `type` / `type alias` whose name shadows a reserved built-in type
/// constructor. See [`RESERVED_BUILTIN_TYPES`].
///
/// A [`ModuleOrigin::EmbeddedStdlib`] module is exempt for the
/// [`STDLIB_DEFINABLE_UI_TYPES`] subset (nullary Std.Ui plain names — `Std.Css`)
/// and the [`STDLIB_DEFINABLE_CARRIER_TYPES`] subset (shared opaque boxed-wrapper
/// carriers — `Std.Config`'s `Decoder`). A [`ModuleOrigin::User`] module is gated
/// against the full reserved set, so the default user-facing behaviour is
/// byte-identical.
fn reject_reserved_builtin_type(
    name: Symbol,
    span: Span,
    origin: ModuleOrigin,
    interner: &Interner,
) -> DResult<()> {
    match interner.resolve(name) {
        Some(resolved)
            if RESERVED_BUILTIN_TYPES.contains(&resolved)
                && !(origin == ModuleOrigin::EmbeddedStdlib
                    && (STDLIB_DEFINABLE_UI_TYPES.contains(&resolved)
                        || STDLIB_DEFINABLE_CARRIER_TYPES.contains(&resolved))) =>
        {
            Err(Diagnostic::Name {
                span,
                msg: NameError::ReservedBuiltinType {
                    name: Box::<str>::from(resolved),
                },
            })
        }
        _ => Ok(()),
    }
}

/// Trust provenance of a module entering [`canonicalise_module_with_origin`].
///
/// This is the *unforgeable* answer to upstream's "a user file named `Std.Auth`
/// silently shadows the audited stdlib" supply-chain hazard. The tag is a value
/// the build **driver** constructs — set to [`Self::EmbeddedStdlib`] *only* for a
/// module whose source came from `skyc`'s compile-time embed table, and never
/// derivable from module text. A hostile user file literally named `Std.Palette`
/// is discovered as ordinary user source, tagged [`Self::User`], and stays
/// rejected by the reserved-namespace gate (SKY-N0025).
///
/// MAKE INVALID STATES UNREPRESENTABLE: "this module is trusted stdlib" is a
/// typed value, not a string check on the module name.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ModuleOrigin {
    /// Ordinary user-authored source. Subject to every reserved-namespace and
    /// reserved-builtin-type gate.
    User,
    /// A compiled-source stdlib module injected from `skyc`'s embed table. Exempt
    /// from SKY-N0025 (it legitimately declares `module Std.…`) and required to
    /// be fully annotated (fail-closed gate below).
    EmbeddedStdlib,
}

/// A registered `type alias` awaiting expansion at its use sites.
///
/// `params` are the declared type parameters in source order (empty for a
/// non-parametric alias). `body` is the right-hand-side annotation, kept in
/// source form so each use site can substitute its own arguments for `params`
/// and then expand — no later stage ever observes the alias name.
#[derive(Clone)]
struct AliasDef {
    params: Vec<Symbol>,
    body: src::TypeAnnotation,
    /// When this alias was injected from a dep module, the dep module's
    /// complete `type_home_map` at the time of canonicalisation.  Used
    /// during body expansion to resolve type names that appear in the body
    /// but were NOT imported by the IMPORTING module — because the body
    /// references types from the DEP module's own deps.
    ///
    /// `None` for locally-declared aliases: they expand in the importing
    /// module's own context (the default `ctx` argument to
    /// `canonicalise_type`).
    dep_scope_types: Option<BTreeMap<Symbol, Vec<Symbol>>>,
    /// When this alias was injected from a dep module, the dep module's
    /// complete alias scope at the time of canonicalisation.  Paired with
    /// `dep_scope_types` so that when we build the `alt_ctx` for body
    /// expansion, we can also merge in the dep's aliases — making alias-typed
    /// fields (e.g. `board : Dict Int Piece` where `Piece` is a record alias
    /// from `Chess.Piece`) visible without the importing module having to
    /// re-import those transitive aliases itself.
    dep_scope_aliases: Option<BTreeMap<Symbol, crate::ExportedAlias>>,
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
    /// Maps each import qualifier (the short name used in `Q.TypeName` type
    /// annotations) to the full dep-module path.  Built from the module's
    /// `import` declarations in [`canonicalise_module_with_origin`].
    ///
    /// When a `TType(qualifier, segments, args)` node has a non-empty qualifier,
    /// this map is consulted FIRST.  On a hit, the dep path becomes the `home`
    /// for the resulting `Con` node — so `Counter.Msg` correctly gets
    /// `home = ["Counter"]` regardless of whether the current module also
    /// defines a local `type Msg`.  On a miss (kernel / stdlib qualifier) the
    /// lookup falls back to `type_home_map` as before.
    qualifier_paths: &'a BTreeMap<Symbol, Vec<Symbol>>,
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
    // Register `import Sky.… as Alias` / `import Std.… as Alias` qualifiers.
    // The single-module path does no dep injection, but stdlib qualifier
    // aliases must still resolve (`import Sky.Core.Json.Encode as Encode` →
    // `Encode.string`), so run the same registration the multi-module path uses.
    register_stdlib_import_aliases(&m.imports, &mut env, interner)?;
    // type_home_map and extra_aliases both start empty; canonicalise_with_env
    // populates type_home_map from this module's unions and merges extra_aliases
    // (empty here) with the module's own aliases.
    let mut type_home_map: BTreeMap<Symbol, Vec<Symbol>> = BTreeMap::new();
    let extra_aliases: BTreeMap<Symbol, AliasDef> = BTreeMap::new();
    // Single-module has no deps, so no user qualifiers to map — but a Html-family
    // STDLIB import qualifier (`import Std.Html.Attributes as Attr`) still needs
    // its `["Html"]` type home folded so a qualified `Attr.Attribute` lowers to
    // `html::Attribute` (#179 / #185).
    let mut qualifier_paths: BTreeMap<Symbol, Vec<Symbol>> = BTreeMap::new();
    fold_html_stdlib_qualifier_homes(&m.imports, &mut qualifier_paths, interner)?;
    // The bare single-module entry is always ordinary USER source: the trust tag
    // can only be raised via `canonicalise_module_with_origin`.
    // The single-module entry does not build a `ModuleExports`, so the kernel-
    // alias map is discarded — registration + def-skip already happened in `env`.
    canonicalise_with_env(
        m,
        &mut env,
        &mut type_home_map,
        &qualifier_paths,
        extra_aliases,
        ModuleOrigin::User,
        interner,
    )
    .map(|(canon_mod, _kernel_aliases)| canon_mod)
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
    // Thin wrapper: an unqualified call is ordinary USER source. The trust tag
    // can only be raised by a driver that explicitly reaches for
    // [`canonicalise_module_with_origin`] with the compiled-stdlib source it
    // embedded — never inferable from module text.
    canonicalise_module_with_origin(m, expected_path, deps, ModuleOrigin::User, interner)
}

/// Canonicalise a module carrying an explicit trust [`ModuleOrigin`].
///
/// Identical to [`canonicalise_module`] except that an [`ModuleOrigin::EmbeddedStdlib`]
/// module is (a) exempt from the SKY-N0025 reserved-namespace gate — it
/// legitimately declares `module Std.…` — and (b) required to carry a type
/// annotation on every top-level binding (fail-closed; see the gate at the end
/// of this function). A [`ModuleOrigin::User`] module is treated exactly as
/// before, so no user-facing behaviour changes on the default path.
///
/// # Errors
/// Same set as [`canonicalise_module`], plus a fail-closed
/// [`Diagnostic::CompilerBug`] when an [`ModuleOrigin::EmbeddedStdlib`] module has
/// an un-annotated top-level binding.
pub fn canonicalise_module_with_origin(
    m: &src::Module,
    expected_path: &[Symbol],
    deps: &BTreeMap<Vec<Symbol>, crate::ModuleExports>,
    origin: ModuleOrigin,
    interner: &mut Interner,
) -> DResult<(canon::Module, crate::ModuleExports)> {
    // Legacy entry point: dep exports arrive as an owned map and the
    // known-module universe for SKY-N0020 did-you-mean IS that map's key set —
    // the pre-incremental behaviour, preserved for non-driver callers.
    let dep_refs: BTreeMap<Vec<Symbol>, &crate::ModuleExports> =
        deps.iter().map(|(k, v)| (k.clone(), v)).collect();
    let known_modules: BTreeSet<Box<str>> = deps
        .keys()
        .map(|p| path_to_dot_string(interner, p))
        .collect();
    canonicalise_module_in_project(m, expected_path, &dep_refs, &known_modules, origin, interner)
}

/// Canonicalise a module against per-dep export references plus an explicit
/// known-module universe (the incremental driver's entry point).
///
/// Identical semantics to [`canonicalise_module_with_origin`] except:
/// * `deps` holds *references* to the importers' dep interfaces (the salsa
///   `module_interface` query memos) rather than owned clones, and is expected
///   to contain exactly this module's resolved imports — the only entries the
///   pre-incremental accumulated map ever observably consulted;
/// * `known_modules` (dot-joined module paths) supplies the SKY-N0020
///   did-you-mean candidate list, decoupling the *suggestion universe* (all
///   modules in the project) from the *injection map* (this module's imports).
///   Strings only — suggestion building must never intern (interning here
///   would perturb build-wide symbol numbering).
///
/// # Errors
/// Same set as [`canonicalise_module_with_origin`].
#[allow(clippy::too_many_lines)] // qualifier_paths pass added ~20 lines; refactor tracked in #todo
pub fn canonicalise_module_in_project(
    m: &src::Module,
    expected_path: &[Symbol],
    deps: &BTreeMap<Vec<Symbol>, &crate::ModuleExports>,
    known_modules: &BTreeSet<Box<str>>,
    origin: ModuleOrigin,
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
    // they never shadow prelude symbols downstream. An EmbeddedStdlib module is
    // the ONE legitimate definer of a `Std.…` / `Sky.…` home, so it is exempt —
    // but ONLY because the driver vouched for its provenance (unforgeable tag),
    // never because the text says `module Std.…`.
    let sky_sym = interner.intern("Sky")?;
    let std_sym = interner.intern("Std")?;
    if origin == ModuleOrigin::User
        && home
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
    // Register user import aliases for stdlib (`Sky.*` / `Std.*`) modules BEFORE
    // the dep-injection loop below. The loop bare-`continue`s for stdlib imports
    // (they need no dep injection), so alias registration is a separate,
    // self-contained pass keyed off the same authoritative path table.
    register_stdlib_import_aliases(&m.imports, &mut env, interner)?;
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
        // SKY-kernel vs compiled-source discrimination (fail-closed).
        //
        // A `Sky.*` / `Std.*` import is EITHER a kernel module whose qualifiers
        // are pre-installed by `Env::initial` (absent from the user `deps` map —
        // a `deps.get` on it would spuriously SKY-N0020 every importer of
        // `Sky.Core.Prelude`) OR a compiled-source stdlib module the build driver
        // injected into `deps` (e.g. `Std.Palette` / `Std.Css`). The former stays
        // on the qualifier-only `continue` path; the latter falls through to the
        // ordinary `deps.get` + `inject_dep_exports`, resolving byte-identically
        // to a user dependency. Presence in `deps` is the single discriminator:
        // a genuine kernel is never in `deps`, a compiled-source module always is.
        if dep_path
            .first()
            .copied()
            .is_some_and(|s| s == sky_sym || s == std_sym)
            && !deps.contains_key(dep_path)
        {
            continue;
        }
        // SKY-N0020: dep module must have been discovered + canonicalised before
        // this module in topological order.
        let dep = *deps.get(dep_path).ok_or_else(|| {
            let name = path_to_dot_string(interner, dep_path);
            // Offer did-you-mean over the caller-supplied known-module universe
            // (strings only — never intern on this path).
            let sugg: Box<[Box<str>]> = known_modules.iter().cloned().collect();
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

    // Build qualifier → dep-path map so `TType(qualifier, …)` annotations in
    // type sigs resolve `home` from the dep path, not from the unqualified
    // `type_home_map`.  Example: `import Counter` with no `exposing` clause
    // adds no entry to `type_home_map`, so without this map `Counter.Msg`
    // would look up "Msg" and find the LOCAL `type Msg` instead of Counter's.
    //
    // Qualifier = explicit `as Alias` if present, else last segment of the
    // module path — mirrors `inject_dep_exports`'s `env.qual_vars` logic.
    let mut qualifier_paths: BTreeMap<Symbol, Vec<Symbol>> = BTreeMap::new();
    // AUD-14: track each qualifier's FIRST-seen import span so a genuine clash
    // (below) can point back to it, mirroring `DuplicateValue`/`DuplicateType`'s
    // `first` field convention.
    let mut qualifier_first_span: BTreeMap<Symbol, Span> = BTreeMap::new();
    for import in &m.imports {
        let dep_path = &import.name.value;
        // Skip stdlib kernel imports (not in `deps`).
        if dep_path
            .first()
            .copied()
            .is_some_and(|s| s == sky_sym || s == std_sym)
            && !deps.contains_key(dep_path)
        {
            continue;
        }
        let qualifier = import
            .alias
            .unwrap_or_else(|| dep_path.last().copied().unwrap_or_else(name_zero));
        // AUD-14: `import App.Utils` + `import Lib.Utils` (both default to the
        // qualifier `Utils`), or an explicit `as` alias reused across two
        // distinct dep modules, previously overwrote silently here — every
        // `Utils.format` call downstream then resolved to whichever import
        // came LAST in source order, with no diagnostic. Re-importing the
        // SAME dep module under the same qualifier (a diamond dependency)
        // stays a no-op, matching `inject_dep_type`'s identical-re-injection
        // rule; only a clash between two DIFFERENT dep modules is rejected.
        if let Some(existing_path) = qualifier_paths.get(&qualifier) {
            if existing_path != dep_path {
                let qualifier_s = name_str(interner, qualifier)?;
                let first = qualifier_first_span
                    .get(&qualifier)
                    .copied()
                    .unwrap_or(import.name.span);
                return Err(Diagnostic::Name {
                    span: import.name.span,
                    msg: NameError::DuplicateQualifier {
                        qualifier: qualifier_s,
                        first,
                    },
                });
            }
            continue;
        }
        qualifier_paths.insert(qualifier, dep_path.clone());
        qualifier_first_span.insert(qualifier, import.name.span);
    }

    // Fold Html-family STDLIB import qualifiers into `qualifier_paths` (→
    // `["Html"]`) so a qualified `Attr.Attribute` (`import Std.Html.Attributes as
    // Attr`) resolves to the `html::Attribute` home (#179 / #185). Runs AFTER the
    // user-dep loop so a user qualifier that also names a Html dep keeps its real
    // dep path (`entry(..).or_insert` inside the helper is a no-op on a hit).
    fold_html_stdlib_qualifier_homes(&m.imports, &mut qualifier_paths, interner)?;

    // Snapshot injected aliases (params + body only) so we can include them in
    // `scope_aliases` after `injected_aliases` is moved into `canonicalise_with_env`.
    let injected_alias_snapshot: Vec<(Symbol, crate::ExportedAlias)> = injected_aliases
        .iter()
        .map(|(&name, def)| {
            (name, crate::ExportedAlias { params: def.params.clone(), body: def.body.clone() })
        })
        .collect();

    let (canon_mod, kernel_aliases) =
        canonicalise_with_env(
            m,
            &mut env,
            &mut type_home_map,
            &qualifier_paths,
            injected_aliases,
            origin,
            interner,
        )?;
    // The record-alias auto-constructors that were actually synthesized (#82):
    // exactly the defs whose name is an alias name. A function-field alias is
    // gated out of synthesis, so it is absent here and must NOT be exported as a
    // value — deriving the set from the real defs keeps exports and synthesis in
    // lockstep (no re-derivation that could drift).
    let own_alias_names: BTreeSet<Symbol> =
        m.aliases.iter().map(|a| a.value.name.value).collect();
    let synth_ctor_names: BTreeSet<Symbol> = canon_mod
        .defs
        .iter()
        .map(|d| d.name().value)
        .filter(|n| own_alias_names.contains(n))
        .collect();
    // Fail-closed stdlib annotation gate (design §3). Whole-program rank-based
    // let-generalisation is NOT implemented: an UN-annotated top-level binding
    // used at two distinct concrete types would unify under one mono var and
    // could mis-infer deep inside the stdlib. For an EmbeddedStdlib module we
    // therefore make the fully-annotated precondition a MACHINE-CHECKED contract
    // rather than an assumption — any `Def::Untyped` top-level is a
    // compiler-internal error at THIS boundary, turning a would-be confusing
    // deep-stdlib unification failure (or exit-0-then-cargo-fail) into an explicit
    // build-time invariant. It can never fire for user code (User origin skips
    // this block); synthesised record-alias ctors are `Def::Typed`, so they pass.
    if origin == ModuleOrigin::EmbeddedStdlib {
        for d in &canon_mod.defs {
            if let canon::Def::Untyped { name, .. } = d {
                let binding = name_str(interner, name.value)?;
                let module = path_to_dot_string(interner, &home);
                return Err(Diagnostic::CompilerBug {
                    where_: "canon.stdlib_unannotated",
                    detail: format!(
                        "compiled-source stdlib module `{module}` binding `{binding}` \
                         must carry a type annotation (annotation-driven generalisation \
                         is required for stdlib modules)"
                    ),
                });
            }
        }
    }

    // Build the export surface from the module's own `exposing (…)` clause.
    // Then record the full type scope so importers can use it when expanding
    // this module's alias bodies (see `AliasDef::dep_scope_types`).
    let mut exports = build_module_exports(&home, m, &env, &synth_ctor_names, &kernel_aliases);
    exports.scope_types = type_home_map;

    // Build the full alias scope: own local aliases + all injected dep aliases.
    // Importers use this via `AliasDef::dep_scope_aliases` when expanding alias
    // bodies that reference alias-typed fields from this module's dep scope
    // (e.g. `board : Dict Int Piece` where `Piece` is a record-alias from a
    // transitively imported module not directly imported by the importer).
    let mut scope_aliases: BTreeMap<Symbol, crate::ExportedAlias> = BTreeMap::new();
    for a in &m.aliases {
        scope_aliases.insert(
            a.value.name.value,
            crate::ExportedAlias {
                params: a.value.vars.iter().map(|v| v.value).collect(),
                body: a.value.body.value.clone(),
            },
        );
    }
    for (name, ea) in injected_alias_snapshot {
        scope_aliases.entry(name).or_insert(ea);
    }
    exports.scope_aliases = scope_aliases;

    Ok((canon_mod, exports))
}

/// Register user import aliases for stdlib (`Sky.*` / `Std.*`) modules.
///
/// For every stdlib import, resolve its full path to the canonical qualifier
/// (via [`Env::canonical_stdlib_qualifier`]) and register the user's *effective*
/// qualifier — the explicit `as Alias`, else the Elm last-segment default —
/// against the canonical qualifier's kernel members. This is what makes
/// `import Sky.Core.Json.Encode as Encode` register `Encode` → the `JsonEnc`
/// members, and `import Std.Ui as U` register `U` → the `Ui` members.
///
/// Idempotent when the effective qualifier already equals the canonical name
/// (the common `import Std.Log as Log` case — `Log` is already registered).
///
/// A path that names no known stdlib module is left unregistered (fail-closed,
/// per [`Env::canonical_stdlib_qualifier`]): any later `Alias.member` reference
/// surfaces the ordinary `UnknownModule` diagnostic at its use site rather than
/// resolving against an invented qualifier. This preserves the pre-existing
/// behaviour for as-yet-unported stdlib modules (e.g. `Sky.Core.ToString`).
///
/// # Errors
/// [`Diagnostic::CompilerBug`] if interning `Sky` / `Std` or a canonical name
/// exhausts the interner.
fn register_stdlib_import_aliases(
    imports: &[src::Import],
    env: &mut Env,
    interner: &mut Interner,
) -> DResult<()> {
    let sky_sym = interner.intern("Sky")?;
    let std_sym = interner.intern("Std")?;
    for import in imports {
        let dep_path = &import.name.value;
        // Only `Sky.*` / `Std.*` imports name compiler stdlib modules.
        if !dep_path
            .first()
            .copied()
            .is_some_and(|s| s == sky_sym || s == std_sym)
        {
            continue;
        }
        let Some(canonical) = env.canonical_stdlib_qualifier(dep_path, interner)? else {
            // Unknown stdlib path: register nothing (fail-closed).
            continue;
        };
        // Elm convention: an explicit `as Alias` names the qualifier, otherwise
        // the module is exposed under the LAST path segment.
        let alias = import
            .alias
            .unwrap_or_else(|| dep_path.last().copied().unwrap_or_else(name_zero));
        if alias == canonical {
            // Already registered under its canonical name — nothing to clone.
            continue;
        }
        // Clone the canonical qualifier's members under the alias key. The cloned
        // `VarHome::Kernel` entries carry the CANONICAL module + name symbols, so
        // a later `Alias.member` resolves to the same `VarKernel` a canonical
        // reference would (the lowerer's kernel match arms are unaffected).
        if let Some(members) = env.qual_vars.get(&canonical).cloned() {
            std::rc::Rc::make_mut(&mut env.qual_vars).entry(alias).or_default().extend(members);
        }
    }
    Ok(())
}

/// Bring the stdlib VALUE members named in an explicit
/// `import Sky.*/Std.* exposing (n1, n2, …)` list into UNQUALIFIED scope (#97).
///
/// `import M exposing (member)` for a stdlib module previously registered
/// nothing — stdlib imports are skipped by the dep-injection loop
/// ([`inject_dep_exports`] never runs for them), so a bare `member` reference
/// fell through to `SKY-N0001`. This is the exposing-list counterpart of #78's
/// alias registration ([`register_stdlib_import_aliases`]): it reuses the same
/// authoritative path→qualifier table via [`Env::canonical_stdlib_qualifier`].
///
/// Scope and soundness:
///
/// * Only **VALUE** exposures ([`src::Exposed::Value`], lowercase names) are
///   handled. Capitalized **TYPE** exposures (`exposing (Element)`,
///   `exposing (Error)`) are kernel-implicit Prelude types resolved by a
///   separate mechanism and are deliberately left untouched — treating them as
///   value members would spuriously reject every `exposing (SomeType)`.
/// * The registered [`VarHome::Kernel`] is **cloned verbatim** from the
///   canonical qualifier's member table, so the cloned entry carries the SAME
///   kernel id + canonical module + name a qualified `M.member` reference
///   resolves to. Lowering is therefore identical whether the call site is
///   qualified or unqualified (exactly like #78's alias clones).
/// * **Fail-closed.** A lowercase exposed name that is NOT a real value member
///   of the module yields [`NameError::NameNotExposed`] with a did-you-mean over
///   the module's actual members — never a silently invented unqualified
///   binding. A path that names no known/ported stdlib module registers nothing
///   (matching [`register_stdlib_import_aliases`]); a later unqualified use
///   surfaces the ordinary `SKY-N0001` at its use site.
/// * Exposed names fold into `seen_values`, so a user top-level value (or a
///   synth record-alias ctor) of the same name surfaces
///   [`NameError::DuplicateValue`] rather than silently shadowing — matching the
///   Elm rule that explicitly importing a name and defining it locally is a
///   conflict.
///
/// `exposing (..)` (open import) on a stdlib module is intentionally a **no-op**
/// here: open imports have low priority (a local definition shadows them without
/// error), which is a different, non-strict insertion discipline than the
/// explicit list above. Flooding every member into `seen_values` would wrongly
/// turn a legal local shadow of a Prelude name (e.g. a user `map`) into a
/// `DuplicateValue` and regress the corpus. The common wildcard case
/// (`import Sky.Core.Prelude exposing (..)`) already works via the pre-installed
/// Prelude builtins + qualified access; correct open-import member flooding is
/// filed as follow-up #98.
///
/// # Errors
/// [`NameError::NameNotExposed`] / [`NameError::DuplicateValue`]; or
/// [`Diagnostic::CompilerBug`] if interning `Sky` / `Std` / a name exhausts the
/// interner.
fn inject_stdlib_exposed_values(
    m: &src::Module,
    env: &mut Env,
    seen_values: &mut BTreeMap<Symbol, Span>,
    interner: &mut Interner,
) -> DResult<()> {
    let sky_sym = interner.intern("Sky")?;
    let std_sym = interner.intern("Std")?;
    for import in &m.imports {
        let dep_path = &import.name.value;
        // Only `Sky.*` / `Std.*` imports name compiler stdlib modules.
        if !dep_path
            .first()
            .copied()
            .is_some_and(|s| s == sky_sym || s == std_sym)
        {
            continue;
        }
        // Open imports (`exposing (..)`) are a no-op — see the doc comment.
        let src::Exposing::List(items) = &import.exposing.value else {
            continue;
        };
        let Some(canonical) = env.canonical_stdlib_qualifier(dep_path, interner)? else {
            // Unknown / unported stdlib path: register nothing (fail-closed).
            continue;
        };
        for item in items {
            // Only VALUE members are brought unqualified; TYPE exposures resolve
            // through the kernel-implicit type mechanism and are left as-is.
            let src::Exposed::Value(name) = &item.value else {
                continue;
            };
            let name = *name;
            // Resolve against the canonical qualifier's member table, cloning the
            // kernel home out so the immutable borrow ends before the mutation.
            let member = env
                .qual_vars
                .get(&canonical)
                .and_then(|members| members.get(&name))
                .cloned();
            let Some(home) = member else {
                // Fail-closed: not a real value member of this module.
                let name_s = name_str(interner, name)?;
                let module_s = path_to_dot_string(interner, dep_path);
                let candidates: Vec<Symbol> = env
                    .qual_vars
                    .get(&canonical)
                    .map(|members| members.keys().copied().collect())
                    .unwrap_or_default();
                let sugg = suggestions(name, candidates.into_iter(), interner);
                return Err(Diagnostic::Name {
                    span: item.span,
                    msg: NameError::NameNotExposed {
                        module: module_s,
                        name: name_s,
                        suggestions: sugg,
                    },
                });
            };
            // Fold into the value namespace with DuplicateValue detection.
            if let Some(&first) = seen_values.get(&name) {
                return Err(Diagnostic::Name {
                    span: item.span,
                    msg: NameError::DuplicateValue {
                        name: name_str(interner, name)?,
                        first,
                    },
                });
            }
            seen_values.insert(name, item.span);
            env.vars.insert(name, home);
        }
    }
    Ok(())
}

/// Builtin type names whose lowering (`sky_lower::ir_type_from_{ty,canon}`)
/// disambiguates by the `home` path — `Attribute` exists in BOTH `Std.Ui`
/// (→ `sky_runtime::ui::element::Attribute`) and `Std.Html.Attributes`
/// (→ `sky_runtime::html::Attribute`), and the lowerer's `is_html = home
/// contains "Html"` check drives the choice. `Event` is listed for the same
/// home-driven precedent (currently single-ctor `html::Event`, so harmless).
/// Every OTHER builtin type is home-insensitive at lowering (its named arm fires
/// on the name string regardless of `home`).
const HOME_SENSITIVE_BUILTIN_TYPES: &[&str] = &["Attribute", "Event"];

/// Record the canonical `["Html"]` `home` for a home-sensitive builtin TYPE
/// (`Attribute` / `Event`) brought UNQUALIFIED into scope via an explicit
/// `import <Html-family> exposing (Attribute, …)` list (#179) — the TYPE
/// counterpart of [`inject_stdlib_exposed_values`] (which handles only lowercase
/// VALUE members).
///
/// ## Why
///
/// `Attribute` exists in BOTH `Std.Ui` (→ `ui::element::Attribute`) and
/// `Std.Html.Attributes` (→ `html::Attribute`); the lowerer disambiguates by
/// `is_html = home contains "Html"`. A bare `Attribute` exposed from a stdlib
/// Html module previously reached the empty-home sentinel (stdlib imports are
/// skipped by the dep-injection loop), so `is_html` always failed and the
/// `Std.Live.Head.pairToAttr : (String,String) -> Attribute msg` shape
/// mis-lowered to `ui::element::Attribute` while its `Attr.attribute` body
/// produced `html::Attribute` — an exit-0-then-cargo-fail E0308 SEAL violation.
/// The bare path resolves via `resolve_unqualified_type_home`, which consults
/// `type_home_map` first, so recording `["Html"]` here fixes it.
///
/// ## Why `["Html"]` and not the full dep path
///
/// The HM constrainer builds the Std.Html attribute type as `Ty::Con { module:
/// ["Html"], name: Attribute }` (`constrain.rs` `html_attr`). Recording the full
/// `["Std","Html","Attributes"]` would mint a NOMINALLY-DISTINCT `Attribute` that
/// fails to unify with `Html.node`'s parameter (`expected Html.Attribute, found
/// Std.Html.Attributes.Attribute`). `["Html"]` matches the constrainer AND
/// satisfies the lowerer's `is_html` check.
///
/// ## Scope and soundness
///
/// * Only names in [`HOME_SENSITIVE_BUILTIN_TYPES`] are touched — a no-op for
///   every home-insensitive builtin.
/// * The QUALIFIED `Attr.Attribute` / `Ui.Attribute` case is handled separately
///   by [`fold_html_stdlib_qualifier_homes`] (keyed on the QUALIFIER, so the two
///   stay distinct); this pass must touch ONLY names exposed UNQUALIFIED, or it
///   would also hijack a qualified `Ui.Attribute` in the same module (whose
///   fallback also reads `type_home_map`) — the exact regression in
///   `26-ui-showcase/RegressionGates.testId : Ui.Attribute msg`.
/// * **Conflict guard.** If the SAME module ALSO brings the Ui `Attribute` type
///   into UNQUALIFIED scope (`import Std.Ui … exposing (Attribute)` or `exposing
///   (..)`), a bare `Attribute` is genuinely ambiguous, so we record NOTHING and
///   leave the sentinel rather than silently pinning one home.
/// * Runs BEFORE any value body is canonicalised.
/// * A later LOCAL `type Attribute` is already rejected by
///   `reject_reserved_builtin_type` (SKY-N0026); re-inserting the same `["Html"]`
///   is idempotent (`entry(..).or_insert`).
fn inject_stdlib_exposed_type_homes(
    m: &src::Module,
    type_home_map: &mut BTreeMap<Symbol, Vec<Symbol>>,
    interner: &mut Interner,
) -> DResult<()> {
    let html_sym = interner.intern("Html")?;

    // Does this module bring the Ui `Attribute` TYPE into UNQUALIFIED scope?
    let ui_exposes_attribute = m.imports.iter().any(|import| {
        let dep = &import.name.value;
        // Exactly `Std.Ui` (owner of the Ui `Attribute`), not a `Std.Ui.*`
        // sub-module (which does not re-export it).
        let is_std_ui = dep.len() == 2
            && dep
                .first()
                .copied()
                .is_some_and(|s| interner.resolve(s) == Some("Std"))
            && dep
                .get(1)
                .copied()
                .is_some_and(|s| interner.resolve(s) == Some("Ui"));
        if !is_std_ui {
            return false;
        }
        match &import.exposing.value {
            src::Exposing::All => true,
            src::Exposing::List(items) => items.iter().any(|item| {
                matches!(&item.value, src::Exposed::Type(n, _)
                    if interner.resolve(*n) == Some("Attribute"))
            }),
        }
    });
    if ui_exposes_attribute {
        return Ok(());
    }

    for import in &m.imports {
        let src::Exposing::List(items) = &import.exposing.value else {
            continue;
        };
        let is_html_family = import
            .name
            .value
            .iter()
            .any(|s| interner.resolve(*s) == Some("Html"));
        if !is_html_family {
            continue;
        }
        for item in items {
            let src::Exposed::Type(type_name, _privacy) = &item.value else {
                continue;
            };
            let Some(resolved) = interner.resolve(*type_name) else {
                continue;
            };
            if !HOME_SENSITIVE_BUILTIN_TYPES.contains(&resolved) {
                continue;
            }
            type_home_map
                .entry(*type_name)
                .or_insert_with(|| vec![html_sym]);
        }
    }
    Ok(())
}

/// Fold each Html-family STDLIB import qualifier into `qualifier_paths`, mapping
/// it to the canonical `["Html"]` type home (#179 / #185).
///
/// Stdlib kernel imports are skipped by the ordinary `qualifier_paths`
/// construction (they carry no `deps` entry), so a QUALIFIED `Attr.Attribute`
/// (from `import Std.Html.Attributes as Attr`) used to fall through to the
/// by-name `type_home_map.get("Attribute")` — a single entry that cannot tell
/// `Attr.Attribute` (Html) from `Ui.Attribute` (Ui), so both mis-lowered to the
/// same newtype. Registering the qualifier here — `Attr` (and the canonical
/// `Html`) → `["Html"]` — makes the `TType` arm resolve `Attr.Attribute` to the
/// `html::Attribute` home directly, while `Ui.Attribute` (Ui-family qualifier,
/// deliberately NOT folded) keeps falling through to the empty Ui sentinel.
///
/// The `["Html"]` value is the SAME single-segment home the HM constrainer uses
/// for the Std.Html attribute type (`constrain.rs` `html_attr`), so the emitted
/// type unifies with `Html.node`'s parameter rather than minting a distinct
/// `Std.Html.Attributes.Attribute`.
///
/// A qualifier the user has ALSO bound to a real dep module keeps its dep path
/// (`entry(..).or_insert` — the dep-path insertion runs first and wins).
///
/// # Errors
/// [`Diagnostic::CompilerBug`] if interning `Html` exhausts the interner.
fn fold_html_stdlib_qualifier_homes(
    imports: &[src::Import],
    qualifier_paths: &mut BTreeMap<Symbol, Vec<Symbol>>,
    interner: &mut Interner,
) -> DResult<()> {
    let html_sym = interner.intern("Html")?;
    for import in imports {
        let dep_path = &import.name.value;
        let is_html_family = dep_path
            .iter()
            .any(|s| interner.resolve(*s) == Some("Html"));
        if !is_html_family {
            continue;
        }
        // Effective qualifier: explicit `as Alias`, else the Elm last-segment
        // default (`import Std.Html.Attributes` → `Attributes`).
        let qualifier = import
            .alias
            .unwrap_or_else(|| dep_path.last().copied().unwrap_or_else(name_zero));
        qualifier_paths
            .entry(qualifier)
            .or_insert_with(|| vec![html_sym]);
    }
    Ok(())
}

/// Flood EVERY value member of a stdlib module named in
/// `import Sky.*/Std.* exposing (..)` into the LOW-PRIORITY wildcard tier
/// ([`Env::wildcard_vars`]) so bare `div` / `text` / … resolve unqualified (#98).
///
/// This is the open-import counterpart of [`inject_stdlib_exposed_values`]. The
/// two paths differ ONLY in insertion discipline, and the difference is the whole
/// point of the wildcard/explicit distinction:
///
/// * **Explicit `exposing (name)`** folds into `seen_values` + `env.vars` — a
///   local of the same name is a hard [`NameError::DuplicateValue`] ("I demand
///   this name"), and the name resolves at the same priority as a top-level
///   binding.
/// * **Wildcard `exposing (..)`** inserts into `env.wildcard_vars` ONLY — never
///   into `seen_values`, so a local / explicit-exposed / synth-ctor / prelude
///   name of the same spelling SILENTLY shadows it at resolve time ("fill in the
///   rest"). [`resolve_var`] consults this tier last.
///
/// Ambiguity is NOT decided here: two wildcards exposing the same name are both
/// legal at import time. The clash is recorded (both origins kept, keyed by
/// canonical qualifier) and only surfaces as [`NameError::AmbiguousImport`] if a
/// bare use of the name actually occurs and no higher-priority binding shadows it
/// — matching the deferred-conflict rule Elm applies to open imports.
///
/// Soundness: each cloned [`VarHome::Kernel`] carries the canonical module + name
/// (and kernel id) of the qualified member, so an unqualified wildcard reference
/// lowers byte-identically to a qualified `M.member` reference. Keying by the
/// canonical qualifier symbol means importing the same module twice (or once
/// under an alias) collapses to a single origin — never a spurious self-ambiguity.
///
/// A path that names no known/ported stdlib module floods nothing (fail-closed,
/// via [`Env::canonical_stdlib_qualifier`]); a later bare use surfaces the
/// ordinary `SKY-N0001` at its use site.
///
/// # Errors
/// [`Diagnostic::CompilerBug`] if interning `Sky` / `Std` exhausts the interner.
fn inject_stdlib_wildcard_values(
    m: &src::Module,
    env: &mut Env,
    interner: &mut Interner,
) -> DResult<()> {
    let sky_sym = interner.intern("Sky")?;
    let std_sym = interner.intern("Std")?;
    for import in &m.imports {
        let dep_path = &import.name.value;
        // Only `Sky.*` / `Std.*` imports name compiler stdlib modules.
        if !dep_path
            .first()
            .copied()
            .is_some_and(|s| s == sky_sym || s == std_sym)
        {
            continue;
        }
        // Only open imports (`exposing (..)`) flood the wildcard tier; the
        // explicit-list case is handled by `inject_stdlib_exposed_values`.
        if !matches!(&import.exposing.value, src::Exposing::All) {
            continue;
        }
        let Some(canonical) = env.canonical_stdlib_qualifier(dep_path, interner)? else {
            // Unknown / unported stdlib path: flood nothing (fail-closed).
            continue;
        };
        // Snapshot the canonical qualifier's members (clone out so the immutable
        // borrow of `env.qual_vars` ends before we mutate `env.wildcard_vars`).
        let Some(members) = env.qual_vars.get(&canonical).cloned() else {
            continue;
        };
        let dep_owned = dep_path.clone();
        for (name, home) in members {
            // Dedup by canonical qualifier: a second import of the SAME module
            // (or an aliased re-import) overwrites its own prior origin rather
            // than registering a phantom second candidate that would fake an
            // ambiguity with itself.
            std::rc::Rc::make_mut(&mut env.wildcard_vars).entry(name).or_default().insert(
                canonical,
                WildcardOrigin {
                    home,
                    dep_path: dep_owned.clone(),
                },
            );
        }
    }
    Ok(())
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
// A linear resolution pipeline (register types → collect aliases → synthesize
// record-alias ctors → register values → canonicalise bodies); splitting the
// stages into separate functions would thread the same six mutable maps through
// each and obscure the ordering the stages depend on.
#[allow(clippy::too_many_lines)]
fn canonicalise_with_env(
    m: &src::Module,
    env: &mut Env,
    type_home_map: &mut BTreeMap<Symbol, Vec<Symbol>>,
    qualifier_paths: &BTreeMap<Symbol, Vec<Symbol>>,
    extra_aliases: BTreeMap<Symbol, AliasDef>,
    origin: ModuleOrigin,
    interner: &mut Interner,
) -> DResult<(canon::Module, BTreeMap<Symbol, KernelAlias>)> {
    let home = env.home.clone();

    // #102: a LOCAL type/alias declaration whose name already has a
    // `type_home_map` entry from a DIFFERENT home is a dep-imported type being
    // shadowed. Reject it here, at the declaration, with the SAME
    // `NameError::DuplicateType` (SKY-N0012) `inject_dep_type` already uses for a
    // dep-vs-dep clash — closing the asymmetry where THAT clash was caught
    // cleanly but this one silently mis-registered the environment (the local
    // ctors won, but the type-home map kept pointing at the dep's home),
    // surfacing three functions later as an unrelated SKY-T0001 type mismatch
    // (docs/adr/0010-pattern-and-lowering-completeness.md, item D).
    //
    // This standalone pre-pass READS `type_home_map` but does NOT yet write this
    // module's own entries — the two loops below still own that. Running it first
    // (unions before aliases, both before the `entry/or_insert` loop mutates the
    // map) avoids a two-declarations-in-one-module case spuriously seeing its own
    // freshly-inserted entry and misreporting a shadow. Same-module duplicates
    // (`type X = A; type X = B` in ONE module) are still caught — with a better,
    // first-declared span — by the `seen_types` loops below, which run after this
    // pre-pass has already ruled out the dep-shadow case.
    for u in &m.unions {
        let type_name = u.value.name.value;
        if let Some(existing) = type_home_map.get(&type_name)
            && existing.as_slice() != home.as_slice()
        {
            return Err(Diagnostic::Name {
                span: u.value.name.span,
                msg: NameError::DuplicateType {
                    name: name_str(interner, type_name)?,
                    // No source span survives from the dep side here — matches
                    // `inject_dep_type`'s established convention for this clash.
                    first: Span::DUMMY,
                },
            });
        }
    }
    for a in &m.aliases {
        let alias_name = a.value.name.value;
        if let Some(existing) = type_home_map.get(&alias_name)
            && existing.as_slice() != home.as_slice()
        {
            return Err(Diagnostic::Name {
                span: a.value.name.span,
                msg: NameError::DuplicateType {
                    name: name_str(interner, alias_name)?,
                    first: Span::DUMMY,
                },
            });
        }
    }

    // Add this module's own types to the type-home map. Dep-imported types were
    // already added by inject_dep_exports before this call; the dep-shadow
    // pre-pass above has already rejected any local type whose name clashes with
    // a DIFFERENT home, so `entry/or_insert` here only ever inserts a genuinely
    // local (or same-home) type.
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
        // SKY-N0026: reject a user type whose name shadows a reserved built-in
        // before it can silently override the lowerer's builtin-name mapping.
        reject_reserved_builtin_type(type_name, type_span, origin, interner)?;
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
        // SKY-N0026: `type alias` names are gated the same as `type` names — an
        // alias shadowing a built-in would be silently overridden too.
        reject_reserved_builtin_type(alias_name, alias_span, origin, interner)?;
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
                dep_scope_types: None,
                dep_scope_aliases: None,
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
            qualifier_paths,
            &aliases,
            interner,
        )?);
    }

    // Synthesize a value-level auto-constructor for every local record type
    // alias (SKY-N0001 / #82). Built here — the single site where each alias's
    // source-order fields are known — as an ordinary typed `Def`, so no later
    // stage special-cases it. Must run before the value pre-pass so the ctor
    // names participate in `seen_values` (a user value colliding with a ctor
    // name surfaces `DuplicateValue`, never a silent skip).
    let synth_ctor_defs = synthesize_record_alias_ctors(
        m,
        &home,
        env,
        type_home_map,
        qualifier_paths,
        &aliases,
        &seen_ctors,
        interner,
    )?;

    // Register every top-level value name so bindings can be referenced before
    // their definition (mutual / forward references), rejecting duplicates.
    // Seed with the synthesized record-alias constructors first so their names
    // are reserved in the value namespace.
    let mut seen_values: BTreeMap<Symbol, Span> = BTreeMap::new();
    for d in &synth_ctor_defs {
        let name = d.name();
        seen_values.insert(name.value, name.span);
        env.vars.insert(name.value, VarHome::TopLevel(home.clone()));
    }
    // Bring stdlib VALUE members named in an explicit `import Sky.*/Std.* exposing
    // (name, …)` list into UNQUALIFIED scope (#97). Runs after the synth-ctor
    // seeding and before the local-value pre-pass so an exposed name and a local
    // of the same name collide as `DuplicateValue` (mirrors #82's ctor-collision
    // rule). Fail-closed: a lowercase name that is not a real value member of the
    // module surfaces `NameNotExposed`, never a dangling binding.
    inject_stdlib_exposed_values(m, env, &mut seen_values, interner)?;
    // #179: record the `["Html"]` origin home for a home-sensitive builtin TYPE
    // brought UNQUALIFIED via `import <Html-family> exposing (Attribute)`, so the
    // bare `Attribute msg` annotation lowers to the SAME newtype (`html::Attribute`
    // vs `ui::element::Attribute`) its body produces. Runs before any value body
    // is canonicalised so `resolve_unqualified_type_home` (which consults
    // `type_home_map` first) sees the recorded home rather than the empty-home
    // sentinel that always mis-selects `UiAttribute`.
    inject_stdlib_exposed_type_homes(m, type_home_map, interner)?;
    // #98: flood every member of an `import Sky.*/Std.* exposing (..)` stdlib
    // module into the LOW-PRIORITY wildcard tier. Deliberately does NOT touch
    // `seen_values` — a local / explicit-exposed / synth-ctor / prelude name of
    // the same spelling silently shadows a wildcard member (see the fn doc);
    // cross-wildcard clashes surface only at an ambiguous use site.
    inject_stdlib_wildcard_values(m, env, interner)?;
    // Stage-4 kernel aliases discovered in this module: `f = Ffi.kernel "K_n"`.
    // Each is registered as a `VarHome::Kernel` (so every reference — in-module
    // `f` or cross-module `Alias.f` — routes straight to the kernel) and its
    // body is NOT canonicalised into a top-level def (the alias emits no runtime
    // function; it IS the kernel). The map is returned so the project entry can
    // record it on `ModuleExports.kernel_aliases`.
    let mut kernel_aliases: BTreeMap<Symbol, KernelAlias> = BTreeMap::new();
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
        // FAIL-CLOSED (THE SEAL): `detect_kernel_alias` errors when the binding
        // is a kernel alias naming an unregistered kernel — never a silent
        // TopLevel fall-through that would emit a dangling call.
        if let Some(alias) = detect_kernel_alias(&v.value, env, interner)? {
            env.vars
                .insert(name, VarHome::Kernel(Some(alias.id), alias.module, alias.function));
            kernel_aliases.insert(name, alias);
        } else {
            env.vars.insert(name, VarHome::TopLevel(home.clone()));
        }
    }

    // Canonicalise each value declaration. A kernel alias has no runtime body —
    // it lowers as its kernel at every call site — so it is skipped here, exactly
    // as a kernel-qualifier member is never a compiled def.
    let mut defs = Vec::with_capacity(m.values.len() + synth_ctor_defs.len());
    for v in &m.values {
        if kernel_aliases.contains_key(&v.value.name.value) {
            continue;
        }
        defs.push(canonicalise_value(
            &v.value,
            env,
            type_home_map,
            qualifier_paths,
            &aliases,
            interner,
        )?);
    }
    // The synthesized constructor defs are already fully canonical.
    defs.extend(synth_ctor_defs);

    Ok((
        canon::Module {
            name: home,
            unions,
            defs,
        },
        kernel_aliases,
    ))
}

/// Synthesize the value-level auto-constructor for every LOCAL `type alias`
/// whose declaration body is a *literal* record (SKY-N0001 / task #82).
///
/// For `type alias T p0..pk = { f0 : A0, …, fN : AN }` this produces the
/// ordinary typed binding
///
/// ```text
/// T : ∀ (used-of p0..pk). A0 -> … -> AN -> { f0:A0, …, fN:AN }
/// T f0 … fN = { f0 = f0, …, fN = fN }
/// ```
///
/// materialised as a [`canon::Def::Typed`] indistinguishable from a hand-written
/// function — every downstream stage (HM, lowering, backend) needs no
/// special-casing and no new IR node. Field order is captured **once** from the
/// source `TRecord` vec and projected into the parameter patterns, the body
/// record literal, and the arrow argument types from a **single** iteration, so
/// positional argument `i` provably binds field `f_i` (there is no structure in
/// which the two orders can disagree).
///
/// Gating (PARSE, DON'T VALIDATE):
/// * Only a **literal** `TRecord` body qualifies. A head alias to a record
///   alias (`type alias U = T`) gets **no** constructor — matching Elm — because
///   its source body is a `TType`, not a `TRecord`.
/// * A non-record alias (`type alias Count = Int`) gets no binding, so using it
///   as a value stays an ordinary `SKY-N0001` name error.
///
/// The result is a **closed** record: this compiler has no row variable, so a
/// missing / extra / mis-typed field is a compile error, never silent
/// acceptance — the constructor opens no row-poly surface.
///
/// # Errors
/// [`NameError::DuplicateValue`] when the alias name already names a data
/// constructor in scope (the value-namespace usage of that name is already
/// taken — rejecting the silent constructor-wins shadow, Elm-faithful).
/// [`Diagnostic::Name`] ([`NameError::AliasArity`] / [`NameError::UnknownModule`])
/// propagated from canonicalising a field type; [`Diagnostic::CompilerBug`] on
/// an un-interned symbol.
/// Built-in opaque boxed-wrapper type constructors — `Decoder` / `Task` / `Cmd`
/// / `Sub`. Each lowers to a runtime type that boxes its payload behind a trait
/// object and derives nothing over that payload, so a function in its type
/// arguments is a legitimate value rather than a non-derivable-derive carrier.
///
/// Mirrors `sky_lower`'s `is_opaque_boxed_wrapper`
/// (`crates/sky_lower/src/lower.rs` L151) EXACTLY — the set MUST stay identical
/// so this canon-side synthesis gate and the lowerer's
/// `embeds_nonderivable_function` agree on which record-alias fields carry a
/// buildable constructor. Matched by name only, sound because these are
/// kernel-implicit Prelude type constructors a user program cannot redefine.
fn is_opaque_boxed_wrapper_canon(interner: &Interner, name: Symbol) -> bool {
    matches!(
        interner.resolve(name),
        Some("Decoder" | "Task" | "Cmd" | "Sub")
    )
}

/// Could this canonical field type NOT be a field of a
/// `#[derive(Clone, Debug, PartialEq)]` + `impl SkyStringify` struct — i.e. is it
/// non-derivable-as-a-struct-field?
///
/// A synthesised record-alias constructor's body is a record literal, so the
/// backend emits a `#[derive(Clone, Debug, PartialEq)]` struct (plus an
/// `impl SkyStringify`) over the field types. A field whose type satisfies none
/// of those obligations makes the emitted Rust fail to compile. Two disjoint
/// shapes are non-derivable-as-a-struct-field:
///
/// 1. A raw function (`Lambda`) — lowers to `Box<dyn Fn(...) + Send>`, which
///    is `!Clone`, `!Debug`, `!PartialEq`, `!SkyStringify`.
/// 2. An OPAQUE boxed-wrapper VALUE ([`is_opaque_boxed_wrapper_canon`] —
///    `Decoder` / `Task` / `Cmd` / `Sub`). Its runtime representation is a
///    `Box<dyn Fn>` (`Decoder`), a boxed-thunk enum (`SkyCmd` / `SkySub`), or a
///    `Pin<Box<dyn Future>>` (`SkyTask`) — none of which impl `Clone` / `Debug` /
///    `PartialEq` / `SkyStringify` over their payload.
///
/// # Why this predicate is NOT the lowerer's function-embedding one
///
/// This is deliberately DISTINCT from `sky_lower`'s `embeds_nonderivable_function`
/// (`crates/sky_lower/src/lower.rs` L183) — and from this crate's round-1 port
/// `canon_type_embeds_function`, now deleted. That predicate answers a
/// different question: "does a RAW FUNCTION appear anywhere inside this type",
/// and it EXEMPTS an opaque wrapper HEAD (returns `false` and does NOT recurse)
/// because a function nested inside a `Decoder` payload is boxed away — the
/// L0107 concern is a bare function reaching the derive, not a wrapper doing its
/// job.
///
/// That exemption is CORRECT for the lowerer's payload-scan but WRONG for the
/// struct-synthesis decision: an opaque wrapper in FIELD position is ITSELF the
/// non-derivable value, regardless of its payload. `{ dec : Decoder Int }`,
/// `{ cmd : Cmd Msg }` carry no raw function at all, yet the emitted
/// `#[derive(…)]` struct over `Decoder` / `SkyCmd` does not build (round-1
/// verified: skyc-0 then cargo-101 — the seal hole this fix closes). So here the
/// opaque head SHORT-CIRCUITS to `true` (the flip): the wrapper is non-derivable
/// as a struct field, so the alias must DECLINE synthesis and keep its exact
/// pre-#82 behaviour (no positional constructor). It loses zero capability — such
/// a record is unbuildable-as-a-struct at every real construction/use site
/// regardless — and turns the un-buildable constructor UNREPRESENTABLE at canon
/// rather than emitted-then-cargo-rejected.
///
/// [`is_opaque_boxed_wrapper_canon`]'s SET stays byte-identical to
/// `lower.rs` L151 (`Decoder` / `Task` / `Cmd` / `Sub`); only its ROLE here is
/// flipped (field-position opaque head ⇒ non-derivable, not exempt).
fn field_type_nonderivable(interner: &Interner, t: &canon::Type) -> bool {
    match t {
        canon::Type::Lambda(_, _) => true,
        canon::Type::Con { name, args, .. } => {
            // FLIP vs the lowerer's function-embedding predicate: an opaque
            // boxed-wrapper HEAD in field position is ITSELF non-derivable —
            // short-circuit to `true`, do NOT recurse-into-and-exempt its
            // payload. Otherwise recurse: a carrier (`List`/`Maybe`/`Result`/…)
            // is non-derivable exactly when one of its arguments is.
            is_opaque_boxed_wrapper_canon(interner, *name)
                || args
                    .iter()
                    .any(|a| field_type_nonderivable(interner, a))
        }
        canon::Type::Tuple(elems) => elems
            .iter()
            .any(|e| field_type_nonderivable(interner, e)),
        canon::Type::Record(fields) => fields
            .iter()
            .any(|(_, f)| field_type_nonderivable(interner, f)),
        canon::Type::Var(_) | canon::Type::Unit => false,
    }
}

#[allow(clippy::too_many_arguments)] // qualifier_paths added to thread context; refactor tracked
fn synthesize_record_alias_ctors(
    m: &src::Module,
    home: &[Symbol],
    env: &Env,
    type_home_map: &BTreeMap<Symbol, Vec<Symbol>>,
    qualifier_paths: &BTreeMap<Symbol, Vec<Symbol>>,
    aliases: &BTreeMap<Symbol, AliasDef>,
    seen_ctors: &BTreeMap<Symbol, Span>,
    interner: &Interner,
) -> DResult<Vec<canon::Def>> {
    let mut synth = Vec::new();
    for a in &m.aliases {
        // Strict-literal gate: only a `{ … }` body carries a constructor.
        let src::TypeAnnotation::TRecord(fields) = &a.value.body.value else {
            continue;
        };
        let alias_name = a.value.name.value;
        let alias_span = a.value.name.span;

        // Canonicalise every field type ONCE, in declared (source) order. The
        // alias's own params fall through to `Type::Var` (empty `subst`), so a
        // param used in a field generalises and a phantom param drops out. The
        // alias name is pre-seeded into `visited` so a self-referential field
        // (`{ next : List T }`) expands exactly as `x : T` would — the ctor's
        // return record is byte-identical to the annotation expansion.
        let ctx = TypeCtx {
            env,
            type_home_map,
            qualifier_paths,
            aliases,
            interner,
            ann_span: a.value.body.span,
        };
        let subst = BTreeMap::new();
        let mut free_set = BTreeSet::new();
        let mut visited = vec![alias_name];
        let mut can_fields: Vec<(Symbol, canon::Type)> = Vec::with_capacity(fields.len());
        for (fname, fty) in fields {
            let cty = canonicalise_type(fty, &ctx, &subst, &mut free_set, &mut visited)?;
            can_fields.push((*fname, cty));
        }

        // Data-record gate (#82 scope). DECLINE synthesis when ANY field type is
        // non-derivable-as-a-struct-field ([`field_type_nonderivable`]). The
        // synthesised constructor's body is a record literal that lowers to a
        // `#[derive(Clone, Debug, PartialEq)]` + `impl SkyStringify` struct over
        // the field types; a field that satisfies none of those obligations makes
        // the emitted Rust fail to build. Two disjoint non-derivable shapes:
        //
        //   * a raw function — directly (`{ handler : Int -> Msg }`, config-record
        //     aliases like `Live.app`'s cfg) OR nested inside a derive carrier
        //     (`{ xs : List (Int -> Int) }`, `{ f : Maybe (Int -> Int) }`,
        //     `{ p : (Int -> Int, Bool) }`, `{ g : Result e (Int -> Int) }`, a
        //     nested record). For a DIRECT arrow the lowerer's own
        //     `embeds_nonderivable_function` region gate would reject the (unused,
        //     un-DCE'd) ctor body at SKY-L0107; for a nested one the head-only
        //     round-0 gate emitted-then-cargo-failed.
        //   * an OPAQUE boxed-wrapper field (`{ dec : Decoder Int }`,
        //     `{ cmd : Cmd Msg }`, `Sub`, `Task`) — round-1 verified skyc-0 then
        //     cargo-101 (E0277 Clone/Debug, E0369 ==, E0599 SkyStringify), because
        //     the wrapper VALUE is itself non-derivable; nesting one under a
        //     carrier (`List (Decoder Int)`, `Maybe (Cmd Msg)`) is equally
        //     non-derivable (the predicate recurses).
        //
        // There is no whole-program DCE to prune an *unused* such ctor, so
        // synthesizing it would turn a module that merely *names* the alias into a
        // build failure — a regression (pre-#82 a merely-named alias built clean).
        // Declining keeps the alias's exact pre-#82 behaviour (no positional
        // constructor) and makes the un-buildable constructor UNREPRESENTABLE at
        // canon rather than emitted-then-rejected. See [`field_type_nonderivable`]
        // for why the synthesis predicate is NOT the lowerer's function-embedding
        // one (opaque head in field position ⇒ non-derivable, a deliberate flip).
        if can_fields
            .iter()
            .any(|(_, t)| field_type_nonderivable(interner, t))
        {
            continue;
        }

        // A record alias whose name coincides with a data constructor is valid
        // per the upstream Elm / Sky rules: the TYPE namespace (`type alias`) and
        // the CONSTRUCTOR namespace (`type … = Ctor | …`) are distinct.
        //
        // The upstream Haskell (`Sky.Canonicalise.Module.registerAliases`) inserts
        // the alias name into `_vars` via `Map.insert` without ANY check against
        // `_ctors` — the two occupy separate namespaces and coexist peacefully.
        //
        // `resolve_var` here also checks `env.ctors` BEFORE `env.vars`, so if we
        // DID synthesise the alias auto-ctor entry into `env.vars`, the ADT ctor
        // would always win in expression position anyway.  Skipping synthesis
        // achieves the same effect more cleanly: the ADT constructor is the sole
        // winner, and there is no competing entry in `env.vars` that a user could
        // accidentally reference.
        //
        // The old code emitted SKY-N0010 (DuplicateValue) here, which was wrong
        // and broke the `type Tab = Overview | …` + `type alias Overview = { … }`
        // pattern found in examples/25-sky-console/src/State.sky.
        //
        // Ref: `Sky.Canonicalise.Module.registerAliases` upstream, lines 1759–1775.
        if seen_ctors.contains_key(&alias_name) {
            continue;
        }

        // Quantified vars, ordered by resolved NAME (stable wire order — intern
        // ids are allocation-dependent), matching `canonicalise_value`.
        let mut free_vars: Vec<Symbol> = free_set.into_iter().collect();
        free_vars.sort_by(|x, y| interner.resolve(*x).cmp(&interner.resolve(*y)));

        // Three co-constructed views from the one ordered `can_fields` vec:
        // parameter patterns, the body record literal, and the arrow arg types.
        let patterns: Vec<canon::Pattern> = can_fields
            .iter()
            .map(|(fname, _)| Located::new(alias_span, canon::Pattern_::PVar(*fname)))
            .collect();
        let body_fields: Vec<(Symbol, canon::Expr)> = can_fields
            .iter()
            .map(|(fname, _)| {
                (
                    *fname,
                    Located::new(alias_span, canon::Expr_::VarLocal(*fname)),
                )
            })
            .collect();
        let body = Located::new(alias_span, canon::Expr_::Record(body_fields));
        let mut ty = canon::Type::Record(can_fields.clone());
        for (_, fty) in can_fields.iter().rev() {
            ty = canon::Type::Lambda(Box::new(fty.clone()), Box::new(ty));
        }

        synth.push(canon::Def::Typed {
            home: home.to_vec(),
            name: a.value.name,
            free_vars,
            patterns,
            body,
            ty,
        });
    }
    Ok(synth)
}

/// Register a dep-imported type name into `type_home_map`, rejecting a clash
/// with a DIFFERENT already-imported home under [`NameError::DuplicateType`]
/// (SKY-N0012).
///
/// The two types are genuinely distinct nominal identities `(home, name)` — the
/// #100 fix lets each emit its own Rust enum — but bringing both into scope
/// under the same UNQUALIFIED type name (`import ModA exposing (ColorA)` +
/// `import ModB exposing (ColorA)`) leaves `ColorA` unresolvable to a single
/// type. This is the type-level analogue of the value-import ambiguity gate
/// ([`check_and_inject_value`], SKY-N0024). A re-injection of the SAME
/// `(name, home)` — e.g. a diamond dependency reaching one home by two paths —
/// is idempotent and accepted.
fn inject_dep_type(
    type_home_map: &mut BTreeMap<Symbol, Vec<Symbol>>,
    type_name: Symbol,
    home: &[Symbol],
    span: Span,
    interner: &Interner,
) -> DResult<()> {
    if let Some(existing) = type_home_map.get(&type_name)
        && existing.as_slice() != home
    {
        return Err(Diagnostic::Name {
            span,
            msg: NameError::DuplicateType {
                name: name_str(interner, type_name)?,
                first: Span::DUMMY,
            },
        });
    }
    type_home_map.insert(type_name, home.to_vec());
    Ok(())
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
                    dep.kernel_aliases.get(&name),
                    interner,
                )?;
            }
            // Inject all dep types (union homes) + all ctors.
            for (&type_name, home) in &dep.types {
                inject_dep_type(type_home_map, type_name, home, import.name.span, interner)?;
                inject_ctors_for_type(
                    type_name,
                    dep,
                    env,
                    import.name.span,
                    unqual_ctor_origins,
                    interner,
                )?;
            }
            // Inject all dep aliases, carrying the dep module's type scope so
            // body expansion can resolve types not imported by the IMPORTING
            // module (e.g. `Model`'s body references `Piece` from Chess.Piece
            // which is only in State.sky's scope, not in Home.sky's).
            for (&alias_name, ea) in &dep.aliases {
                injected_aliases
                    .entry(alias_name)
                    .or_insert_with(|| AliasDef {
                        params: ea.params.clone(),
                        body: ea.body.clone(),
                        dep_scope_types: Some(dep.scope_types.clone()),
                        dep_scope_aliases: Some(dep.scope_aliases.clone()),
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
                            dep.kernel_aliases.get(name),
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
                                inject_dep_type(
                                    type_home_map,
                                    *type_name,
                                    home,
                                    item.span,
                                    interner,
                                )?;
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
                                    dep_scope_types: Some(dep.scope_types.clone()),
                                    dep_scope_aliases: Some(dep.scope_aliases.clone()),
                                });
                            // A record alias also exports a value-level
                            // auto-constructor under the same name (#82); when the
                            // dep exposed it (present in `dep.values`), bring it
                            // into the value namespace too so `exposing (Account)`
                            // makes `Account` usable as a constructor.
                            if dep.values.contains(type_name) {
                                // A record-alias auto-constructor is never a kernel
                                // alias (kernel aliases are lowercase values, not
                                // type names), so this lookup is always `None`;
                                // passed for uniformity with the value paths.
                                check_and_inject_value(
                                    *type_name,
                                    dep_path,
                                    item.span,
                                    env,
                                    unqual_origins,
                                    dep.kernel_aliases.get(type_name),
                                    interner,
                                )?;
                            }
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
    let qual_map = std::rc::Rc::make_mut(&mut env.qual_vars).entry(qualifier).or_default();
    for &v in &dep.values {
        // A dep value that is a Stage-4 kernel alias resolves as its kernel, so a
        // qualified `Alias.f` routes straight to the kernel dispatch — never a
        // `TopLevel(dep_path)` reference to a def the alias module never emits.
        if let Some(alias) = dep.kernel_aliases.get(&v) {
            qual_map.insert(v, VarHome::Kernel(Some(alias.id), alias.module, alias.function));
        } else {
            qual_map.insert(v, VarHome::TopLevel(dep_path.clone()));
        }
    }
    // Register qualified constructors so `Alias.CtorName` resolves correctly.
    // Needed for compiled-source ADTs (e.g. `Money.USD` from `import Std.Money
    // as Money`) where constructors are not stdlib kernels and never enter
    // `qual_vars`.  We register ALL ctors from this dep regardless of the
    // user's `exposing (...)` clause — qualified access does not require the
    // name to be in the exposing list (only unqualified access does).
    if !dep.ctors.is_empty() {
        let qual_ctor_map = std::rc::Rc::make_mut(&mut env.qual_ctors).entry(qualifier).or_default();
        for (ctor_sym, ctor_home) in &dep.ctors {
            qual_ctor_map.entry(*ctor_sym).or_insert_with(|| ctor_home.clone());
        }
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
    kernel_alias: Option<&crate::ExportedKernelAlias>,
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
    // A kernel alias resolves unqualified to its kernel, same as it would
    // qualified — otherwise `import Std.PubSub exposing (publish)` would bind
    // `publish` to a non-existent `TopLevel` def.
    let home = kernel_alias.map_or_else(
        || VarHome::TopLevel(dep_path.to_vec()),
        |a| VarHome::Kernel(Some(a.id), a.module, a.function),
    );
    env.vars.insert(name, home);
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
                std::rc::Rc::make_mut(&mut env.ctors).insert(ctor_home.name, ctor_home.clone());
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
fn build_module_exports(
    home: &[Symbol],
    m: &src::Module,
    env: &Env,
    synth_ctor_names: &BTreeSet<Symbol>,
    kernel_aliases: &BTreeMap<Symbol, KernelAlias>,
) -> crate::ModuleExports {
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
                // A record alias also exports its value-level auto-constructor
                // (#82) — but ONLY when one was actually synthesized (a
                // function-field alias is gated out). The synthesized `Def` lives
                // in this module's `defs`, so the importer's
                // `check_and_inject_value` path registers the name as
                // `TopLevel(dep_path)` with no re-synthesis.
                if synth_ctor_names.contains(&a.value.name.value) {
                    exports.values.insert(a.value.name.value);
                }
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
                                    // Exposing a record alias also exposes its
                                    // value-level auto-constructor (#82), when one
                                    // was synthesized (function-field aliases are
                                    // gated out).
                                    if synth_ctor_names.contains(type_name) {
                                        exports.values.insert(*type_name);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Record every EXPORTED kernel alias so importers register `Alias.f` as the
    // kernel rather than a `TopLevel` reference. A kernel alias is an ordinary
    // value as far as `exposing` is concerned, so it is already in
    // `exports.values`; we only add the ones actually exported (an un-exposed
    // alias is module-private and needs no cross-module entry).
    for (&name, alias) in kernel_aliases {
        if exports.values.contains(&name) {
            exports.kernel_aliases.insert(
                name,
                crate::ExportedKernelAlias {
                    id: alias.id,
                    module: alias.module,
                    function: alias.function,
                },
            );
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
        std::rc::Rc::make_mut(&mut env.ctors).insert(
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
    qualifier_paths: &BTreeMap<Symbol, Vec<Symbol>>,
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
            qualifier_paths,
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
    qualifier_paths: &BTreeMap<Symbol, Vec<Symbol>>,
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
                qualifier_paths,
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
        // Triple-quoted strings: desugar `{{expr}}` interpolation into a `++`
        // chain at canonicalise time. Mirrors `Sky.Canonicalise.Expression.hs`
        // line 42 (`Src.MultilineStr s -> desugarMultiline env s`).
        src::Expr_::MultilineStr(s) => desugar_multiline(s, span, env, interner)?,
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
    if let Some(home) = env.lookup_var(name) {
        // A local / top-level / explicit-exposed / prelude binding wins over any
        // wildcard-exposed member of the same spelling (silent shadow, #98).
        return Ok(var_home_to_expr(name, home));
    }
    // Low-priority wildcard tier (#98): only reached when the higher tiers miss.
    resolve_wildcard_var(name, span, env, interner)
}

/// Map a resolved [`VarHome`] to its canonical [`canon::Expr_`] form. Total over
/// all three variants so both the primary tiers and the wildcard tier share one
/// lowering rule (a wildcard clone is always a `Kernel`, but keeping the mapping
/// total avoids any partial assumption).
fn var_home_to_expr(name: Symbol, home: &VarHome) -> canon::Expr_ {
    match home {
        VarHome::Local => canon::Expr_::VarLocal(name),
        VarHome::TopLevel(module) => canon::Expr_::VarTopLevel {
            module: module.clone(),
            name,
        },
        VarHome::Kernel(id, m, f) => canon::Expr_::VarKernel {
            id: *id,
            module: *m,
            name: *f,
        },
    }
}

/// Resolve a bare name against the low-priority wildcard tier ([`Env::wildcard_vars`],
/// #98), or fail with the ordinary `SKY-N0001` when it is absent there too.
///
/// * Exactly one surviving origin → resolve to its cloned kernel home (identical
///   to the qualified reference).
/// * Two or more distinct origins → [`NameError::AmbiguousImport`] (SKY-N0024) at
///   THIS use site, listing every contributing module — never a silent
///   last-wins.
fn resolve_wildcard_var(
    name: Symbol,
    span: Span,
    env: &Env,
    interner: &Interner,
) -> DResult<canon::Expr_> {
    // Only a non-empty origin set participates; an empty entry (never produced by
    // the injector) falls through to `ValueNotFound`.
    if let Some(origins) = env.wildcard_vars.get(&name).filter(|o| !o.is_empty()) {
        if origins.len() == 1 {
            if let Some(origin) = origins.values().next() {
                return Ok(var_home_to_expr(name, &origin.home));
            }
            // Unreachable (len == 1 ⇒ non-empty), but stay total — no `unwrap`.
            return Err(value_not_found(name, span, env, interner)?);
        }
        // Two or more distinct modules expose this name unqualified → ambiguous.
        let modules: Box<[Box<str>]> = origins
            .values()
            .map(|o| path_to_dot_string(interner, &o.dep_path))
            .collect();
        return Err(Diagnostic::Name {
            span,
            msg: NameError::AmbiguousImport {
                name: name_str(interner, name)?,
                modules,
            },
        });
    }
    Err(value_not_found(name, span, env, interner)?)
}

/// Build the ordinary `SKY-N0001` [`NameError::ValueNotFound`] for a bare name.
///
/// A bare value name can resolve to either a value binding or a constructor used
/// as a value, so the suggestion pool spans both namespaces (value bindings
/// first, then constructor names).
fn value_not_found(
    name: Symbol,
    span: Span,
    env: &Env,
    interner: &Interner,
) -> DResult<Diagnostic> {
    Ok(Diagnostic::Name {
        span,
        msg: NameError::ValueNotFound {
            name: name_str(interner, name)?,
            suggestions: suggestions(
                name,
                env.vars.keys().chain(env.ctors.keys()).copied(),
                interner,
            ),
        },
    })
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
        Some(VarHome::Kernel(id, m, f)) => Ok(canon::Expr_::VarKernel {
            id: *id,
            module: *m,
            name: *f,
        }),
        Some(VarHome::TopLevel(module)) => Ok(canon::Expr_::VarTopLevel {
            module: module.clone(),
            name,
        }),
        Some(VarHome::Local) => Ok(canon::Expr_::VarLocal(name)),
        // The qualifier resolves via qual_vars but the member is not a value.
        // Check qual_ctors — needed for compiled-source ADT constructors accessed
        // as `Alias.CtorName` (e.g. `Money.USD`, `Money.EUR`).
        None => {
            if let Some(ctor_map) = env.qual_ctors.get(&qualifier)
                && let Some(ch) = ctor_map.get(&name)
            {
                return Ok(canon::Expr_::VarCtor {
                    home: ch.home.clone(),
                    type_name: ch.type_name,
                    name: ch.name,
                    index: ch.index,
                });
            }
            Err(Diagnostic::Name {
                span,
                msg: NameError::NoSuchMember {
                    module: name_str(interner, qualifier)?,
                    member: name_str(interner, name)?,
                    suggestions: suggestions(
                        name,
                        members
                            .keys()
                            .chain(
                                env.qual_ctors
                                    .get(&qualifier)
                                    .iter()
                                    .flat_map(|m| m.keys()),
                            )
                            .copied(),
                        interner,
                    ),
                },
            })
        }
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
/// Resolve the `home` path for an **unqualified** type constructor `name`.
///
/// Resolution order:
/// 1. `type_home_map` — user-defined ADTs and explicitly-imported dep types.
/// 2. `RESERVED_BUILTIN_TYPES` / `EXTRA_BUILTIN_TYPE_NAMES` — known builtin
///    names that the lowerer handles by explicit arm; they receive the
///    empty-home sentinel (`Vec::new()`).
/// 3. Anything else: emit `TypeNotFound` / `SKY-N0002` with a did-you-mean
///    suggestion list.  This replaces the former `unwrap_or_default()` silent
///    fallback and the downstream `enum_variants` unique-match heuristic
///    (removed in `sky_lower`) that previously ICE'd with `SKY-I0001` on
///    ambiguous or absent names.
#[allow(clippy::redundant_else)] // cascading early-returns need else for clarity
fn resolve_unqualified_type_home(name: Symbol, ctx: &TypeCtx) -> DResult<Vec<Symbol>> {
    if let Some(h) = ctx.type_home_map.get(&name) {
        return Ok(h.clone());
    }
    let name_s = ctx.interner.resolve(name).unwrap_or("");
    if RESERVED_BUILTIN_TYPES.contains(&name_s)
        || EXTRA_BUILTIN_TYPE_NAMES.contains(&name_s)
        || KERNEL_IMPLICIT_PRELUDE_TYPE_NAMES.contains(&name_s)
    {
        // Empty-home sentinel: the lowerer's per-name explicit arm resolves it.
        return Ok(Vec::new());
    }
    // Unknown type — fail closed at canon time so this never reaches the
    // lowerer as an empty-home Con (former ICE path, SKY-I0001).
    let candidates = ctx.type_home_map.keys().chain(ctx.aliases.keys()).copied();
    let sugg = suggestions(name, candidates, ctx.interner);
    Err(Diagnostic::Name {
        span: ctx.ann_span,
        msg: NameError::TypeNotFound {
            name: name_s.into(),
            suggestions: sugg,
        },
    })
}

#[allow(clippy::too_many_lines)] // exhaustive type-annotation walker; scope-alias merge pushed it over 100
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
        src::TypeAnnotation::TType(qualifier, segments, args) => {
            let name = segments.last().copied().unwrap_or_else(|| {
                // An unnamed type cannot occur in the grammar; fall back to
                // the home module's name so the node is still well-formed.
                ctx.env.home.last().copied().unwrap_or_else(name_zero)
            });
            // Tier-1 qualified-type validation: when the parser produced a
            // non-empty qualifier (e.g. `JsonDec.Decoder`), verify it names a
            // known module qualifier in `env.qual_vars`. Tier-2 (resolving the
            // actual type name via a `qual_types` map) is a follow-up once the
            // multi-module import layer builds that map; for now, a valid
            // qualifier is sufficient to accept the annotation and look the type
            // up in `type_home_map` as usual.
            let qualifier_str = ctx.interner.resolve(*qualifier).unwrap_or("");
            if !qualifier_str.is_empty() && !ctx.env.qual_vars.contains_key(qualifier) {
                let sugg = suggestions(*qualifier, ctx.env.qual_vars.keys().copied(), ctx.interner);
                return Err(Diagnostic::Name {
                    span: ctx.ann_span,
                    msg: NameError::UnknownModule {
                        qualifier: qualifier_str.into(),
                        suggestions: sugg,
                    },
                });
            }
            // Type arguments are canonicalised under the current substitution
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
                // When the alias was injected from a dep module, expand its
                // body in the DEP's type scope rather than the importing
                // module's scope. This lets body references to types from the
                // dep's OWN deps (e.g. `Piece` in `Model`'s body, where
                // `Piece` came from Chess.Piece which is NOT imported by the
                // importing module) still resolve correctly.
                let expanded = if let Some(dep_scope) = &alias.dep_scope_types {
                    // Merge the dep module's alias scope into the current
                    // aliases so alias-typed fields in the body (e.g. `Piece`
                    // from Chess.Piece when expanding `Model` from State) are
                    // visible even if the importing module never imported them
                    // directly.  Lower priority: existing ctx aliases win.
                    let merged_aliases_opt: Option<BTreeMap<Symbol, AliasDef>> =
                        alias.dep_scope_aliases.as_ref().map(|dep_aliases| {
                            let mut m = ctx.aliases.clone();
                            for (name, ea) in dep_aliases {
                                m.entry(*name).or_insert_with(|| AliasDef {
                                    params: ea.params.clone(),
                                    body: ea.body.clone(),
                                    dep_scope_types: alias.dep_scope_types.clone(),
                                    dep_scope_aliases: None,
                                });
                            }
                            m
                        });
                    let aliases_ref: &BTreeMap<Symbol, AliasDef> =
                        merged_aliases_opt.as_ref().map_or(ctx.aliases, |m| m);
                    let alt_ctx = TypeCtx {
                        type_home_map: dep_scope,
                        env: ctx.env,
                        qualifier_paths: ctx.qualifier_paths,
                        aliases: aliases_ref,
                        interner: ctx.interner,
                        ann_span: ctx.ann_span,
                    };
                    canonicalise_type(&alias.body, &alt_ctx, &body_subst, free_vars, visited)?
                } else {
                    canonicalise_type(&alias.body, ctx, &body_subst, free_vars, visited)?
                };
                visited.pop();
                return Ok(expanded);
            }
            // Qualified reference (e.g. `Counter.Msg`): use `qualifier_paths`
            // for the dep module's full home path. It ALSO carries (folded in by
            // `fold_html_stdlib_qualifier_homes`) the canonical `["Html"]` home
            // for a Html-family STDLIB qualifier, so `Attr.Attribute` →
            // `html::Attribute` while `Ui.Attribute` (not folded) falls through to
            // the empty Ui sentinel (#179 / #185). Unqualified: delegate to
            // `resolve_unqualified_type_home` which fails closed with SKY-N0002
            // for unknown names (builtins get the empty-home sentinel).
            let home = if qualifier_str.is_empty() {
                resolve_unqualified_type_home(name, ctx)?
            } else {
                ctx.qualifier_paths
                    .get(qualifier)
                    .cloned()
                    .unwrap_or_else(|| ctx.type_home_map.get(&name).cloned().unwrap_or_default())
            };
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

/// A resolved Stage-4 kernel alias — the target of a standard-library binding
/// of the shape `f = Ffi.kernel "Module_function"`.
///
/// The binding routes every reference of `f` straight to the built-in kernel
/// `id`, so it lowers identically to a qualified `Module.function` call. The
/// `module` / `function` symbols are the first-`_` split of the kernel string,
/// retained so the alias registers a [`VarHome::Kernel`] carrying the same
/// canonical `(module, function)` pair a direct qualified reference produces.
#[derive(Clone, Copy)]
pub struct KernelAlias {
    pub id: StdlibKernel,
    pub module: Symbol,
    pub function: Symbol,
}

/// Recognise a Stage-4 kernel-alias binding and resolve it against the kernel
/// registry — the compiled-source counterpart of the reference compiler's
/// `collectKernelAliases` (`Sky.Build.Compile`).
///
/// A binding qualifies when it takes NO parameters and its body is exactly
/// `Ffi.kernel "Module_function"`. The string is split at the FIRST `_` into a
/// `(module, function)` pair (the `KernelMod_funcName` convention) and looked up
/// in `env.stdlib_index`.
///
/// Returns:
/// * `Ok(None)` — the binding is an ordinary value/function, not a kernel alias.
/// * `Ok(Some(alias))` — a kernel alias whose target is a registered kernel.
/// * `Err(SKY-N0028)` — the binding IS a kernel alias but its string names no
///   registered kernel. This is the FAIL-CLOSED gate demanded by THE SEAL:
///   accepting it would let `skyc` emit a call to a non-existent kernel that
///   type-checks here yet fails the downstream `cargo build`. A kernel the
///   resolver would recognise but the registry does not cover is a
///   representable-but-illegal state, rejected at compile time.
///
/// # Errors
/// [`NameError::UnknownKernelAlias`] (SKY-N0028) when the split `(module,
/// function)` pair is absent from the kernel registry.
pub fn detect_kernel_alias(
    value: &src::Value,
    env: &Env,
    interner: &mut Interner,
) -> DResult<Option<KernelAlias>> {
    // A kernel alias binds a bare value — a binding with parameters is an
    // ordinary function, never the point-free Layer-3 alias shape.
    if !value.patterns.is_empty() {
        return Ok(None);
    }
    // Body must be `Ffi.kernel "<raw>"`, i.e. a call of the qualified
    // `Ffi.kernel` to a single string literal.
    let src::Expr_::Call(callee, args) = &value.body.value else {
        return Ok(None);
    };
    let src::Expr_::VarQual(qualifier, member) = &callee.value else {
        return Ok(None);
    };
    // Compare against the reserved `Ffi.kernel` spelling. These interns are
    // idempotent (the strings almost always already exist), and only run for the
    // narrow `VarQual`-applied-to-one-arg shape, so the cost is negligible.
    let ffi_sym = interner.intern("Ffi")?;
    let kernel_sym = interner.intern("kernel")?;
    if *qualifier != ffi_sym || *member != kernel_sym {
        return Ok(None);
    }
    let [arg] = args.as_slice() else {
        return Ok(None);
    };
    let src::Expr_::Str(raw) = &arg.value else {
        return Ok(None);
    };

    // Split at the FIRST `_` — `"PubSub_publish"` → `("PubSub", "publish")`,
    // matching the runtime's `KernelMod_funcName` convention. A string with no
    // `_`, or an empty module/function half, is a malformed alias and fails
    // closed the same way an unknown kernel does.
    let split = raw.split_once('_').filter(|(m, f)| !m.is_empty() && !f.is_empty());
    // A `SKY-N0028` for the alias — its `module` / `function` are the split
    // halves (empty when the string is malformed, so the message still renders).
    let unknown_alias = |module: Box<str>, function: Box<str>| Diagnostic::Name {
        span: value.body.span,
        msg: NameError::UnknownKernelAlias {
            alias: Box::<str>::from(raw.as_str()),
            module,
            function,
        },
    };
    let Some((module_str, function_str)) = split else {
        return Err(unknown_alias(Box::<str>::from(""), Box::<str>::from("")));
    };
    let module = interner.intern(module_str)?;
    let function = interner.intern(function_str)?;
    // FAIL-CLOSED: only a kernel the registry actually covers resolves. An
    // unregistered pair is rejected here, never emitted as a dangling call.
    env.stdlib_index
        .get(&(module, function))
        .copied()
        .map_or_else(
            || {
                Err(unknown_alias(
                    Box::<str>::from(module_str),
                    Box::<str>::from(function_str),
                ))
            },
            |id| {
                Ok(Some(KernelAlias {
                    id,
                    module,
                    function,
                }))
            },
        )
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

// ── Triple-quoted string interpolation desugar ────────────────────────────────
//
// Faithful Rust port of `Sky.Canonicalise.Expression.desugarMultiline` /
// `splitInterpolation` / `chunkToExpr` / `resolveInterpolationRef`.
//
// Entry point: `desugar_multiline(raw, span, env, interner)` — called from
// `canonicalise_expr` when it sees `src::Expr_::MultilineStr`.
//
// The raw string is split into alternating `Lit` / `Interp` chunks, each
// expression chunk is wrapped in `Basics.toString`, and the whole list is
// folded left into a `++` (String.append) binop chain.
// ─────────────────────────────────────────────────────────────────────────────

/// A parsed chunk from a triple-quoted string.
enum Chunk {
    /// A literal string segment (no interpolation).
    Lit(String),
    /// An interpolation body — the raw text between `{{` and `}}`.
    Interp(String),
}

/// Split a raw triple-quoted string body into alternating literal / expression
/// chunks. Direct Rust port of `Sky.Canonicalise.Expression.splitInterpolation`.
///
/// Escape grammar (spec from upstream `splitInterpolation` comments):
///   `\{{`  → literal `{{`  (no interpolation consumed)
///   `\\`   → literal `\`
///   `\X`   → literal `\X` for any other `X` (verbatim pass-through)
/// An unclosed `{{` (no matching `}}`) is treated as literal content.
fn split_interpolation(raw: &str) -> Vec<Chunk> {
    let chars: Vec<char> = raw.chars().collect();
    let mut result: Vec<Chunk> = Vec::new();
    let mut acc = String::new();
    let mut i = 0;
    while let Some(&c0) = chars.get(i) {
        let c1 = chars.get(i + 1).copied();
        let c2 = chars.get(i + 2).copied();
        match (c0, c1, c2) {
            // `\{{` → emit literal `{{`, skip 3 chars.
            ('\\', Some('{'), Some('{')) => {
                acc.push_str("{{");
                i += 3;
            }
            // `\\` → emit literal `\`, skip 2 chars.
            ('\\', Some('\\'), _) => {
                acc.push('\\');
                i += 2;
            }
            // `{{` → start an interpolation. Flush accumulated literal first.
            ('{', Some('{'), _) => {
                if !acc.is_empty() {
                    result.push(Chunk::Lit(std::mem::take(&mut acc)));
                }
                i += 2;
                // Collect the body up to the matching `}}`.
                let mut body = String::new();
                let mut closed = false;
                while let Some(&bc) = chars.get(i) {
                    if bc == '}' && chars.get(i + 1).copied() == Some('}') {
                        i += 2;
                        closed = true;
                        break;
                    }
                    body.push(bc);
                    i += 1;
                }
                if closed {
                    result.push(Chunk::Interp(body));
                } else {
                    // Unclosed `{{` — treat entire `{{body` as literal content.
                    acc.push_str("{{");
                    acc.push_str(&body);
                }
            }
            // Any other character — copy verbatim.
            _ => {
                acc.push(c0);
                i += 1;
            }
        }
    }
    if !acc.is_empty() {
        result.push(Chunk::Lit(acc));
    }
    result
}

/// Convert a `Chunk` to a canonical expression.
/// Port of `Sky.Canonicalise.Expression.chunkToExpr`.
///
/// `Lit` → `Expr_::Str`.
/// `Interp` → resolve the body as a simple ref, then wrap in `Basics.toString`.
fn chunk_to_expr(
    chunk: Chunk,
    span: Span,
    env: &Env,
    interner: &mut Interner,
) -> DResult<canon::Expr> {
    match chunk {
        Chunk::Lit(s) => Ok(Located::new(span, canon::Expr_::Str(s))),
        Chunk::Interp(body) => {
            // Trim leading/trailing whitespace, matching the Haskell
            // `dropWhile (== ' ') (reverse (dropWhile …))`.
            let trimmed = body.trim();
            let resolved = resolve_interp_ref(trimmed, span, env, interner)?;
            // Wrap in Basics.toString.
            let mod_sym = interner.intern("Basics")?;
            let fn_sym = interner.intern("toString")?;
            let stringify = Located::new(
                span,
                canon::Expr_::VarKernel {
                    id: Some(StdlibKernel::BasicsToString),
                    module: mod_sym,
                    name: fn_sym,
                },
            );
            Ok(Located::new(
                span,
                canon::Expr_::Call(Box::new(stringify), vec![resolved]),
            ))
        }
    }
}

/// Resolve a simple interpolation reference.
/// Port of `Sky.Canonicalise.Expression.resolveInterpolationRef`.
///
/// Handles four shapes:
///   `foo`          — bare identifier → local var (or kernel if in scope)
///   `record.field` — field access → `Access(VarLocal(record), field)`
///   `Module.func`  — qualified name → `VarKernel` (if known) or literal fallback
///   `func arg`     — single function call → `Call(resolve(func), [resolve(arg)])`
/// Anything more complex falls back to a literal `{{...}}` string (clear signal
/// to the developer that only simple expressions are interpolable).
fn resolve_interp_ref(
    s: &str,
    span: Span,
    env: &Env,
    interner: &mut Interner,
) -> DResult<canon::Expr> {
    // Check for `func arg` (a single space separates them).
    if let Some(space_pos) = s.find(' ') {
        let func_str = &s[..space_pos];
        let arg_str = s[space_pos + 1..].trim();
        if !func_str.is_empty() && !arg_str.is_empty() {
            let func_expr = resolve_interp_ref(func_str, span, env, interner)?;
            let arg_expr = resolve_interp_ref(arg_str, span, env, interner)?;
            return Ok(Located::new(
                span,
                canon::Expr_::Call(Box::new(func_expr), vec![arg_expr]),
            ));
        }
    }
    resolve_simple_interp_ref(s, span, env, interner)
}

/// Inner resolver for an interpolation reference without a space (no call form).
/// Port of `resolveSimpleRef` inside `resolveInterpolationRef`.
fn resolve_simple_interp_ref(
    s: &str,
    span: Span,
    env: &Env,
    interner: &mut Interner,
) -> DResult<canon::Expr> {
    // Numeric literal. A body that begins with an ASCII digit can never be an
    // identifier (Sky identifiers never start with a digit), so it is an
    // integer or float literal — NOT a local reference. Emitting `VarLocal`
    // here (the fall-through below) would leak an unbound name past
    // canonicalisation and fire the SKY-I0001 "unbound local `<n>`" ICE in
    // `constrain`, which treats an unresolved local as a violated invariant
    // (the resolver is supposed to have resolved every local). Recognise the
    // literal instead, so e.g. `{{String.fromInt 54}}` lowers to
    // `String.fromInt 54` and prints "54". This must precede the `.`-split
    // below, else a float like `1.5` is mis-parsed as `Access(1, 5)`.
    //
    // Divergence from ../sky: `resolveInterpolationRef` lacks literal handling
    // and would surface `54` as a `VarLocal` → naming error. Recognising the
    // literal is strictly better (a well-typed program compiles instead of
    // ICE-ing). Recorded in docs/divergences-from-sky.md.
    if s.starts_with(|c: char| c.is_ascii_digit()) {
        if let Ok(n) = s.parse::<i64>() {
            return Ok(Located::new(span, canon::Expr_::Int(n)));
        }
        if let Ok(f) = s.parse::<f64>()
            && f.is_finite()
        {
            return Ok(Located::new(span, canon::Expr_::Float(f)));
        }
        // Leading digit but not a valid `Int`/`Float` (e.g. `1e400`, `0xFF`,
        // `9z`): fall back to the literal `{{...}}` string rather than emit a
        // `VarLocal` that would ICE. Mirrors the "too complex → literal" policy.
        return Ok(Located::new(span, canon::Expr_::Str(format!("{{{{{s}}}}}"))));
    }
    if let Some(dot_pos) = s.find('.') {
        let first = &s[..dot_pos];
        let rest = &s[dot_pos + 1..];
        if first.is_empty() || rest.is_empty() {
            // Degenerate `.foo` or `foo.` — literal fallback.
            return Ok(Located::new(
                span,
                canon::Expr_::Str(format!("{{{{{s}}}}}")),
            ));
        }
        let first_char = first.chars().next().unwrap_or('_');
        if first_char.is_uppercase() {
            // Qualified reference `Module.func`.
            let qual_sym = interner.intern(first)?;
            // Look up the qualifier in the current environment.
            if let Some(members) = env.qual_vars.get(&qual_sym) {
                let name_sym = interner.intern(rest)?;
                if let Some(home) = members.get(&name_sym) {
                    return Ok(Located::new(span, var_home_to_expr(name_sym, home)));
                }
            }
            // Unknown module or member — literal fallback (clear signal to dev).
            return Ok(Located::new(
                span,
                canon::Expr_::Str(format!("{{{{{s}}}}}")),
            ));
        }
        // Lowercase `record.field` → `Access(VarLocal(record), field)`.
        let rec_sym = interner.intern(first)?;
        let field_sym = interner.intern(rest)?;
        return Ok(Located::new(
            span,
            canon::Expr_::Access(
                Box::new(Located::new(span, canon::Expr_::VarLocal(rec_sym))),
                field_sym,
            ),
        ));
    }
    // Bare identifier — look up in vars, then wildcard tier (mirrors
    // `resolve_wildcard_var` but treats ambiguity as VarLocal rather
    // than a hard error, since interpolation refs are best-effort).
    let sym = interner.intern(s)?;
    let expr = match (
        env.vars.get(&sym),
        env.wildcard_vars.get(&sym).filter(|o| !o.is_empty()),
    ) {
        (Some(h), _) => var_home_to_expr(sym, h),
        // Unambiguous wildcard import — resolve to its one origin.
        (None, Some(origins)) if origins.len() == 1 => origins
            .values()
            .next()
            .map_or(canon::Expr_::VarLocal(sym), |origin| {
                var_home_to_expr(sym, &origin.home)
            }),
        // Ambiguous wildcard or unknown — fall back to VarLocal; the type
        // checker will catch genuine errors later.
        _ => canon::Expr_::VarLocal(sym),
    };
    Ok(Located::new(span, expr))
}

/// Desugar a triple-quoted string into a `++`-chained canonical expression.
/// Entry point from `canonicalise_expr`. Port of
/// `Sky.Canonicalise.Expression.desugarMultiline`.
fn desugar_multiline(
    raw: &str,
    span: Span,
    env: &Env,
    interner: &mut Interner,
) -> DResult<canon::Expr_> {
    let chunks = split_interpolation(raw);
    // Build one canonical expression per chunk.
    let mut parts: Vec<canon::Expr> = Vec::with_capacity(chunks.len());
    for chunk in chunks {
        parts.push(chunk_to_expr(chunk, span, env, interner)?);
    }
    match parts.len() {
        0 => Ok(canon::Expr_::Str(String::new())),
        1 => Ok(parts.remove(0).value),
        _ => {
            // Left-fold into a `++` chain: ((a ++ b) ++ c) ++ …
            let mut iter = parts.into_iter();
            let Some(first) = iter.next() else {
                // Unreachable: the match arm guards len >= 2.
                return Ok(canon::Expr_::Str(String::new()));
            };
            let op_sym = interner.intern("++")?;
            let home_sym = interner.intern("Basics")?;
            let func_sym = interner.intern("append")?;
            let mut acc = first;
            for part in iter {
                let merged_span = Span::new(acc.span.lo, part.span.hi);
                acc = Located::new(
                    merged_span,
                    canon::Expr_::Binop {
                        op: op_sym,
                        home: home_sym,
                        func: func_sym,
                        lhs: Box::new(acc),
                        rhs: Box::new(part),
                    },
                );
            }
            Ok(acc.value)
        }
    }
}

#[cfg(test)]
mod alias_ctor_gate_tests {
    //! Unit coverage for [`field_type_nonderivable`] — the STRUCT-derivability
    //! gate that decides whether a record `type alias` gets a synthesised
    //! auto-constructor (SKY-N0001 / #82). It returns `true` (DECLINE synthesis)
    //! when a field type could NOT be a field of a
    //! `#[derive(Clone, Debug, PartialEq)]` + `impl SkyStringify` struct — a raw
    //! function at ANY depth inside a derive carrier (the round-1 seal fix) OR an
    //! OPAQUE boxed-wrapper (`Decoder` / `Task` / `Cmd` / `Sub`) in field
    //! position (the round-2 flip: the wrapper VALUE is itself non-derivable).
    //! It returns `false` only for genuinely derivable data (plain records,
    //! non-opaque parametric containers of derivable payloads, vars, unit).

    use super::*;

    fn sym(i: &mut Interner, s: &str) -> Symbol {
        i.intern(s).expect("intern must succeed")
    }

    /// A bare arrow `() -> ()` — the minimal `Lambda`; only its shape matters to
    /// the predicate.
    fn arrow() -> canon::Type {
        canon::Type::Lambda(Box::new(canon::Type::Unit), Box::new(canon::Type::Unit))
    }

    fn con(i: &mut Interner, name: &str, args: Vec<canon::Type>) -> canon::Type {
        let n = sym(i, name);
        canon::Type::Con {
            home: Vec::new(),
            name: n,
            args,
        }
    }

    #[test]
    fn direct_arrow_field_is_nonderivable() {
        let i = Interner::new();
        // `{ handler : () -> () }` — a raw function lowers to `Box<dyn Fn>`.
        assert!(field_type_nonderivable(&i, &arrow()));
    }

    #[test]
    fn function_nested_in_derive_carriers_is_nonderivable() {
        let mut i = Interner::new();

        // `List (a -> b)` — the round-1 seal break.
        let list_arrow = con(&mut i, "List", vec![arrow()]);
        assert!(
            field_type_nonderivable(&i, &list_arrow),
            "List (arrow) must be gated"
        );

        // `Maybe (a -> b)`.
        let maybe_arrow = con(&mut i, "Maybe", vec![arrow()]);
        assert!(
            field_type_nonderivable(&i, &maybe_arrow),
            "Maybe (arrow) must be gated"
        );

        // `(a -> b, Bool)` — arrow in a tuple element.
        let bool_con = con(&mut i, "Bool", vec![]);
        let tuple_arrow = canon::Type::Tuple(vec![arrow(), bool_con]);
        assert!(
            field_type_nonderivable(&i, &tuple_arrow),
            "tuple carrying an arrow must be gated"
        );

        // `Result Error (a -> b)` — arrow in the second type argument.
        let err_con = con(&mut i, "Error", vec![]);
        let result_arrow = con(&mut i, "Result", vec![err_con, arrow()]);
        assert!(
            field_type_nonderivable(&i, &result_arrow),
            "Result e (arrow) must be gated"
        );

        // Nested record: `{ inner : { f : a -> b } }`.
        let f = sym(&mut i, "f");
        let inner = sym(&mut i, "inner");
        let inner_rec = canon::Type::Record(vec![(f, arrow())]);
        let nested_rec = canon::Type::Record(vec![(inner, inner_rec)]);
        assert!(
            field_type_nonderivable(&i, &nested_rec),
            "a nested record carrying an arrow must be gated"
        );
    }

    #[test]
    fn opaque_wrapper_field_is_nonderivable() {
        // ROUND-2 FLIP. An opaque boxed-wrapper in FIELD position is ITSELF
        // non-derivable as a struct field — its runtime rep (`Box<dyn Fn>` /
        // boxed-thunk enum / `Pin<Box<dyn Future>>`) impls no
        // `Clone`/`Debug`/`PartialEq`/`SkyStringify`. So the head SHORT-CIRCUITS
        // to `true`, DECLINING synthesis (round-1 asserted the opposite — the
        // seal hole: skyc-0 then cargo-101).
        let mut i = Interner::new();

        // `Decoder Int` — opaque head, no raw arrow anywhere, yet non-derivable.
        let int_con = con(&mut i, "Int", vec![]);
        let decoder_int = con(&mut i, "Decoder", vec![int_con]);
        assert!(
            field_type_nonderivable(&i, &decoder_int),
            "Decoder Int is a non-derivable struct field → must be gated"
        );

        // `Cmd Msg` — boxed-thunk enum, non-derivable.
        let msg_con = con(&mut i, "Msg", vec![]);
        let cmd_msg = con(&mut i, "Cmd", vec![msg_con]);
        assert!(
            field_type_nonderivable(&i, &cmd_msg),
            "Cmd Msg is a non-derivable struct field → must be gated"
        );

        // `Sub Msg`.
        let msg2 = con(&mut i, "Msg", vec![]);
        let sub_msg = con(&mut i, "Sub", vec![msg2]);
        assert!(
            field_type_nonderivable(&i, &sub_msg),
            "Sub Msg is a non-derivable struct field → must be gated"
        );

        // `Task Error a` — `Pin<Box<dyn Future>>`, non-derivable regardless of
        // its (here function) payload.
        let err_con = con(&mut i, "Error", vec![]);
        let task_arrow = con(&mut i, "Task", vec![err_con, arrow()]);
        assert!(
            field_type_nonderivable(&i, &task_arrow),
            "Task Error a is a non-derivable struct field → must be gated"
        );
    }

    #[test]
    fn opaque_wrapper_nested_under_carrier_is_nonderivable() {
        // The predicate RECURSES into non-opaque carriers, so an opaque wrapper
        // nested one level down is still caught.
        let mut i = Interner::new();

        // `List (Decoder Int)`.
        let int_con = con(&mut i, "Int", vec![]);
        let decoder_int = con(&mut i, "Decoder", vec![int_con]);
        let list_decoder = con(&mut i, "List", vec![decoder_int]);
        assert!(
            field_type_nonderivable(&i, &list_decoder),
            "List (Decoder Int) must be gated"
        );

        // `Maybe (Cmd Msg)`.
        let msg_con = con(&mut i, "Msg", vec![]);
        let cmd_msg = con(&mut i, "Cmd", vec![msg_con]);
        let maybe_cmd = con(&mut i, "Maybe", vec![cmd_msg]);
        assert!(
            field_type_nonderivable(&i, &maybe_cmd),
            "Maybe (Cmd Msg) must be gated"
        );
    }

    #[test]
    fn plain_data_types_are_derivable() {
        let mut i = Interner::new();

        // `{ x : Int }` — a plain record field.
        let int_con = con(&mut i, "Int", vec![]);
        assert!(!field_type_nonderivable(&i, &int_con));

        // `List Int` — a non-opaque container of a derivable payload.
        let int_arg = con(&mut i, "Int", vec![]);
        let list_int = con(&mut i, "List", vec![int_arg]);
        assert!(!field_type_nonderivable(&i, &list_int));

        // `Maybe (List String)` — nested derivable data.
        let str_con = con(&mut i, "String", vec![]);
        let list_str = con(&mut i, "List", vec![str_con]);
        let maybe_list_str = con(&mut i, "Maybe", vec![list_str]);
        assert!(!field_type_nonderivable(&i, &maybe_list_str));

        // Type variables and unit are derivable.
        let a = sym(&mut i, "a");
        assert!(!field_type_nonderivable(&i, &canon::Type::Var(a)));
        assert!(!field_type_nonderivable(&i, &canon::Type::Unit));
    }
}
