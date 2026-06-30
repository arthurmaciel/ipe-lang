//! The canonicalisation environment: the name → resolution tables consulted
//! during name resolution. Port of the M0 subset of
//! `Sky.Canonicalise.Environment`.
//!
//! Iteration order is never observable (lookups only), but the tables are
//! `BTreeMap`s so the structure is deterministic regardless of insertion order.

use std::collections::BTreeMap;

use sky_diagnostics::DResult;
use sky_intern::{Interner, Symbol};

/// Where a (possibly qualified) variable resolves to.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum VarHome {
    /// A locally-bound name.
    Local,
    /// A top-level binding of the named module.
    TopLevel(Vec<Symbol>),
    /// A stdlib kernel function: kernel module, function name.
    Kernel(Symbol, Symbol),
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
    pub vars: BTreeMap<Symbol, VarHome>,
    /// Unqualified constructor bindings.
    pub ctors: BTreeMap<Symbol, CtorHome>,
    /// Qualified variable bindings: qualifier → (name → home).
    pub qual_vars: BTreeMap<Symbol, BTreeMap<Symbol, VarHome>>,
}

impl Env {
    /// Build the base environment with Sky's built-in variables and the
    /// auto-qualified prelude kernel modules. The `home` module's top-level
    /// names and unions are registered separately by the caller.
    ///
    /// # Errors
    /// [`sky_diagnostics::Diagnostic::CompilerBug`] if the interner's symbol
    /// table is exhausted while interning the built-in names.
    pub fn initial(home: Vec<Symbol>, interner: &mut Interner) -> DResult<Self> {
        let mut env = Self {
            home,
            ..Self::default()
        };
        env.install_builtin_vars(interner)?;
        env.install_builtin_ctors(interner)?;
        env.install_prelude_qualifiers(interner)?;
        Ok(env)
    }

    /// Register the Prelude-exposed built-in constructors so `Just` / `Nothing` /
    /// `Ok` / `Err` / `True` / `False` resolve as constructors — both as value
    /// expressions and in `case` patterns — without an explicit import. These
    /// belong to the built-in `Maybe a` / `Result e a` / `Bool` types, which have
    /// no user `type` declaration; `home` is left empty (matching how the builtin
    /// type names carry no user module) and `type_name` is the built-in type's
    /// symbol so downstream stages recognise it by name.
    ///
    /// # Errors
    /// [`sky_diagnostics::Diagnostic::CompilerBug`] if the interner is exhausted.
    fn install_builtin_ctors(&mut self, interner: &mut Interner) -> DResult<()> {
        let maybe = interner.intern("Maybe")?;
        let result = interner.intern("Result")?;
        let bool_ = interner.intern("Bool")?;
        // (constructor name, owning built-in type, index within the type, arity).
        for (name, type_name, index, arity) in [
            ("True", bool_, 0, 0),
            ("False", bool_, 1, 0),
            ("Just", maybe, 0, 1),
            ("Nothing", maybe, 1, 0),
            ("Ok", result, 0, 1),
            ("Err", result, 1, 1),
        ] {
            let name = interner.intern(name)?;
            self.ctors.insert(
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

    /// Built-in unqualified variables (from the Prelude). M0 subset of
    /// `Environment.builtinVars`.
    fn install_builtin_vars(&mut self, interner: &mut Interner) -> DResult<()> {
        let basics = interner.intern("Basics")?;
        let log = interner.intern("Log")?;
        for (name, module, func) in [
            ("identity", basics, "identity"),
            ("always", basics, "always"),
            ("not", basics, "not"),
            ("toString", basics, "toString"),
            ("modBy", basics, "modBy"),
            ("clamp", basics, "clamp"),
            ("fst", basics, "fst"),
            ("snd", basics, "snd"),
            ("errorToString", basics, "errorToString"),
            ("println", log, "println"),
        ] {
            let key = interner.intern(name)?;
            let func = interner.intern(func)?;
            self.vars.insert(key, VarHome::Kernel(module, func));
        }
        Ok(())
    }

    /// Auto-qualified prelude kernel modules. M0 subset of
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
                    // ── Haystack-first pure-Sky aliases (compile from source) ──
                    "containsIn",
                    "startsWithIn",
                    "endsWithIn",
                    // ── Legacy entry kept for compatibility ───────────────
                    "toChar",
                ],
            ),
            (
                "Char",
                &[
                    "isAlpha", "isDigit", "isLower", "isUpper", "toLower", "toUpper", "toCode",
                    "fromCode",
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
                    "reverse",
                    "member",
                    "any",
                    "all",
                    "range",
                    "zip",
                    "isEmpty",
                    "cons",
                ],
            ),
            ("Maybe", &["withDefault", "map", "andThen"]),
            ("Result", &["withDefault", "map", "andThen", "mapError"]),
            // `Sky.Core.Math` — `min` / `max` are polymorphic `a -> a -> a`
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
            // `Sky.Core.Dict` — associative map kernels (M4d).
            (
                "Dict",
                &[
                    "empty", "isEmpty", "size", "insert", "get", "remove", "member", "keys",
                    "values", "toList", "fromList", "map", "foldl", "union",
                ],
            ),
            // `Sky.Core.Set` — set kernels (M4d).
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
                ],
            ),
            // `Sky.Core.Bytes` — byte-buffer kernels (M4e).
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
            // `Sky.Core.Encoding` — text encoding helpers (M4f).
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
            // `Sky.Core.Json.Encode` — JSON encoder (M4g).
            (
                "JsonEnc",
                &["string", "int", "float", "bool", "null", "list", "object", "encode"],
            ),
            // `Sky.Core.Json.Decode` — JSON decoder combinators (M4h).
            (
                "JsonDec",
                &[
                    "string", "int", "float", "bool", "decodeString", "field", "at", "index",
                    "list", "map", "andThen", "succeed", "fail", "oneOf", "map2", "map3", "map4",
                ],
            ),
            // `Sky.Core.Json.Decode.Pipeline` — pipeline-style record decoders (M4h).
            (
                "JsonDecP",
                &["required", "optional", "custom", "requiredAt"],
            ),
        ];

        for (qual, funcs) in QUALIFIERS {
            let qual_sym = interner.intern(qual)?;
            let mut module = BTreeMap::new();
            for func in *funcs {
                let func_sym = interner.intern(func)?;
                module.insert(func_sym, VarHome::Kernel(qual_sym, func_sym));
            }
            self.qual_vars.entry(qual_sym).or_default().extend(module);
        }
        Ok(())
    }
}
