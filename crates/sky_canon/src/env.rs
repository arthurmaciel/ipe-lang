//! The canonicalisation environment: the name → resolution tables consulted
//! during name resolution. Port of the M0 subset of
//! `Sky.Canonicalise.Environment`.
//!
//! Iteration order is never observable (lookups only), but the tables are
//! `BTreeMap`s so the structure is deterministic regardless of insertion order.

use std::collections::BTreeMap;

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
    #[must_use]
    pub fn initial(home: Vec<Symbol>, interner: &mut Interner) -> Self {
        let mut env = Self {
            home,
            ..Self::default()
        };
        env.install_builtin_vars(interner);
        env.install_prelude_qualifiers(interner);
        env
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

    /// Built-in unqualified variables (from the Prelude). M0 subset of
    /// `Environment.builtinVars`.
    fn install_builtin_vars(&mut self, interner: &mut Interner) {
        let basics = interner.intern("Basics");
        let log = interner.intern("Log");
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
            let key = interner.intern(name);
            let func = interner.intern(func);
            self.vars.insert(key, VarHome::Kernel(module, func));
        }
    }

    /// Auto-qualified prelude kernel modules. M0 subset of
    /// `Environment.preludeQualifiers` — `String.fromInt`, `String.fromFloat`,
    /// etc. resolve without an explicit `import String`.
    fn install_prelude_qualifiers(&mut self, interner: &mut Interner) {
        const QUALIFIERS: &[(&str, &[&str])] = &[
            (
                "String",
                &[
                    "length",
                    "reverse",
                    "append",
                    "split",
                    "join",
                    "contains",
                    "startsWith",
                    "endsWith",
                    "toInt",
                    "fromInt",
                    "toFloat",
                    "fromFloat",
                    "toUpper",
                    "toLower",
                    "trim",
                    "replace",
                    "slice",
                    "isEmpty",
                    "fromChar",
                    "toChar",
                    "repeat",
                    "padLeft",
                    "padRight",
                    "lines",
                    "words",
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
            (
                "Basics",
                &[
                    "identity", "always", "not", "toString", "modBy", "clamp", "fst", "snd",
                    "compare", "negate", "abs", "sqrt", "min", "max",
                ],
            ),
        ];

        for (qual, funcs) in QUALIFIERS {
            let qual_sym = interner.intern(qual);
            let mut module = BTreeMap::new();
            for func in *funcs {
                let func_sym = interner.intern(func);
                module.insert(func_sym, VarHome::Kernel(qual_sym, func_sym));
            }
            self.qual_vars.entry(qual_sym).or_default().extend(module);
        }
    }
}
