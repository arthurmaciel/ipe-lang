//! The canonicalisation environment: the name → resolution tables consulted
//! during name resolution. Port of the supported subset of
//! `Ipe.Canonicalise.Environment`.
//!
//! Iteration order is never observable (lookups only), but the tables are
//! `BTreeMap`s so the structure is deterministic regardless of insertion order.

use std::collections::BTreeMap;
use std::rc::Rc;

use ipe_diagnostics::DResult;
use ipe_intern::{Interner, Symbol};
use ipe_kernels::StdlibKernel;

use crate::resolve::ModuleOrigin;

/// Authoritative map from a stdlib module's full import path to its canonical
/// qualifier short-name.
///
/// The key is the module's segment list; the value is the short-name under which
/// that module's members are registered in [`Env::qual_vars`] (see the
/// `QUALIFIERS` table in [`Env::install_prelude_qualifiers`]).
///
/// This is the single source of truth consulted by the canonicaliser when it
/// registers a user's `import Ipe.… as Alias` (or the Elm last-segment default)
/// so the alias resolves to the same kernel members as the canonical qualifier.
/// It is the Rust-port counterpart of the upstream Haskell
/// `Ipe.Canonicalise.Environment.staticKernelModules` (path → canonical name).
///
/// Two invariants keep this table from drifting out of sync with the qualifier
/// registry, both enforced by unit tests:
///
/// * **No dangling target** (`stdlib_module_paths_target_a_known_qualifier`):
///   every `canonical` here is a key of a freshly-built `Env`'s `qual_vars`. A
///   path whose canonical were absent would resolve to `None` (fail-closed) —
///   the alias is simply not registered and the reference surfaces the usual
///   `UnknownModule` at its use site, never a silently-invented empty qualifier.
/// * **Total coverage** (`every_canonical_qualifier_has_an_import_path`): every
///   primary qualifier the registry defines appears here under at least one
///   import path, so a newly-added kernel module cannot ship without a way for
///   users to `import … as Alias` it.
///
/// Only real `Ipe.*` module paths belong here — the first segment must
/// be `Ipe`, matching the guard in `resolve::register_stdlib_import_aliases`.
pub const STDLIB_MODULE_QUALIFIERS: &[(&[&str], &str)] = &[
    // ── Ipe.* pure + effect modules ────────────────────────────────────
    (&["Ipe", "Basics"], "Basics"),
    (&["Ipe", "String"], "String"),
    (&["Ipe", "Char"], "Char"),
    (&["Ipe", "List"], "List"),
    (&["Ipe", "Maybe"], "Maybe"),
    (&["Ipe", "Result"], "Result"),
    (&["Ipe", "Error"], "Error"),
    (&["Ipe", "Math"], "Math"),
    (&["Ipe", "Dict"], "Dict"),
    (&["Ipe", "Set"], "Set"),
    (&["Ipe", "Bytes"], "Bytes"),
    (&["Ipe", "Encoding"], "Encoding"),
    (&["Ipe", "Crypto"], "Crypto"),
    (&["Ipe", "Uuid"], "Uuid"),
    // `Ipe.Secret` — opaque secret-string wrapper.
    (&["Ipe", "Secret"], "Secret"),
    // `Ipe.CssSafety` — the four Ipe.Css leaf security kernels. This
    // is a KERNEL qualifier (imported by the compiled-source `Ipe.Css`); `Ipe.Css`
    // itself stays OUT of this table (it is compiled source, registered in ipe's
    // `COMPILED_STD_MODULES`), so the `compiled_vs_kernel_qualifier_disjoint`
    // invariant holds.
    (&["Ipe", "CssSafety"], "CssSafety"),
    (&["Ipe", "Jwt"], "Jwt"),
    (&["Ipe", "Json", "Encode"], "JsonEnc"),
    (&["Ipe", "Json", "Decode"], "JsonDec"),
    (&["Ipe", "Json", "Decode", "Pipeline"], "JsonDecP"),
    (&["Ipe", "Task"], "Task"),
    (&["Ipe", "Io"], "Io"),
    // `Ipe.Debug` — development-only `Debug.log` escape hatch (kernel-only).
    (&["Ipe", "Debug"], "Debug"),
    (&["Ipe", "Time"], "Time"),
    (&["Ipe", "System"], "System"),
    (&["Ipe", "Random"], "Random"),
    (&["Ipe", "File"], "File"),
    (&["Ipe", "Http"], "Http"),
    // NOTE — `Ipe.Path` and `Ipe.Regex` are DELIBERATELY
    // absent here. They are COMPILED-SOURCE Layer-3 modules (registered in
    // `ipe::stdlib::COMPILED_STD_MODULES`): their members are point-free
    // `Ffi.kernel "Path_*"` / `"Regex_*"` aliases that `detect_kernel_alias`
    // routes to the registered pure `Path*` / `Regex*` `StdlibKernel` variants
    // (runtime: `ipe_runtime::{path,regex_kernel}::*`). A module is EITHER a
    // kernel qualifier here OR compiled-source — never both
    // (`compiled_vs_kernel_qualifier_disjoint`), so these stay out of this table.
    // ── Ipe.Http.* server surface ───────────────────────────────────────────
    (&["Ipe", "Http", "Server"], "Server"),
    (&["Ipe", "Http", "Middleware"], "Middleware"),
    (&["Ipe", "Http", "RateLimit"], "RateLimit"),
    // ── Ipe.* modules ───────────────────────────────────────────────────────
    (&["Ipe", "Log"], "Log"),
    (&["Ipe", "Cmd"], "Cmd"),
    (&["Ipe", "Sub"], "Sub"),
    (&["Ipe", "Db"], "Db"),
    (&["Ipe", "Db", "Decode"], "Db.Decode"),
    (&["Ipe", "Db", "Sql"], "Sql"), // SqlFragment builder
    (&["Ipe", "Ui"], "Ui"),
    (&["Ipe", "Ui", "Background"], "Background"),
    (&["Ipe", "Ui", "Border"], "Border"),
    (&["Ipe", "Ui", "Font"], "Font"),
    (&["Ipe", "Ui", "Region"], "Region"),
    (&["Ipe", "Ui", "Input"], "Input"),
    (&["Ipe", "Ui", "Lazy"], "Lazy"),
    (&["Ipe", "Ui", "Keyed"], "Keyed"), // ipe-key diff identity
    (&["Ipe", "Decimal"], "Decimal"),   // arbitrary-precision decimal arithmetic
    (&["Ipe", "Html"], "Html"),
    (&["Ipe", "Html", "Attributes"], "Attr"),
    (&["Ipe", "Html", "Events"], "Event"),
    // ── Ipe.Tea.<Shape> managed-update-loop shapes (ADR 0048) ────────────────
    // The four TEA shapes live under `Ipe.Tea.*`; the canonical short qualifier
    // ("Web"/"Tui"/…) is preserved so every lower.rs kernel match arm is
    // unchanged. Importing any `Ipe.Tea.*` module marks the module a TEA app —
    // a plain-`main` Program that imports one is rejected (IPE-N0033).
    (&["Ipe", "Tea", "Web"], "Web"),
    (&["Ipe", "Tea", "Tui"], "Tui"),
    (&["Ipe", "Tea", "WebView"], "WebView"),
    // ── Effect stdlib modules ───────────────────────────────────────────────
    (&["Ipe", "Tea", "Console"], "Console"),
    (&["Ipe", "Auth"], "Auth"),
    (&["Ipe", "Auth"], "Auth"),
    (&["Ipe", "Http", "Server", "Stream"], "Stream"),
    (&["Ipe", "Http", "Stream"], "HttpStream"),
    // Ipe.Http.Server.WebSocket (12 kernels).
    (&["Ipe", "Http", "Server", "WebSocket"], "Ws"),
];

/// Where a (possibly qualified) variable resolves to.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum VarHome {
    /// A locally-bound name.
    Local,
    /// A top-level binding of the named module.
    TopLevel(Vec<Symbol>),
    /// A stdlib kernel function.
    ///
    /// `id` is `Some` when the kernel resolves against `stdlib_index` at
    /// parse time (the fast path in `lower_callee`); `None` for entries
    /// present in `qual_vars` but not wired into the registry (the
    /// string-match fallback in `lower_callee`).  `module` and `name` are
    /// always present for diagnostics and that fallback path.
    Kernel(Option<StdlibKernel>, Symbol, Symbol),
}

/// One origin of a wildcard-exposed stdlib value member.
///
/// Records the resolved kernel [`VarHome`] together with the user's import
/// `dep_path` (e.g. `["Ipe", "Html"]`) so an ambiguous bare use can name every
/// contributing module in its diagnostic without re-deriving the path.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct WildcardOrigin {
    /// The kernel home cloned from the canonical qualifier's member table — the
    /// SAME `VarHome::Kernel` a qualified `M.member` reference resolves to, so
    /// lowering is identical whether the call site is qualified or unqualified.
    pub home: VarHome,
    /// The user's import path, used only to render the ambiguity diagnostic.
    pub dep_path: Vec<Symbol>,
}

/// Where a constructor resolves to.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CtorHome {
    pub home: Vec<Symbol>,
    pub type_name: Symbol,
    pub name: Symbol,
    pub index: usize,
    pub arity: usize,
}

/// The name-resolution environment.
#[derive(Clone, Debug, Default)]
pub struct Env {
    /// The module being canonicalised.
    pub home: Vec<Symbol>,
    /// Unqualified variable bindings.
    ///
    /// The one genuinely scope-local table — it stays owned so per-scope
    /// entry (`env.clone()` in `resolve.rs`) copies only the current local
    /// bindings. Every other (large, setup-time-immutable) table below is
    /// behind an `Rc`, making the per-scope clone a refcount bump instead of
    /// a deep copy of the ~600-entry kernel registry (efficiency-audit §5
    /// high). Setup-phase writes go through `Rc::make_mut` (refcount 1 →
    /// in-place, no copy). Same maps read with the same `BTreeMap` ordering →
    /// identical resolution + diagnostic order.
    pub vars: BTreeMap<Symbol, VarHome>,
    /// Unqualified constructor bindings.
    pub ctors: Rc<BTreeMap<Symbol, CtorHome>>,
    /// Qualified variable bindings: qualifier → (name → home).
    pub qual_vars: Rc<BTreeMap<Symbol, BTreeMap<Symbol, VarHome>>>,
    /// Qualified constructor bindings: qualifier → (`ctor_name` → home).
    ///
    /// Populated when `import Foo as Alias` (user-module import) registers
    /// `dep.ctors` under the alias qualifier.  Lets `Alias.CtorName` resolve
    /// to `VarCtor` — needed for compiled-source ADTs like `Ipe.Money`'s
    /// `Currency` constructors accessed as `Money.USD`, `Money.EUR`, etc.
    pub qual_ctors: Rc<BTreeMap<Symbol, BTreeMap<Symbol, CtorHome>>>,
    /// **Low-priority wildcard-exposed stdlib value members.**
    ///
    /// A bare value name maps to the set of stdlib modules that flooded it into
    /// unqualified scope via `import M exposing (..)`, keyed by canonical
    /// qualifier so re-importing the same module (or importing it under an
    /// alias) dedups to a single origin.
    ///
    /// This is a strictly LOWER-priority tier than [`Self::vars`] /
    /// [`Self::ctors`]: [`resolve_var`](crate::resolve) consults it ONLY after a
    /// local, top-level binding, explicit `exposing (name)`, synth record-alias
    /// constructor, or prelude builtin of the same spelling all miss — so any of
    /// those SILENTLY shadow a wildcard member (no `DuplicateValue`, unlike the
    /// explicit-list path). When two or more distinct modules survive for a bare
    /// use, that use is `AmbiguousImport` (IPE-N0024) AT THE USE SITE, never a
    /// silent last-wins.
    pub wildcard_vars: Rc<BTreeMap<Symbol, BTreeMap<Symbol, WildcardOrigin>>>,
    /// **Parse-once registry index.**  Maps `(qualifier_sym, name_sym)`
    /// to the typed [`StdlibKernel`] variant, built anti-drift from
    /// [`StdlibKernel::ALL`] in `install_prelude_qualifiers`.
    ///
    /// Threaded through `VarHome::Kernel`, and exposed here so the
    /// `canon_equals_registry` tripwire test can validate parity with
    /// `qual_vars` without touching any downstream path.
    pub stdlib_index: Rc<BTreeMap<(Symbol, Symbol), StdlibKernel>>,
    /// The module's driver-vouched trust provenance. `Ffi.binding` bodies
    /// resolve ONLY under [`ModuleOrigin::FfiInterface`]; any other origin
    /// falls through to ordinary qualified-name resolution (and fails there —
    /// `Ffi` is not an importable module).
    pub origin: ModuleOrigin,
}

impl Env {
    /// Build the base environment with Ipê's built-in variables and the
    /// auto-qualified prelude kernel modules. The `home` module's top-level
    /// names and unions are registered separately by the caller.
    ///
    /// # Errors
    /// [`ipe_diagnostics::Diagnostic::CompilerBug`] if the interner's symbol
    /// table is exhausted while interning the built-in names.
    pub fn initial(home: Vec<Symbol>, interner: &mut Interner) -> DResult<Self> {
        let mut env = Self {
            home,
            ..Self::default()
        };
        // install_prelude_qualifiers MUST run first — it populates
        // stdlib_index, which install_builtin_vars consults for the fast-path id.
        env.install_prelude_qualifiers(interner)?;
        env.install_builtin_ctors(interner)?;
        env.install_builtin_vars(interner)?;
        Ok(env)
    }

    /// Register the ambient (Tier-B) built-in constructors so `Just` / `Nothing` /
    /// `Ok` / `Err` / `True` / `False` resolve as constructors — both as value
    /// expressions and in `case` patterns — without an explicit import. These
    /// belong to the built-in `Maybe a` / `Result e a` / `Bool` types, which have
    /// no user `type` declaration; `home` is left empty (matching how the builtin
    /// type names carry no user module) and `type_name` is the built-in type's
    /// symbol so downstream stages recognise it by name.
    ///
    /// # Errors
    /// [`ipe_diagnostics::Diagnostic::CompilerBug`] if the interner is exhausted.
    fn install_builtin_ctors(&mut self, interner: &mut Interner) -> DResult<()> {
        // The full built-in constructor set is drawn from the ONE shared table
        // (`crate::builtins::BUILTIN_UNIONS`) that `types::exhaust` and `lower`
        // also consume, so the three can never disagree. Every built-in type
        // carries no user `type` declaration, so `home` is left empty (matching
        // how the built-in type names carry no user module); `type_name` is the
        // built-in type's interned symbol so downstream stages recognise it.
        for union in crate::builtins::BUILTIN_UNIONS {
            let type_name = interner.intern(union.type_name)?;
            for &(name, index, arity) in union.ctors {
                let name = interner.intern(name)?;
                Rc::make_mut(&mut self.ctors).insert(
                    name,
                    CtorHome {
                        home: Vec::new(),
                        type_name,
                        name,
                        index,
                        arity,
                    },
                );
            }
        }
        Ok(())
    }

    /// Bind a name as a local (function parameter / `case` binding).
    pub fn add_local(&mut self, name: Symbol) {
        self.vars.insert(name, VarHome::Local);
    }

    /// Look up an unqualified variable.
    #[must_use]
    pub fn lookup_var(&self, name: Symbol) -> Option<&VarHome> {
        self.vars.get(&name)
    }

    /// Look up an unqualified constructor.
    #[must_use]
    pub fn lookup_ctor(&self, name: Symbol) -> Option<&CtorHome> {
        self.ctors.get(&name)
    }

    /// Look up a qualified variable (`Qualifier.name`).
    #[must_use]
    pub fn lookup_qual_var(&self, qualifier: Symbol, name: Symbol) -> Option<&VarHome> {
        self.qual_vars.get(&qualifier).and_then(|m| m.get(&name))
    }

    /// The member table for a qualifier, or `None` when the qualifier names no
    /// known module/import alias. Lets a caller distinguish an unknown
    /// qualifier from a known qualifier missing the member.
    #[must_use]
    pub fn qual_members(&self, qualifier: Symbol) -> Option<&BTreeMap<Symbol, VarHome>> {
        self.qual_vars.get(&qualifier)
    }

    /// Resolve a stdlib module's full import `path` (segment symbols) to the
    /// canonical qualifier symbol under which its kernel members are registered.
    ///
    /// Consults [`STDLIB_MODULE_QUALIFIERS`] (the single source of truth) and
    /// returns the interned canonical qualifier **only when it actually carries
    /// members** in [`Self::qual_vars`]. A path that names no known stdlib module
    /// — or whose canonical is (defensively) absent from the registry — yields
    /// `None`, so the caller registers nothing and the reference fails closed
    /// with the ordinary `UnknownModule` diagnostic at its use site rather than
    /// resolving to an invented, empty qualifier.
    ///
    /// # Errors
    /// [`ipe_diagnostics::Diagnostic::CompilerBug`] if interning the canonical
    /// name exhausts the interner's symbol table.
    pub fn canonical_stdlib_qualifier(
        &self,
        path: &[Symbol],
        interner: &mut Interner,
    ) -> DResult<Option<Symbol>> {
        // Resolve the path to string segments under an immutable interner borrow
        // and match it against the table. The `&'static str` canonical is owned
        // by the table, so it outlives the borrow released at the end of the
        // block — leaving the interner free for the mutable `intern` below.
        let canonical: Option<&'static str> = {
            let mut segs: Vec<&str> = Vec::with_capacity(path.len());
            for &s in path {
                match interner.resolve(s) {
                    Some(seg) => segs.push(seg),
                    // An un-interned path segment cannot name a known module.
                    None => return Ok(None),
                }
            }
            STDLIB_MODULE_QUALIFIERS
                .iter()
                .find(|(p, _)| p.len() == segs.len() && p.iter().zip(&segs).all(|(a, b)| a == b))
                .map(|(_, canonical)| *canonical)
        };
        match canonical {
            None => Ok(None),
            Some(canon) => {
                let sym = interner.intern(canon)?;
                // Fail-closed: only report a qualifier that actually has members.
                Ok(self.qual_vars.contains_key(&sym).then_some(sym))
            }
        }
    }

    /// Built-in unqualified variables (the Tier-A `Ipe.Basics` surface).
    /// Supported subset of `Environment.builtinVars`.
    ///
    /// Must run AFTER `install_prelude_qualifiers` so `stdlib_index` is
    /// populated and the id fast-path can be threaded in.
    fn install_builtin_vars(&mut self, interner: &mut Interner) -> DResult<()> {
        let basics = interner.intern("Basics")?;
        let error_sym = interner.intern("Error")?;
        for (name, module, func) in [
            ("identity", basics, "identity"),
            ("always", basics, "always"),
            ("not", basics, "not"),
            ("toString", basics, "toString"),
            ("modBy", basics, "modBy"),
            ("clamp", basics, "clamp"),
            ("fst", basics, "fst"),
            ("snd", basics, "snd"),
            // `errorToString` is the Basics-exposed unqualified form of
            // `Error.toString`.  The kernel declaration uses module="Error" /
            // func="toString", so the stdlib_index key is (Error, toString).
            // We must register with the same key so `id` resolves to
            // `Some(StdlibKernel::ErrorToString)` and the type-checker
            // can look up its scheme without hitting IPE-L0108.
            ("errorToString", error_sym, "toString"),
            // ── Basics numerics ─────────────────────────────────────────────
            ("negate", basics, "negate"),
            ("abs", basics, "abs"),
            ("sqrt", basics, "sqrt"),
            ("min", basics, "min"),
            ("max", basics, "max"),
            // ── end Basics numerics ─────────────────────────────────────────
        ] {
            let key = interner.intern(name)?;
            let func_sym = interner.intern(func)?;
            let id = self.stdlib_index.get(&(module, func_sym)).copied();
            self.vars.insert(key, VarHome::Kernel(id, module, func_sym));
        }
        Ok(())
    }

    /// Auto-qualified prelude kernel modules. Supported subset of
    /// `Environment.preludeQualifiers` — `String.fromInt`, `String.fromFloat`,
    /// etc. resolve without an explicit `import String`.
    #[allow(clippy::too_many_lines)] // declarative table — extracting a helper would obscure the data
    fn install_prelude_qualifiers(&mut self, interner: &mut Interner) -> DResult<()> {
        const QUALIFIERS: &[(&str, &[&str])] = &[
            (
                "String",
                &[
                    // ── Arity-1 kernels ───────────────────────────────────
                    "length",
                    "reverse",
                    "isEmpty",
                    "toUpper",
                    "toLower",
                    "casefold",
                    "trim",
                    "trimStart",
                    "trimEnd",
                    "toInt",
                    "fromInt",
                    "toFloat",
                    "fromFloat",
                    "fromChar",
                    "fromList",
                    "concat",
                    "words",
                    "lines",
                    "toList",
                    "isEmail",
                    "isUrl",
                    // ── Arity-2 kernels ───────────────────────────────────
                    "append",
                    "split",
                    "join",
                    "contains",
                    "startsWith",
                    "endsWith",
                    "equalFold",
                    "repeat",
                    "dropLeft",
                    "dropRight",
                    // ── Arity-3 kernels ───────────────────────────────────
                    "replace",
                    "slice",
                    "padLeft",
                    "padRight",
                    // ── Haystack-first pure-Ipê aliases (compile from source) ──
                    "containsIn",
                    "startsWithIn",
                    "endsWithIn",
                    // ── Char-level navigation + fold family ───────────────
                    "left",
                    "right",
                    "cons",
                    "uncons",
                    "pad",
                    "indexes",
                    "map",
                    "filter",
                    "foldl",
                    "foldr",
                    "any",
                    "all",
                    // ── Legacy entry kept for compatibility ───────────────
                    "toChar",
                ],
            ),
            (
                "Char",
                &[
                    "isAlpha",
                    "isDigit",
                    "isLower",
                    "isUpper",
                    "toLower",
                    "toUpper",
                    "toCode",
                    "fromCode",
                    "isAlphaNum",
                    "isHexDigit",
                    "isOctDigit",
                ],
            ),
            (
                "List",
                &[
                    "map",
                    "filter",
                    "foldl",
                    "foldr",
                    "length",
                    "head",
                    "tail",
                    "take",
                    "drop",
                    "append",
                    "concat",
                    "concatMap",
                    "indexedMap",
                    "reverse",
                    "member",
                    "any",
                    "all",
                    "find",
                    "range",
                    "zip",
                    "isEmpty",
                    "cons",
                    // ── List batch ───────────────────────────────────────────
                    "filterMap",
                    "sortBy",
                    "sort",
                    "sortWith",
                    "singleton",
                    "repeat",
                    "sum",
                    "product",
                    "maximum",
                    "minimum",
                    "intersperse",
                    "partition",
                    "unzip",
                    "map2",
                    "map3",
                    "map4",
                    "map5",
                ],
            ),
            (
                "Maybe",
                &[
                    "withDefault",
                    "map",
                    "andThen",
                    "map2",
                    "map3",
                    "map4",
                    "map5",
                    "andMap",
                    "combine",
                ],
            ),
            (
                "Result",
                &[
                    "withDefault",
                    "map",
                    "andThen",
                    "mapError",
                    "map2",
                    "map3",
                    "map4",
                    "map5",
                    "andMap",
                    "combine",
                    "traverse",
                    "toMaybe",
                    "fromMaybe",
                ],
            ),
            // `Ipe.Error` — the real `Error ErrorKind ErrorInfo` ADT.
            // Message constructors + nullary constructors + `toString`
            // render + `withMessage` modifier + `isRetryable` classification +
            // `withDetails` modifier (attaches the
            // `ErrorDetails` union to `ErrorInfo.details : Maybe ErrorDetails`).
            (
                "Error",
                &[
                    "unexpected",
                    "invalidInput",
                    "io",
                    "network",
                    "ffi",
                    "decode",
                    "conflict",
                    "unavailable",
                    "timeout",
                    "notFound",
                    "permissionDenied",
                    "toString",
                    "withMessage",
                    "isRetryable",
                    "withDetails",
                ],
            ),
            // `Ipe.CssSafety` — the four Ipe.Css leaf security kernels:
            // three `String -> Maybe String` parsers + the `String -> String`
            // `<style>`-breakout floor. Imported (and called unqualified) by the
            // compiled-source `Ipe.Css`.
            (
                "CssSafety",
                &[
                    "safeValue",
                    "safePropName",
                    "safeSelector",
                    "stripStyleClose",
                ],
            ),
            // `Ipe.Log` — qualified form (`import Ipe.Log as Log`).
            // `info`/`debug`/`warn`/`error` are backed; the `*With`
            // variants take Stringify-bounded attrs and stay fail-closed
            // (IPE-L0108) until the Stringify obligation is added.
            // `Log` is observability-only — line printing lives in `Ipe.Io`
            // (`Io.println` / `Io.eprintln`).
            (
                "Log",
                &[
                    "info",
                    "debug",
                    "warn",
                    "error",
                    "infoWith",
                    "debugWith",
                    "warnWith",
                    "errorWith",
                ],
            ),
            // `Ipe.Math` — `min` / `max` are polymorphic `a -> a -> a`
            // (Elm `Basics.min`/`max` semantics). Wired in the lowerer to the
            // runtime's generic compare. All other Math kernels have concrete
            // monomorphic types (abs : Int->Int, sqrt : Float->Float, etc.).
            (
                "Math",
                &[
                    "min",
                    "max",
                    // constants
                    "pi",
                    "e",
                    "phi",
                    "sqrt2",
                    "inf",
                    "nan",
                    // arity-1 Float→Bool
                    "isNaN",
                    // arity-1 Int→Int
                    "abs",
                    // arity-1 Float→Float
                    "sqrt",
                    "cbrt",
                    "exp",
                    "exp2",
                    "log",
                    "log2",
                    "log10",
                    "sin",
                    "cos",
                    "tan",
                    "asin",
                    "acos",
                    "atan",
                    "sinh",
                    "cosh",
                    "tanh",
                    "asinh",
                    "acosh",
                    "atanh",
                    // arity-1 Float→Int
                    "floor",
                    "ceil",
                    "round",
                    "trunc",
                    // arity-2 Float→Float→Float
                    "pow",
                    "hypot",
                    "atan2",
                    "mod",
                    "remainder",
                ],
            ),
            (
                "Basics",
                &[
                    "identity", "always", "not", "toString", "modBy", "clamp", "fst", "snd",
                    "compare", "negate", "abs", "sqrt", "min", "max",
                ],
            ),
            // `Ipe.Dict` — associative map kernels.
            (
                "Dict",
                &[
                    "empty",
                    "isEmpty",
                    "size",
                    "insert",
                    "get",
                    "remove",
                    "member",
                    "keys",
                    "values",
                    "toList",
                    "fromList",
                    "map",
                    "foldl",
                    "union",
                    "singleton",
                    "foldr",
                    "filter",
                    "partition",
                    "intersect",
                    "diff",
                    "update",
                ],
            ),
            // `Ipe.Set` — set kernels.
            (
                "Set",
                &[
                    "empty",
                    "size",
                    "insert",
                    "remove",
                    "member",
                    "toList",
                    "fromList",
                    "union",
                    "intersect",
                    "diff",
                    "isEmpty",
                    "singleton",
                    "foldl",
                    "foldr",
                    "map",
                    "filter",
                    "partition",
                ],
            ),
            // `Ipe.Bytes` — byte-buffer kernels.
            (
                "Bytes",
                &[
                    "empty",
                    "length",
                    "isEmpty",
                    "fromString",
                    "toString",
                    "fromHex",
                    "toHex",
                    "fromBase64",
                    "toBase64",
                    "append",
                    "slice",
                ],
            ),
            // `Ipe.Encoding` — text encoding helpers.
            (
                "Encoding",
                &[
                    "base64Encode",
                    "base64Decode",
                    "urlEncode",
                    "urlDecode",
                    "hexEncode",
                    "hexDecode",
                ],
            ),
            // `Ipe.Json.Encode` — JSON encoder.
            (
                "JsonEnc",
                &[
                    "string", "int", "float", "bool", "null", "list", "object", "encode",
                ],
            ),
            // `Ipe.Json.Decode` — JSON decoder combinators.
            (
                "JsonDec",
                &[
                    "string",
                    "int",
                    "float",
                    "bool",
                    "decodeString",
                    "field",
                    "at",
                    "index",
                    "list",
                    "map",
                    "andThen",
                    "succeed",
                    "fail",
                    "oneOf",
                    "map2",
                    "map3",
                    "map4",
                ],
            ),
            // `Ipe.Json.Decode.Pipeline` — pipeline-style record decoders.
            (
                "JsonDecP",
                &["required", "optional", "custom", "requiredAt"],
            ),
            // `Ipe.Crypto` — hashes / HMAC / RSA / AEAD / key-derivation / random.
            (
                "Crypto",
                &[
                    "sha256",
                    "sha512",
                    "sha1",
                    "md5",
                    "hmacSha256",
                    "hmacSha512",
                    "rsaSha256Sign",
                    "rsaSha256Verify",
                    "constantTimeEqual",
                    "aesGcmEncrypt",
                    "aesGcmDecrypt",
                    "chacha20Encrypt",
                    "chacha20Decrypt",
                    "aesKeyFromPassword",
                    "chachaKeyFromPassword",
                    "randomBytes",
                    "randomToken",
                ],
            ),
            // `Ipe.Uuid` — UUID generation and parsing.
            // `v4` and `v7` are arity-0 (bare value); `parse` is arity-1.
            ("Uuid", &["v4", "v7", "parse"]),
            // `Ipe.Secret` — opaque secret-string wrapper.
            // `fromString` is the seal; `reveal` is the single greppable
            // un-parse; `redacted` is the explicit "<redacted>" accessor.
            ("Secret", &["fromString", "reveal", "redacted"]),
            // `Ipe.Jwt` — JWT encode/decode for HS256 and RS256,
            // plus builder API: claims / hs256 / rs256 / subject / issuer /
            // audience / expiresAt / notBefore / issuedAt / jwtId / withClaim /
            // encode / decode.
            (
                "Jwt",
                &[
                    "encodeHs256",
                    "decodeHs256",
                    "encodeRs256",
                    "decodeRs256",
                    // builder API
                    "claims",
                    "hs256",
                    "rs256",
                    "subject",
                    "issuer",
                    "audience",
                    "expiresAt",
                    "notBefore",
                    "issuedAt",
                    "jwtId",
                    "withClaim",
                    "encode",
                    "decode",
                ],
            ),
            // `Ipe.Task` — Task combinators + retry surface.
            (
                "Task",
                &[
                    "succeed",
                    "fail",
                    "map",
                    "map2",
                    "map3",
                    "map4",
                    "map5",
                    "attempt",
                    "andThen",
                    "mapError",
                    "onError",
                    "fromResult",
                    "andThenResult",
                    "sequence",
                    "parallel",
                    "run",
                    "perform",
                    "lazy",
                    // retry surface
                    "retryWith",
                    "linearBackoff",
                    "exponentialBackoff",
                    "withJitter",
                    "retryOn",
                    "withRetryOn",
                    "defaultRetryPolicy",
                    "withMaxAttempts",
                    "withBaseMs",
                    "withKind",
                ],
            ),
            // `Ipe.Io` — I/O effects. `println`/`eprintln` write a line
            // (message + trailing newline) to stdout/stderr respectively.
            (
                "Io",
                &[
                    "readLine",
                    "writeStdout",
                    "writeStderr",
                    "println",
                    "eprintln",
                ],
            ),
            // `Ipe.Debug` — dev-only escape hatch. `log : String -> a -> a`.
            ("Debug", &["log"]),
            // `Ipe.Time` — time effects + TEA tick subscription.
            (
                "Time",
                &[
                    "now",
                    "sleep",
                    "unixMillis",
                    "every",
                    "timeString",
                    // `Ipe.Time` pure calendar helpers (Int -> Bool / Int -> Int -> Int).
                    "isLeapYear",
                    "daysInMonth",
                ],
            ),
            // `Ipe.System` — system effects.
            (
                "System",
                &[
                    "args",
                    "getenv",
                    "getenvOr",
                    "getArg",
                    "getenvInt",
                    "getenvBool",
                    "setenv",
                    "unsetenv",
                    "cwd",
                    "loadEnv",
                    "exit",
                ],
            ),
            // `Ipe.Random` — random effects.
            ("Random", &["int", "float", "choice"]),
            // `Ipe.File` — file effects.
            (
                "File",
                &[
                    "readFile",
                    "writeFile",
                    "exists",
                    "remove",
                    "mkdirAll",
                    "readFileLimit",
                    "readFileBytes",
                    "append",
                    "readDir",
                    "isDir",
                    "tempFile",
                    "tempDir",
                    "copy",
                    "rename",
                    "delete",
                ],
            ),
            // `Ipe.Http` — outbound HTTP client.
            // `get` / `post` / `request` are effect kernels (Task Error
            // HttpResponse); `parseQuery` is a pure kernel (String -> Dict
            // String String); the `with*` builders + `defaultRequest` are ALSO
            // pure kernels (HttpRequest record-update emission in the backend) —
            // cross-module pure-Ipê stdlib calls are not resolved by ipe, so the
            // builders cannot live as pure Ipê in Http.ipe. Every name below is
            // registered so `Http.foo` resolves during name-resolution and lands
            // as `Callee::Kernel` (see lower.rs ("Http", _) arms + constrain.rs
            // kernel_ty Http entries that give each its record type).
            (
                "Http",
                &[
                    "get",
                    "post",
                    "request",
                    "defaultRequest",
                    "withMethod",
                    "withHeader",
                    "withTimeout",
                    "withBody",
                    "withUrl",
                    "withFollowRedirects",
                    "withMaxRedirects",
                    "parseQuery",
                ],
            ),
            // ── TEA Cmd / Sub kernels ───────────────────────────────────────────
            // `Cmd.publish` / `Cmd.publishNoEcho` are backed by runtime
            // `cmd_publish` / `cmd_publish_no_echo` in live/pubsub.rs.
            // `Sub.subscribeTopic` is backed by runtime `sub_subscribe_topic`
            // in live/pubsub.rs; emit path uses the standard N-arg route.
            (
                "Cmd",
                &[
                    "none",
                    "batch",
                    "perform",
                    "map",
                    "publish",
                    "publishNoEcho",
                ],
            ),
            (
                "Sub",
                &[
                    "none",
                    "batch",
                    "every",
                    "map",
                    "subscribeTopic",
                    "subscribeWebSocket",
                ],
            ),
            // ── Ipe.PubSub — Task-shaped top-level publish surface ───────────────
            // NOT TEA-loop machinery: `publish` / `publishNoEcho` return
            // `Task Error Int`, callable wherever a broadcast bus runs (i.e. a
            // process running an `Ipe.Web` live app). Backed by runtime
            // `pubsub_publish` / `pubsub_publish_no_echo` in web/pubsub.rs; these
            // kernels are `class = Web` (see `StdlibKernel::decl`), so a caller
            // outside the TEA loop pulls in the `web`/`live` runtime module, never
            // the `Cmd`/`Sub` (`tea` module) aliases. Registering the qualifier
            // here makes `Ipe.PubSub.publish` a first-class qualified call.
            ("PubSub", &["publish", "publishNoEcho"]),
            // ── Db kernels ──────────────────────────────────────────────────────
            // `Ipe.Db` — database connection + query surface.
            // All effect-returning kernels (Task Error …) and pure helpers
            // (`getString`, `getInt`, `getBool`, `getField`) are registered here.
            // `SqlValue` / `SqlField` ADT constructors are handled by
            // `install_builtin_ctors` above; they are unqualified.
            (
                "Db",
                &[
                    "connect",
                    "open",
                    "close",
                    "execRaw",
                    "exec",
                    "query",
                    "queryDecode",
                    "getString",
                    "getInt",
                    "getBool",
                    "getField",
                    "insertRow",
                    "getById",
                    "updateById",
                    "deleteById",
                    "findOneByField",
                    "findManyByField",
                    "findByConditions",
                    "findWhere",
                    "deleteWhere",
                    "insertFields",
                    "updateFields",
                    "insertFieldsReturning",
                    "withTransaction",
                    "migrate",
                    "defaultMigration",
                ],
            ),
            // `Ipe.Db.Sql` — typed, parameterized WHERE-fragment builder.
            // A `SqlFragment` can only be built through
            // these combinators, so a naive string-concatenated WHERE clause
            // is a type error (`String` where `SqlFragment` is expected) at
            // `Db.findWhere` / `Db.deleteWhere`, not a runtime injection risk.
            (
                "Sql",
                &[
                    "column",
                    "param",
                    "int",
                    "string",
                    "float",
                    "bool",
                    "eq",
                    "ne",
                    "gt",
                    "lt",
                    "gte",
                    "lte",
                    "and",
                    "or",
                    "not",
                    "isNull",
                    "isNotNull",
                    "inList",
                    "like",
                ],
            ),
            // `Ipe.Db.Decode` — row decoder combinators.
            // The qualifier string contains a dot ("Db.Decode") which the parser
            // produces correctly for the 3-segment path `Db.Decode.string` — see
            // ipe_parse::parser::ident_expr (qualifier = init.join(".")).
            (
                "Db.Decode",
                &[
                    "string", "int", "float", "bool", "bytes", "money", "nullable", "map",
                    "andThen", "succeed", "fail", "map2", "map3", "map4", "required", "optional",
                ],
            ),
            // Ipe.Http.Server kernels.
            (
                "Server",
                &[
                    "get",
                    "post",
                    "put",
                    "delete",
                    "any",
                    "api",
                    "static",
                    "listen",
                    "text",
                    "json",
                    "html",
                    "withStatus",
                    "withHeader",
                    "redirect",
                    "param",
                    "queryParam",
                    "header",
                    "getCookie",
                    "body",
                    "path",
                    "method",
                    "cookie",
                    "withCookie",
                ],
            ),
            // Ipe.Http.Middleware kernels.
            (
                "Middleware",
                &[
                    "withCors",
                    "withLogging",
                    "withBasicAuth",
                    "withRateLimit",
                    "withCsrf",
                ],
            ),
            // Ipe.Http.RateLimit kernels.
            ("RateLimit", &["allow"]),
            // ── Ipe.Ui — element / attribute / color / layout builders ──────────
            // `layout` and `layoutWith` are render kernels; the rest are element /
            // attribute / length / color value builders wired as kernel helpers.
            // All names below resolve as `VarHome::Kernel("Ui", name)` so that
            // qualified references like `Ui.column [...]` succeed in the canon phase.
            (
                "Ui",
                &[
                    // ── render kernels ────────────────────────────────────────
                    "layout",
                    "layoutWith",
                    // ── element builders ─────────────────────────────────────
                    "none",
                    "text",
                    "el",
                    "row",
                    "column",
                    "wrappedRow",
                    "grid",
                    "html",
                    // ── attribute builders ───────────────────────────────────
                    "spacing",
                    "padding",
                    "paddingXY",
                    "paddingEach",
                    "width",
                    "height",
                    "centerX",
                    "centerY",
                    "alignLeft",
                    "alignRight",
                    "alignTop",
                    "alignBottom",
                    "pointer",
                    "clip",
                    "clipX",
                    "clipY",
                    "scrollbars",
                    "scrollbarX",
                    "scrollbarY",
                    "gridColumns",
                    "above",
                    "below",
                    "onLeft",
                    "onRight",
                    "inFront",
                    "behind",
                    "onClick",
                    "onSubmit",
                    "onInput",
                    "onChange",
                    "onFocus",
                    "onBlur",
                    "onMouseOver",
                    "onMouseOut",
                    "onKeyDown",
                    "onKeyUp",
                    "onBool",
                    "onFile",
                    "htmlAttribute",
                    "mediaQuery",
                    "breakpoint",
                    "aspectRatio",
                    "aspectRatioWH",
                    "square",
                    "widescreen",
                    "cinemascope",
                    "name",
                    "style",
                    "transitionRaw",
                    "gridTracksRaw",
                    "animateRaw",
                    "onPseudo",
                    "hover",
                    "focus",
                    "focusVisible",
                    "active",
                    "disabled",
                    "mobile",
                    "tablet",
                    "desktop",
                    "darkMode",
                    "lightMode",
                    "reducedMotion",
                    // ── Length builders ─────────────────────────────────────
                    "px",
                    "fill",
                    "fillPortion",
                    "content",
                    "shrink",
                    "minimum",
                    "maximum",
                    "vh",
                    "vw",
                    // ── Color builders ──────────────────────────────────────
                    "rgb",
                    "rgba",
                    "white",
                    "black",
                    "transparent",
                    "colorCss",
                    // ── Other ────────────────────────────────────────────────
                    "paragraph",
                    "textColumn",
                    "image",
                    "link",
                    "button",
                    "input",
                    "form",
                    // ── Ui.describe + desc* constructors ─────────────────────
                    "describe",
                    "descMain",
                    "descNavigation",
                    "descContentInfo",
                    "descComplementary",
                    "descLivePolite",
                    "descLiveAssertive",
                    "descHeading",
                    "descLabel",
                ],
            ),
            // ── Ipe.Ui.Background sub-module ─────────────────────────────────────
            (
                "Background",
                &[
                    "color",
                    "image",
                    "hoverColor",
                    "focusColor",
                    "activeColor",
                    "disabledColor",
                    "linearGradient",
                ],
            ),
            // ── Ipe.Ui.Border sub-module ─────────────────────────────────────────
            (
                "Border",
                &[
                    "width",
                    "widthEach",
                    "color",
                    "rounded",
                    "solid",
                    "dashed",
                    "dotted",
                    "shadow",
                    "glow",
                    "innerShadow",
                    "hoverColor",
                    "focusColor",
                    "activeColor",
                    "hoverWidth",
                    "hoverRounded",
                ],
            ),
            // ── Ipe.Ui.Font sub-module ───────────────────────────────────────────
            (
                "Font",
                &[
                    "color",
                    "family",
                    "size",
                    "weight",
                    "bold",
                    "semiBold",
                    "regular",
                    "light",
                    "extraBold",
                    "black",
                    "italic",
                    "underline",
                    "lineThrough",
                    "noDecoration",
                    "letterSpacing",
                    "wordSpacing",
                    "alignLeft",
                    "alignRight",
                    "alignCenter",
                    "center",
                    "justify",
                    "sansSerif",
                    "serif",
                    "monospace",
                    "hoverColor",
                    "focusColor",
                    "activeColor",
                    "disabledColor",
                    "hoverSize",
                ],
            ),
            // ── Ipe.Ui.Region sub-module ─────────────────────────────────────────
            (
                "Region",
                &[
                    "mainContent",
                    "navigation",
                    "footer",
                    "aside",
                    "heading",
                    "label",
                    "announce",
                    "announceUrgently",
                ],
            ),
            // ── Ipe.Ui.Input sub-module ──────────────────────────────────────────
            (
                "Input",
                &[
                    "labelAbove",
                    "labelBelow",
                    "labelLeft",
                    "labelRight",
                    "labelHidden",
                    "placeholder",
                    "text",
                    "multiline",
                    "email",
                    "username",
                    "search",
                    "currentPassword",
                    "newPassword",
                    "checkbox",
                    "slider",
                    "option",
                    "radio",
                    "radioRow",
                ],
            ),
            // ── Ipe.Ui.Lazy sub-module ───────────────────────────────────────────
            ("Lazy", &["lazy", "lazy2", "lazy3", "lazy4", "lazy5"]),
            // ── Ipe.Ui.Keyed — ipe-key for diff identity ─────────────────────────
            ("Keyed", &["column", "row"]),
            // ── Ipe.Html — typed HTML element / text surface ─────────────────────
            // `render` / `escapeHtml` / `escapeAttr` / `attrToString` are render
            // kernels; all element-builder names create `Html msg` values.
            (
                "Html",
                &[
                    // render kernels
                    "render",
                    "toString",
                    "escapeHtml",
                    "escapeAttr",
                    "attrToString",
                    // text / raw nodes
                    "text",
                    "raw",
                    // generic builder
                    "node",
                    "voidNode",
                    "doctype",
                    "styleNode",
                    "titleNode",
                    // common containers
                    "div",
                    "span",
                    "p",
                    "a",
                    "button",
                    "form",
                    "label",
                    "nav",
                    "section",
                    "article",
                    "header",
                    "footer",
                    "main",
                    "aside",
                    "ul",
                    "ol",
                    "li",
                    "table",
                    "thead",
                    "tbody",
                    "tfoot",
                    "tr",
                    "th",
                    "td",
                    "textarea",
                    "select",
                    "option",
                    "pre",
                    "code",
                    "strong",
                    "em",
                    "small",
                    "fieldset",
                    "legend",
                    "blockquote",
                    "figure",
                    "figcaption",
                    "details",
                    "summary",
                    "dialog",
                    "video",
                    "audio",
                    "canvas",
                    "iframe",
                    "progress",
                    "meter",
                    "script",
                    // headings
                    "h1",
                    "h2",
                    "h3",
                    "h4",
                    "h5",
                    "h6",
                    // void elements
                    "img",
                    "input",
                    "br",
                    "hr",
                    "meta",
                    "link",
                    "area",
                    "base",
                    "col",
                    "embed",
                    "source",
                    "track",
                    "wbr",
                    // document elements
                    "body",
                    "htmlNode",
                    "headNode",
                    "title",
                    // legacy compat aliases
                    "headerNode",
                    "codeNode",
                    "mainNode",
                    "footerNode",
                    "linkNode",
                ],
            ),
            // ── Ipe.Html.Attributes alias ────────────────────────────────────────
            (
                "Attr",
                &[
                    "attribute",
                    "boolAttribute",
                    "style",
                    "class",
                    "id",
                    "type_",
                    "name",
                    "value",
                    "placeholder",
                    "href",
                    "src",
                    "alt",
                    "title",
                    "for_",
                    "checked",
                    "disabled",
                    "readonly",
                    "required",
                    "multiple",
                    "selected",
                    "autofocus",
                    "autocomplete",
                    "tabindex",
                    "rows",
                    "noAttr",
                ],
            ),
            // ── Ipe.Html.Events alias ─────────────────────────────────────────────
            (
                "Event",
                &[
                    "onClick",
                    "onInput",
                    "onChange",
                    "onSubmit",
                    "onFocus",
                    "onBlur",
                    "onMouseOver",
                    "onMouseOut",
                    "onKeyDown",
                    "onKeyUp",
                    "onBool",
                    "onMsg",
                ],
            ),
            // ── Ipe.Web / Ipe.Web app-entry kernels ──────────────────────────────
            (
                "Web",
                &["app", "appHtml", "appRouted", "route", "renderStatic"],
            ),
            // ── Ipe.Tui / Ipe.Tui app-entry kernels ──────────────────────────────
            ("Tui", &["app", "program"]),
            // ── Ipe.WebView / Ipe.WebView app-entry kernel ───────────────────────
            ("WebView", &["app", "appHtml"]),
            // ── Effect stdlib modules ─────────────────────────────────────────────
            // Ipe.Console — line-oriented TEA app-entry (fully wired). — line-oriented TEA app-entry (fully wired).
            ("Console", &["app"]),
            // Ipe.Auth / Ipe.Auth — authentication helpers (fail-closed: no lower
            // arm yet → IPE-L0108 at lower time; canon registration removes N0004).
            (
                "Auth",
                &[
                    "hashPassword",
                    "hashPasswordCost",
                    "verifyPassword",
                    "passwordStrength",
                    "signToken",
                    "verifyToken",
                    "register",
                    "login",
                    "setRole",
                ],
            ),
            // Ipe.Http.Server.Stream — server-side streaming HTTP (fail-closed).
            ("Stream", &["stream", "emit", "finish", "withContentType"]),
            // Ipe.Http.Stream — client-side HTTP streaming (fail-closed).
            ("HttpStream", &["open", "forEachChunk", "close", "chunks"]),
            // Ipe.Decimal — arbitrary-precision decimal arithmetic.
            (
                "Decimal",
                &[
                    "zero",
                    "one",
                    "oneHundred",
                    "fromString",
                    "fromInt",
                    "fromFloat",
                    "fromMinor",
                    "toString",
                    "toStringFixed",
                    "toFloat",
                    "toInt",
                    "toMinor",
                    "add",
                    "sub",
                    "mul",
                    "div",
                    "mod",
                    "neg",
                    "abs",
                    "floor",
                    "ceil",
                    "round",
                    "roundHalfUp",
                    "truncate",
                    "compare",
                    "eq",
                    "neq",
                    "lt",
                    "lte",
                    "gt",
                    "gte",
                    "min",
                    "max",
                    "isZero",
                    "isPositive",
                    "isNegative",
                    "percentOf",
                    "addPercent",
                    "subPercent",
                    "formatWith",
                ],
            ),
            // Ipe.Http.Server.WebSocket (12 kernels).
            (
                "Ws",
                &[
                    "defaultCfg",
                    "withOnConnect",
                    "withOnMessage",
                    "withOnClose",
                    "withOnError",
                    "withMaxMessageBytes",
                    "withOriginPatterns",
                    "upgrade",
                    "sendToClient",
                    "sendBinaryToClient",
                    "broadcast",
                    "closeClient",
                ],
            ),
        ];

        // ── Per-qualifier function name aliases ───────────────────────────────
        // Maps a Ipê-source alias name (e.g. `htmlRender`) to its canonical
        // kernel function name (e.g. `render`) within a qualifier module, so
        // `Html.htmlRender` and `Ipe.Html.htmlRender` both produce
        // `VarKernel { module: html_sym, name: render_sym }` — which lower.rs
        // matches under the same `("Html", "render")` arm.
        //
        // Declared here (before the first `for` statement) to satisfy
        // `clippy::items_after_statements`.
        //
        // MUST be processed BEFORE QUALIFIER_ALIASES (installed below) so that
        // alias entries are included in any qual-to-qual copy.
        const FUNC_ALIASES: &[(&str, &str, &str)] = &[
            // ("qualifier", "alias_name", "canonical_kernel_name")
            ("Html", "htmlRender", "render"),
            ("Html", "htmlEscapeText", "escapeHtml"),
            ("Html", "htmlEscapeAttr", "escapeAttr"),
            ("Html", "htmlAttrToString", "attrToString"),
        ];

        // ── Qualifier module aliases (Ipe.X / Ipê.X → short canonical) ────────
        // Clones every entry from the canonical qualifier's member map into the
        // alias qualifier key. Because each entry already holds
        // `VarHome::Kernel(canonical_sym, fn_sym)` (NOT the alias key's symbol),
        // `resolve_qual_var` in `resolve.rs` produces a `VarKernel` whose
        // `module` field is always the canonical short name ("Html", "Ui", …).
        // lower.rs match arms therefore work unmodified.
        //
        // Declared here (before the first `for` statement) to satisfy
        // `clippy::items_after_statements`.
        const QUALIFIER_ALIASES: &[(&str, &str)] = &[
            // (alias_qualifier, canonical_qualifier)
            ("Ipe.Html", "Html"),
            ("Ipe.Ui", "Ui"),
            ("Ipe.Html.Attributes", "Attr"),
            ("Ipe.Html.Events", "Event"),
            // ── Ipe.Tea.<Shape> shape aliases (ADR 0048) ──────────────────────
            ("Ipe.Tea.Web", "Web"),
            ("Ipe.Tea.Tui", "Tui"),
            ("Ipe.Tea.WebView", "WebView"),
            ("Ipe.Log", "Log"),
            // ── Effect stdlib module aliases ──────────────────────────────────────
            ("Ipe.Tea.Console", "Console"),
            ("Ipe.Auth", "Auth"),
            ("Ipe.Http.Server.Stream", "Stream"),
            ("Ipe.Http.Stream", "HttpStream"),
            // Ipe.Http.Server.WebSocket alias.
            ("Ipe.Http.Server.WebSocket", "Ws"),
            // Ipe.Ui.Input sub-module.
            ("Ipe.Ui.Input", "Input"),
            // Ipe.Ui.Lazy sub-module.
            ("Ipe.Ui.Lazy", "Lazy"),
            // Ipe.Ui.Keyed sub-module.
            ("Ipe.Ui.Keyed", "Keyed"),
            // Ipe.Decimal — arbitrary-precision decimal arithmetic.
            ("Ipe.Decimal", "Decimal"),
        ];

        // Build stdlib_index FIRST so all VarHome::Kernel(id, ..)
        // insertions below can look up the pre-resolved id.
        // Derived from StdlibKernel::ALL + decl() — anti-drift by construction.
        // Skip internal-only qualifiers (e.g. "_internal_").
        for sk in StdlibKernel::ALL {
            let decl = sk.decl();
            if decl.qualifier.starts_with('_') {
                continue; // e.g. "_internal_" — skip
            }
            let qual_sym = interner.intern(decl.qualifier)?;
            let name_sym = interner.intern(decl.name)?;
            Rc::make_mut(&mut self.stdlib_index).insert((qual_sym, name_sym), *sk);
        }

        for (qual, funcs) in QUALIFIERS {
            let qual_sym = interner.intern(qual)?;
            let mut module = BTreeMap::new();
            for func in *funcs {
                let func_sym = interner.intern(func)?;
                // Thread the pre-resolved id into VarHome so
                // lower_callee can use the fast path for registered kernels.
                //
                // `Ipe.Html.Events` (`Event`) resolves to the DEDICATED
                // `Html*` event kernels (`HtmlOnClick` …), which produce
                // `Ipe.Html.Attribute msg` (`html_attr`) — the same nominal type
                // the `Ipe.Html.Attributes` builders and every element builder's
                // `List (html_attr msg)` slot use. (They must NOT alias to
                // the `Ui` event kernels, which produce the `Ipe.Ui.Attribute`
                // variant — that makes `button [ onClick Msg ]` fail to unify.) `onMsg`
                // is the generic alias for `onClick`. All members are registered
                // under `(Event, name)` in `stdlib_index`, so the id is always
                // `Some` and `lower_callee`'s fast path returns the `Html*`
                // kernel directly.
                let (mod_sym, name_sym, id) = if *qual == "Event" {
                    let canonical = if *func == "onMsg" { "onClick" } else { *func };
                    let canon_sym = interner.intern(canonical)?;
                    (
                        qual_sym,
                        canon_sym,
                        self.stdlib_index.get(&(qual_sym, canon_sym)).copied(),
                    )
                } else {
                    (
                        qual_sym,
                        func_sym,
                        self.stdlib_index.get(&(qual_sym, func_sym)).copied(),
                    )
                };
                module.insert(func_sym, VarHome::Kernel(id, mod_sym, name_sym));
            }
            Rc::make_mut(&mut self.qual_vars)
                .entry(qual_sym)
                .or_default()
                .extend(module);
        }

        for (qual, alias, canonical) in FUNC_ALIASES {
            let qual_sym = interner.intern(qual)?;
            let alias_sym = interner.intern(alias)?;
            let canonical_sym = interner.intern(canonical)?;
            // VarHome stores the CANONICAL module + fn symbols so lower.rs
            // match arms (`("Html", "render")`) work without any changes.
            // The id is resolved against the CANONICAL (qual, name) key.
            let id = self.stdlib_index.get(&(qual_sym, canonical_sym)).copied();
            let home = VarHome::Kernel(id, qual_sym, canonical_sym);
            Rc::make_mut(&mut self.qual_vars)
                .entry(qual_sym)
                .or_default()
                .insert(alias_sym, home);
        }

        for (alias, canonical) in QUALIFIER_ALIASES {
            let alias_sym = interner.intern(alias)?;
            let canonical_sym = interner.intern(canonical)?;
            // `.cloned()` releases the shared borrow before the mutable
            // `entry(alias_sym)` borrow — required by the borrow checker.
            if let Some(canonical_members) = self.qual_vars.get(&canonical_sym).cloned() {
                Rc::make_mut(&mut self.qual_vars)
                    .entry(alias_sym)
                    .or_default()
                    .extend(canonical_members);
            }
        }

        Ok(())
    }
}
