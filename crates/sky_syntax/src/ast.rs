//! Source AST — the raw parse tree before name resolution.
//!
//! This is the Rust port of the Milestone-0 subset of the Haskell compiler's
//! `Sky.AST.Source` (which is itself a derivative work of elm/compiler's
//! `AST.Source`, BSD-3-Clause). Only the nodes the M0 golden program exercises
//! are modelled; the broader source grammar (records, tuples, lambdas, lists,
//! `let`/`if`, FFI imports, multiline strings, type aliases, infix decls) is
//! deliberately omitted until a later milestone needs it.
//!
//! Identifiers are interned [`Symbol`]s; every node that has a source location
//! is wrapped in [`Located`].

use sky_diagnostics::Located;
use sky_intern::Symbol;

/// A parsed module (M0 subset).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Module {
    /// Dotted module-name segments, e.g. `Main` → `[Main]`.
    pub name: Located<Vec<Symbol>>,
    /// The `exposing (...)` clause of the `module` header.
    pub exposing: Located<Exposing>,
    pub imports: Vec<Import>,
    pub values: Vec<Located<Value>>,
    pub unions: Vec<Located<Union>>,
}

/// Export / import exposing specification.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Exposing {
    /// `exposing (..)`
    All,
    /// `exposing (a, B, C(..))`
    List(Vec<Located<Exposed>>),
}

/// A single entry in an `exposing (...)` list.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Exposed {
    /// A value or function name.
    Value(Symbol),
    /// A type name plus the privacy of its constructors.
    Type(Symbol, Privacy),
}

/// Constructor visibility for an exposed type.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Privacy {
    /// `Type(..)` — all constructors exposed.
    Public,
    /// `Type` — opaque, no constructors exposed.
    Private,
    /// `Type(A, B)` — a selected subset of constructors exposed.
    PublicCtors(Vec<Symbol>),
}

/// An `import` declaration.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Import {
    /// Dotted module-name segments, e.g. `Sky.Core.Prelude`.
    pub name: Located<Vec<Symbol>>,
    /// Optional `as Alias`.
    pub alias: Option<Symbol>,
    pub exposing: Located<Exposing>,
}

/// A top-level value / function declaration.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Value {
    pub name: Located<Symbol>,
    /// Argument patterns (empty for a plain value binding).
    pub patterns: Vec<Pattern>,
    pub body: Expr,
    /// Optional `name : T` type annotation.
    pub type_annotation: Option<Located<TypeAnnotation>>,
}

/// A `type` (union) declaration.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Union {
    pub name: Located<Symbol>,
    /// Type variables, e.g. `a` in `type Maybe a`.
    pub vars: Vec<Located<Symbol>>,
    pub ctors: Vec<Located<Ctor>>,
}

/// A single union constructor.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Ctor {
    pub name: Symbol,
    pub args: Vec<TypeAnnotation>,
}

/// An expression with its source span.
pub type Expr = Located<Expr_>;

/// Expression node (M0 subset).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Expr_ {
    /// An unqualified name reference (local var, top-level binding, or
    /// constructor used as a value — name resolution decides which).
    VarLocal(Symbol),
    /// A qualified name reference `Qualifier.name`.
    VarQual(Symbol, Symbol),
    /// An integer literal.
    Int(i64),
    /// Function application: callee applied to one or more arguments.
    Call(Box<Expr>, Vec<Expr>),
    /// `case scrutinee of` with `(pattern, body)` arms.
    Case(Box<Expr>, Vec<(Pattern, Expr)>),
    /// A precedence-climbed binary-operator chain: a sequence of
    /// `(operand, operator)` pairs followed by the final operand.
    Binops(Vec<(Expr, Located<Symbol>)>, Box<Expr>),
}

/// A pattern with its source span.
pub type Pattern = Located<Pattern_>;

/// Pattern node (M0 subset).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Pattern_ {
    /// The wildcard `_`.
    PAnything,
    /// A variable binding.
    PVar(Symbol),
    /// A constructor pattern: name, dotted module segments, sub-patterns.
    PCtor(Symbol, Vec<Symbol>, Vec<Pattern>),
}

/// Type-annotation node (M0 subset).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TypeAnnotation {
    /// An arrow type `A -> B`.
    TLambda(Box<Self>, Box<Self>),
    /// A type variable, e.g. `a`.
    TVar(Symbol),
    /// A type constructor application: module qualifier, dotted name segments,
    /// type arguments. The qualifier is the empty symbol for unqualified types.
    TType(Symbol, Vec<Symbol>, Vec<Self>),
}
