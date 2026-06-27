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
    /// `type alias Name = T` declarations. M1 models the non-parametric form
    /// only; a parametric alias (`type alias F a = …`) carries its declared
    /// `vars` here and is rejected at canonicalisation as a not-yet-supported
    /// feature rather than at the parser.
    pub aliases: Vec<Located<TypeAlias>>,
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

/// A `type alias Name = T` declaration.
///
/// Mirrors the Haskell compiler's `Sky.AST.Source.Alias`, narrowed to what M1
/// supports. The aliased type `body` is expanded away at canonicalisation, so no
/// stage after name resolution ever observes the alias name.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TypeAlias {
    pub name: Located<Symbol>,
    /// Declared type parameters (`a` in `type alias F a = …`). Empty for the
    /// supported non-parametric form; a non-empty list is rejected at
    /// canonicalisation ([`sky_diagnostics::Feature::ParametricAliases`]).
    pub vars: Vec<Located<Symbol>>,
    /// The aliased type.
    pub body: Located<TypeAnnotation>,
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
    /// `let <bindings> in <body>`. M1 models simple value bindings only
    /// (`name = expr`); function bindings (`f x = …`) and destructuring
    /// (`(a, b) = …`) are rejected at the parser. The bindings are scoped
    /// sequentially: each value sees the enclosing scope plus the bindings that
    /// precede it (`let*`), matching the non-recursive nested-`Let` IR.
    Let(Vec<LetBinding>, Box<Expr>),
    /// `if cond then a else b`, with optional `else if` branches. The list holds
    /// one or more `(condition, branch)` pairs in source order — the leading
    /// `if` plus every `else if` — followed by the mandatory final `else`
    /// expression. Mirrors the Haskell compiler's `Src.If [(Expr, Expr)] Expr`.
    If(Vec<(Expr, Expr)>, Box<Expr>),
    /// A tuple literal `(e1, e2, ...)`.
    ///
    /// Invariant: the element list has arity ≥ 2. A parenthesised single
    /// expression `(e)` is *not* a tuple — the parser unwraps it to `e` — and
    /// the empty parens `()` are the unit value (unsupported in M1, rejected at
    /// the parser). Mirrors the Haskell compiler's `Src.Tuple e1 e2 [rest]`,
    /// flattened to one vector here.
    Tuple(Vec<Expr>),
}

/// A single `let` value binding: `name = body`. M1 subset of the Haskell
/// compiler's `Sky.AST.Source.Def` (the `Define` variant with no parameters).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LetBinding {
    pub name: Located<Symbol>,
    pub body: Expr,
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
