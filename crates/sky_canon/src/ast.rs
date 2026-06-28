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

use sky_diagnostics::Located;
use sky_intern::Symbol;

/// A name-resolved module (M0 subset).
#[derive(Clone, PartialEq, Eq, Debug)]
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
    pub ctors: Vec<Ctor>,
}

/// A single resolved constructor: name, positional index, and arity.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Ctor {
    pub name: Symbol,
    pub index: usize,
    pub arity: usize,
}

/// A top-level definition. Mirrors `Can.Def` / `Can.TypedDef`: a binding either
/// carries a canonical type annotation (`Typed`) or it does not (`Untyped`).
#[derive(Clone, PartialEq, Eq, Debug)]
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
#[derive(Clone, PartialEq, Eq, Debug)]
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

/// A resolved `let` value binding: `name = body`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LetBinding {
    pub name: Located<Symbol>,
    pub body: Expr,
}

/// One arm of a `case`: a resolved pattern and the body it guards.
#[derive(Clone, PartialEq, Eq, Debug)]
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
}
