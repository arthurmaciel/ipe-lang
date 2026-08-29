//! The Prelude built-in unions — the SINGLE source of truth for every
//! constructor that resolves without a user `type` declaration.
//!
//! `Maybe`/`Result`/`Bool`/`Order`, the Db `SqlValue`/`SqlField` ADTs, the
//! `Ipe.Http.Stream` `ChunkEvent`/`StreamId` ADTs, and the `Error`/`ErrorKind`/
//! `ErrorDetails` ADTs all appear in typed Ipê code as if declared, but carry no
//! `type` decl in any Ipê source file. Three stages must agree on their exact
//! constructor sets, indices, and arities:
//!
//! * **canon** — name resolution, so `Just` / `SqlString` / `Io` resolve as
//!   constructors (both as expressions and in `case` patterns).
//! * **`types::exhaust`** — usefulness analysis, so a `case` over any built-in
//!   union is checked for exhaustiveness rather than skipped as an
//!   unknown-constructor scrutinee (the drift that shipped E0004 to cargo).
//! * **lower** — the synthesised `EnumDef`s + per-arm coverage backstop.
//!
//! Encoding the set ONCE here and having each stage consume it makes the
//! drifting hand-kept second copy unrepresentable: adding a variant updates all
//! three at once.
//!
//! The declaration order and indices below are load-bearing — they pin the
//! emitted enum variant order the backend and runtime rely on. DO NOT reorder.

use std::collections::BTreeMap;

use ipe_diagnostics::DResult;
use ipe_intern::{Interner, Symbol};

/// One Prelude built-in union: its type name plus its constructors, each with a
/// declaration index and payload arity.
pub struct BuiltinUnion {
    /// The union's type name (e.g. `"Maybe"`, `"ErrorKind"`).
    pub type_name: &'static str,
    /// `(constructor name, declaration index, payload arity)`, in declaration
    /// order. The index is dense per union starting at 0.
    pub ctors: &'static [(&'static str, usize, usize)],
    /// Whether this union participates in `types::exhaust` usefulness analysis
    /// via a union entry. `Bool` is `false`: it is judged through the dedicated
    /// boolean-literal path, so it needs constructor resolution in canon but no
    /// `ctor_to_union` / `union_ctors` entry in exhaust.
    pub exhaust_union: bool,
    /// The kernel-qualifier module through which these constructors are ALSO
    /// reachable qualified (e.g. `Some("Http")` lets `Http.Post` resolve as a
    /// value). `None` for the ambient-only unions (`Just`/`Ok`/`LT`/…), which
    /// resolve unqualified only.
    pub qualified_home: Option<&'static str>,
}

/// Every Prelude built-in union that resolves without a `type` declaration. The
/// single source of truth consumed by canon, `types::exhaust`, and lower.
pub const BUILTIN_UNIONS: &[BuiltinUnion] = &[
    BuiltinUnion {
        type_name: "Bool",
        ctors: &[("True", 0, 0), ("False", 1, 0)],
        exhaust_union: false,
        qualified_home: None,
    },
    BuiltinUnion {
        type_name: "Maybe",
        ctors: &[("Just", 0, 1), ("Nothing", 1, 0)],
        exhaust_union: true,
        qualified_home: None,
    },
    BuiltinUnion {
        type_name: "Result",
        ctors: &[("Ok", 0, 1), ("Err", 1, 1)],
        exhaust_union: true,
        qualified_home: None,
    },
    BuiltinUnion {
        type_name: "Order",
        ctors: &[("LT", 0, 0), ("EQ", 1, 0), ("GT", 2, 0)],
        exhaust_union: true,
        qualified_home: None,
    },
    // ── SqlValue variants ──────────────────────────────────────────────────
    // Index order matches the `StdDbSqlValue` enum emitted by the backend and
    // the `into_sql_param()` dispatch in the runtime.
    BuiltinUnion {
        type_name: "SqlValue",
        ctors: &[
            ("SqlString", 0, 1),
            ("SqlInt", 1, 1),
            ("SqlFloat", 2, 1),
            ("SqlBool", 3, 1),
            ("SqlBytes", 4, 1),
            ("SqlTime", 5, 1),
            ("SqlDecimal", 6, 1),
            ("SqlMoney", 7, 1),
            ("SqlNull", 8, 1),
        ],
        exhaust_union: true,
        qualified_home: None,
    },
    BuiltinUnion {
        type_name: "SqlField",
        ctors: &[("SetField", 0, 1), ("OmitField", 1, 0)],
        exhaust_union: true,
        qualified_home: None,
    },
    // ── ProjectionTerm / ProjectionOperand (Ipe.Db.Store.selectNamed) ────────
    // Index order matches `synthetic_projection_term_enum` / `synthetic_projection_operand_enum`
    // in ipe_lower and the `ProjectionTerm` / `ProjectionOperand` enums in ipe_runtime::db.
    BuiltinUnion {
        type_name: "ProjectionTerm",
        ctors: &[
            ("ColumnTerm", 0, 2),
            ("LiteralTerm", 1, 0),
            ("UpperTerm", 2, 1),
            ("LowerTerm", 3, 1),
            ("CoalesceTerm", 4, 2),
            ("ArithTerm", 5, 3),
        ],
        exhaust_union: true,
        qualified_home: None,
    },
    BuiltinUnion {
        type_name: "ProjectionOperand",
        ctors: &[("OperandColumn", 0, 1), ("OperandLiteral", 1, 0)],
        exhaust_union: true,
        qualified_home: None,
    },
    BuiltinUnion {
        type_name: "ArithOp",
        ctors: &[("ArithAdd", 0, 0), ("ArithSub", 1, 0), ("ArithMul", 2, 0)],
        exhaust_union: true,
        qualified_home: None,
    },
    // ── ChunkEvent / StreamId (Ipe.Http.Stream) ────────────────────────────
    BuiltinUnion {
        type_name: "ChunkEvent",
        ctors: &[("Chunk", 0, 1), ("Done", 1, 0), ("Errored", 2, 1)],
        exhaust_union: true,
        qualified_home: None,
    },
    BuiltinUnion {
        type_name: "StreamId",
        ctors: &[("StreamId", 0, 1)],
        exhaust_union: true,
        qualified_home: None,
    },
    // ── HttpMethod (Ipe.Http) ──────────────────────────────────────────────
    // The closed set of HTTP verbs — make-invalid-states-unrepresentable for
    // the request method. `Http.methodToString` recovers the canonical
    // uppercase string; `Http.methodFromString` is the inbound parse boundary.
    // Index order matches the `HttpMethod` enum in `ipe_runtime::http_client`.
    BuiltinUnion {
        type_name: "HttpMethod",
        ctors: &[
            ("Get", 0, 0),
            ("Post", 1, 0),
            ("Put", 2, 0),
            ("Delete", 3, 0),
            ("Patch", 4, 0),
            ("Head", 5, 0),
            ("Options", 6, 0),
        ],
        exhaust_union: true,
        qualified_home: Some("Http"),
    },
    // ── RedirectPolicy (Ipe.Http) ──────────────────────────────────────────
    // Redirect behaviour for an outbound request, replacing the coupled
    // `followRedirects : Bool` + `maxRedirects : Int` pair. Unlike the verb set,
    // this union is destructured in user code (`case req.redirects of …`), so its
    // constructors are ambient-unqualified (like `Just`/`Ok`) rather than
    // qualified-only. Index order matches the `RedirectPolicy` enum in
    // `ipe_runtime::http_client`.
    BuiltinUnion {
        type_name: "RedirectPolicy",
        ctors: &[("NoRedirects", 0, 0), ("FollowRedirects", 1, 1)],
        exhaust_union: true,
        qualified_home: None,
    },
    // ── Error / ErrorKind / ErrorDetails ───────────────────────────────────
    // `Error : ErrorKind -> ErrorInfo -> Error` — arity 2. The sole constructor
    // shares the type's name.
    BuiltinUnion {
        type_name: "Error",
        ctors: &[("Error", 0, 2)],
        exhaust_union: true,
        qualified_home: None,
    },
    // `ErrorKind` — 11 nullary constructors.
    BuiltinUnion {
        type_name: "ErrorKind",
        ctors: &[
            ("Io", 0, 0),
            ("Network", 1, 0),
            ("Ffi", 2, 0),
            ("Decode", 3, 0),
            ("Timeout", 4, 0),
            ("NotFound", 5, 0),
            ("PermissionDenied", 6, 0),
            ("InvalidInput", 7, 0),
            ("Conflict", 8, 0),
            ("Unavailable", 9, 0),
            ("Unexpected", 10, 0),
        ],
        exhaust_union: true,
        qualified_home: None,
    },
    // `ErrorDetails` — the 5-variant enrichment union carried on
    // `ErrorInfo.details : Maybe ErrorDetails`. Index order matches
    // `ipe_types::constrain`'s ctor scheme registration and the runtime's
    // `IpeErrorDetails` enum.
    BuiltinUnion {
        type_name: "ErrorDetails",
        ctors: &[
            ("FfiPanic", 0, 1),
            ("TypeMismatch", 1, 1),
            ("HttpStatus", 2, 1),
            ("JsonDecode", 3, 1),
            ("Custom", 4, 1),
        ],
        exhaust_union: true,
        qualified_home: None,
    },
];

/// One interned built-in constructor: its owning union's interned type name plus
/// its declaration index and payload arity.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct InternedCtor {
    /// The interned type name of the owning union.
    pub union: Symbol,
    /// Declaration index within the union.
    pub index: usize,
    /// Payload arity.
    pub arity: usize,
}

/// The [`BUILTIN_UNIONS`] table interned once, in the shapes each consumer needs.
/// All maps are derived from the ONE const, so they cannot disagree.
pub struct InternedBuiltins {
    /// Constructor name → its `(union, index, arity)`.
    pub ctor: BTreeMap<Symbol, InternedCtor>,
    /// Union type name → its constructors in declaration order, each paired with
    /// its interned name and arity. Restricted to unions that participate in
    /// exhaustiveness analysis (`exhaust_union == true`).
    pub exhaust_union_ctors: BTreeMap<Symbol, Vec<(Symbol, usize)>>,
}

impl InternedBuiltins {
    /// The interned type-name symbol of the union owning constructor `ctor`, or
    /// `None` when `ctor` is not a built-in.
    #[must_use]
    pub fn union_of(&self, ctor: Symbol) -> Option<Symbol> {
        self.ctor.get(&ctor).map(|c| c.union)
    }
}

/// Intern every built-in union type name and constructor once, returning the
/// lookup tables the consumers need.
///
/// # Errors
/// [`ipe_diagnostics::Diagnostic::CompilerBug`] if the interner's symbol table
/// is exhausted while interning a built-in name.
pub fn intern_builtins(interner: &mut Interner) -> DResult<InternedBuiltins> {
    let mut ctor = BTreeMap::new();
    let mut exhaust_union_ctors = BTreeMap::new();
    for union in BUILTIN_UNIONS {
        let union_sym = interner.intern(union.type_name)?;
        let mut ordered: Vec<(Symbol, usize)> = Vec::with_capacity(union.ctors.len());
        for &(name, index, arity) in union.ctors {
            let name_sym = interner.intern(name)?;
            ctor.insert(
                name_sym,
                InternedCtor {
                    union: union_sym,
                    index,
                    arity,
                },
            );
            ordered.push((name_sym, arity));
        }
        if union.exhaust_union {
            exhaust_union_ctors.insert(union_sym, ordered);
        }
    }
    Ok(InternedBuiltins {
        ctor,
        exhaust_union_ctors,
    })
}

#[cfg(test)]
mod tests {
    use super::{BUILTIN_UNIONS, intern_builtins};
    use ipe_intern::Interner;

    #[test]
    fn every_union_index_is_dense_from_zero() {
        for union in BUILTIN_UNIONS {
            for (pos, &(_, index, _)) in union.ctors.iter().enumerate() {
                assert_eq!(
                    index, pos,
                    "union {} constructor at position {pos} has index {index} — indices must be \
                     dense from 0 in declaration order (emitted enum variant order depends on it)",
                    union.type_name
                );
            }
        }
    }

    #[test]
    fn every_ctor_name_interns_and_is_unique() {
        let mut interner = Interner::new();
        let tables = intern_builtins(&mut interner).expect("intern");
        let total: usize = BUILTIN_UNIONS.iter().map(|u| u.ctors.len()).sum();
        assert_eq!(
            tables.ctor.len(),
            total,
            "every built-in constructor must intern to a distinct symbol — a duplicate name across \
             unions would silently collapse two entries"
        );
    }

    #[test]
    fn exhaust_unions_exclude_bool() {
        let mut interner = Interner::new();
        let tables = intern_builtins(&mut interner).expect("intern");
        let bool_sym = interner.intern("Bool").expect("intern Bool");
        assert!(
            !tables.exhaust_union_ctors.contains_key(&bool_sym),
            "Bool is judged via the dedicated boolean-literal path, not a union entry"
        );
        let maybe_sym = interner.intern("Maybe").expect("intern Maybe");
        assert!(
            tables.exhaust_union_ctors.contains_key(&maybe_sym),
            "Maybe must be an exhaust union"
        );
    }
}
