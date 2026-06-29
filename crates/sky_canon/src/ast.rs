//! Canonical AST — the name-resolved tree the type checker consumes.
//!
//! Rust port of the Milestone-0 subset of the Haskell compiler's
//! `Sky.AST.Canonical` (itself a derivative work of elm/compiler's
//! `AST.Canonical`, BSD-3-Clause). Every variable is fully resolved: a
//! reference is classified as a local binding, a top-level binding of a named
//! module, a stdlib kernel function, or a data constructor. Only the nodes the
//! M0 golden program exercises are modelled.
//!
//! Identifiers are interned [`Symbol`]s; located nodes are wrapped in
//! [`Located`]. Module names are dotted segment vectors (`Main` → `[Main]`).

use sky_diagnostics::{Located, Span};
use sky_intern::Symbol;

/// A name-resolved module (M0 subset).
//
// `Eq` is not derived: `defs` carry [`Expr`] bodies that may hold a float
// literal (only `PartialEq`).
#[derive(Clone, PartialEq, Debug)]
pub struct Module {
    /// Dotted module-name segments, e.g. `Main` → `[Main]`.
    pub name: Vec<Symbol>,
    pub unions: Vec<Union>,
    pub defs: Vec<Def>,
}

/// A resolved union type and its constructors.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Union {
    pub name: Symbol,
    /// The type variables this union quantifies, in declaration order
    /// (`a` in `type Maybe a = …`). Empty for a monomorphic union. The order is
    /// load-bearing: the lowerer carries it through to the IR enum's
    /// `type_params`, where each parameter's position fixes its Rust generic name
    /// and aligns with the positional type arguments at every use site.
    pub vars: Vec<Symbol>,
    pub ctors: Vec<Ctor>,
}

/// A single resolved constructor: name, positional index, arity, declared
/// payload field types, and source span.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Ctor {
    pub name: Symbol,
    pub index: usize,
    pub arity: usize,
    /// The constructor's payload field types in declaration order
    /// (`[a]` for `Just a`, `[Tree, Int, Tree]` for `Node Tree Int Tree`). A
    /// nullary constructor has an empty list; `args.len() == arity`. A field type
    /// variable resolves to a [`Type::Var`] naming one of the union's `vars`.
    pub args: Vec<Type>,
    /// The constructor declaration's source span, for blame on a lowering gap
    /// that concerns the declared field types (e.g. an unsupported field shape).
    pub span: Span,
}

/// A top-level definition. Mirrors `Can.Def` / `Can.TypedDef`: a binding either
/// carries a canonical type annotation (`Typed`) or it does not (`Untyped`).
// `Eq` is not derived: a binding body is an [`Expr`] that may carry a float
// literal (only `PartialEq`).
#[derive(Clone, PartialEq, Debug)]
pub enum Def {
    /// A binding with no type annotation.
    Untyped {
        name: Located<Symbol>,
        patterns: Vec<Pattern>,
        body: Expr,
    },
    /// A binding with an annotation, carrying its free type variables and the
    /// canonicalised annotation type.
    Typed {
        name: Located<Symbol>,
        free_vars: Vec<Symbol>,
        patterns: Vec<Pattern>,
        body: Expr,
        ty: Type,
    },
}

impl Def {
    /// The bound name, regardless of typed/untyped shape.
    #[must_use]
    pub const fn name(&self) -> Located<Symbol> {
        match self {
            Self::Untyped { name, .. } | Self::Typed { name, .. } => *name,
        }
    }
}

/// An expression with its source span.
pub type Expr = Located<Expr_>;

/// Name-resolved expression node (M0 subset).
///
/// `Eq` is not derived: [`Expr_::Float`] carries an `f64` (only `PartialEq`).
#[derive(Clone, PartialEq, Debug)]
pub enum Expr_ {
    /// A locally-bound variable (function parameter, `let`, or `case` binding).
    VarLocal(Symbol),
    /// A top-level binding of a named module.
    VarTopLevel { module: Vec<Symbol>, name: Symbol },
    /// A stdlib kernel function. `module` is the kernel module string (e.g.
    /// `Log`, `Basics`, `String`), `name` the function (e.g. `println`).
    VarKernel { module: Symbol, name: Symbol },
    /// A data constructor used as a value.
    VarCtor {
        home: Vec<Symbol>,
        type_name: Symbol,
        name: Symbol,
        index: usize,
    },
    /// An integer literal.
    Int(i64),
    /// A floating-point literal `1.5`, `3.0`, `1.5e3` — carries its parsed value.
    Float(f64),
    /// A string literal `"hello"` — carries its already-unescaped value.
    Str(String),
    /// A character literal `'a'` — carries its single unescaped character's text.
    Char(String),
    /// The unit value `()` — the sole inhabitant of the unit type. Introduces no
    /// bindings and resolves no names.
    Unit,
    /// Function application.
    Call(Box<Expr>, Vec<Expr>),
    /// `case scrutinee of` with resolved arms.
    Case(Box<Expr>, Vec<CaseBranch>),
    /// An anonymous function `\p0 p1 ... -> body`. The parameter patterns are
    /// resolved (each variable becomes a local in `body`); any free variable in
    /// `body` that is not a parameter is captured from the enclosing scope by
    /// ordinary name resolution. Arity ≥ 1 (the parser rejects `\ -> e`).
    Lambda(Vec<Pattern>, Box<Expr>),
    /// A resolved binary operation. `op` is the source operator symbol;
    /// `home` / `func` is the kernel it resolves to (e.g. `Basics` / `add`).
    Binop {
        op: Symbol,
        home: Symbol,
        func: Symbol,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    /// `let <bindings> in <body>`. Bindings are scoped sequentially: each value
    /// was resolved against the enclosing scope plus the bindings before it, and
    /// every binding name is a local within `body`.
    Let(Vec<LetBinding>, Box<Expr>),
    /// `if cond then a else b`, with optional `else if` branches. The list holds
    /// one or more `(condition, branch)` pairs (the leading `if` plus every
    /// `else if`), followed by the mandatory final `else` expression. Every
    /// sub-expression is resolved against the same enclosing scope — `if`
    /// introduces no bindings.
    If(Vec<(Expr, Expr)>, Box<Expr>),
    /// A tuple literal `(e1, e2, ...)`. Invariant: arity ≥ 2 (a parenthesised
    /// single expression was unwrapped by the parser). Every element is resolved
    /// against the same enclosing scope — a tuple introduces no bindings.
    Tuple(Vec<Expr>),
    /// A list literal `[]` / `[a, b, c]`. Elements are resolved in source order;
    /// the empty list carries an empty vector. Introduces no bindings.
    List(Vec<Expr>),
    /// A cons `head :: tail` — the right-associative list-prepend. `head` is an
    /// element; `tail` is a list. The parser's `::` operator chain is
    /// re-associated to this node at canonicalisation.
    Cons(Box<Expr>, Box<Expr>),
    /// A record literal `{ field = value, ... }`. Fields are `(name, value)`
    /// pairs; the name is a label (not a resolvable reference), the value is
    /// resolved against the enclosing scope. A record introduces no bindings.
    Record(Vec<(Symbol, Expr)>),
    /// A record field access `record.field`. The record sub-expression is
    /// resolved against the enclosing scope; the field is a label.
    Access(Box<Expr>, Symbol),
    /// A record update `{ base | field = value, ... }`. The base is the resolved
    /// reference to the record being copied (a variable, in the M1 grammar); the
    /// field list carries each updated `(name, value)` pair, names being labels
    /// and values resolved against the enclosing scope. An update introduces no
    /// bindings.
    Update(Box<Expr>, Vec<(Symbol, Expr)>),
}

/// A resolved `let` binding: `<pat> = body`.
///
/// The binder is a resolved [`Pattern`]: a [`Pattern_::PVar`] for the common
/// `name = body` case, or an irrefutable tuple / record destructure (M3b-2). A
/// refutable binder is rejected fail-closed at lowering.
// `Eq` is not derived: `body` is an [`Expr`], only `PartialEq` (float literals).
#[derive(Clone, PartialEq, Debug)]
pub struct LetBinding {
    pub pat: Pattern,
    pub body: Expr,
}

/// One arm of a `case`: a resolved pattern and the body it guards.
//
// `Eq` is not derived: `body` is an [`Expr`], only `PartialEq` (float literals).
#[derive(Clone, PartialEq, Debug)]
pub struct CaseBranch {
    pub pat: Pattern,
    pub body: Expr,
}

/// A pattern with its source span.
pub type Pattern = Located<Pattern_>;

/// Name-resolved pattern node (M0 subset).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Pattern_ {
    /// The wildcard `_`.
    PAnything,
    /// A variable binding.
    PVar(Symbol),
    /// A resolved constructor pattern.
    PCtor {
        home: Vec<Symbol>,
        type_name: Symbol,
        name: Symbol,
        index: usize,
        args: Vec<Pattern>,
    },
    /// A tuple pattern `(p0, p1, ...)`. Invariant: arity ≥ 2. Each element is a
    /// resolved sub-pattern; every variable it introduces is bound as a local.
    PTuple(Vec<Pattern>),
    /// A record pattern `{ x, y }` (M3b-2). Field-pun only: each entry is a
    /// located field name that also binds a local of the same name. Always
    /// irrefutable; carries at least one field.
    PRecord(Vec<Located<Symbol>>),
    /// An integer literal pattern `0` (M3b-3). Refutable; Int is OPEN.
    PInt(i64),
    /// A boolean literal pattern `True` / `False` (M3b-3). A `True` + `False`
    /// pair is an exhaustive cover of the closed `Bool` type.
    PBool(bool),
    /// A character literal pattern `'a'` (M3b-3) — carries the single unescaped
    /// character's text. Refutable; Char is OPEN.
    PChar(String),
    /// A string literal pattern `"hi"` (M3b-3) — carries the unescaped value.
    /// Refutable; String is OPEN.
    PStr(String),
    /// An alias / `as` pattern `inner as name` (M3b-3): matches `inner` and also
    /// binds the whole matched value to `name`.
    PAlias(Box<Pattern>, Located<Symbol>),
    /// A list pattern `[]` / `[a, b, c]` (M4a). The empty list is the nil cover;
    /// a fixed-length list matches exactly that many elements.
    PList(Vec<Pattern>),
    /// A cons pattern `head :: tail` (M4a) — binds the first element and the rest.
    PCons(Box<Pattern>, Box<Pattern>),
}

/// Canonical type (M0 subset). Mirrors `Can.Type` narrowed to arrows, type
/// variables, and constructor applications.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Type {
    /// An arrow `A -> B`.
    Lambda(Box<Self>, Box<Self>),
    /// A type variable.
    Var(Symbol),
    /// A type-constructor application. `home` is the defining module (empty for
    /// built-ins like `Int`); `name` the type name; `args` its arguments.
    Con {
        home: Vec<Symbol>,
        name: Symbol,
        args: Vec<Self>,
    },
    /// The unit type `()`.
    Unit,
    /// An anonymous product (tuple) type `(T1, T2, ...)`. Invariant: arity ≥ 2 —
    /// a 0-tuple is [`Self::Unit`] and a 1-tuple is just its element.
    Tuple(Vec<Self>),
    /// A closed record type `{ field : T, ... }`. Fields are `(name, type)` pairs
    /// in source order; the empty record is outside the grammar so the list is
    /// non-empty. A field type variable participates in the binding's
    /// quantification exactly like one in any other position (M2c).
    Record(Vec<(Symbol, Self)>),
}
