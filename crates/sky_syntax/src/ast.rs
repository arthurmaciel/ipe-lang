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
    /// `type alias Name [vars…] = T` declarations. Both the non-parametric form
    /// and the parametric form (`type alias F a = …`) are supported; a parametric
    /// alias carries its declared `vars` here, and canonicalisation substitutes
    /// each use site's type arguments for those parameters before expanding the
    /// body away.
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
    /// Declared type parameters (`a` in `type alias F a = …`). Empty for a
    /// non-parametric alias; a non-empty list is bound, at each use site, to the
    /// type arguments supplied there and substituted into `body` during
    /// canonicalisation. A use site whose argument count differs from this list's
    /// length is a [`sky_diagnostics::NameError::AliasArity`] error.
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
    /// A string literal `"hello"`. The carried [`String`] is the literal's
    /// already-unescaped value (the lexer resolves escape sequences), so the
    /// downstream stages see the runtime string verbatim. Mirrors the Haskell
    /// compiler's `Src.Str`.
    Str(String),
    /// A character literal `'a'`. The carried [`String`] is the source character
    /// text — a single grapheme for an ordinary char, or a backslash-escape pair
    /// (`\n`, `\\`) for an escaped one — matching the Haskell compiler's
    /// `Src.Chr String` representation so the value round-trips to the backend.
    Char(String),
    /// The unit value `()` — the sole inhabitant of the unit type. Built by the
    /// parser from empty parentheses. Mirrors the Haskell compiler's `Src.Unit`.
    Unit,
    /// Function application: callee applied to one or more arguments.
    Call(Box<Expr>, Vec<Expr>),
    /// `case scrutinee of` with `(pattern, body)` arms.
    Case(Box<Expr>, Vec<(Pattern, Expr)>),
    /// An anonymous function `\p0 p1 ... -> body`. The parameter list has arity
    /// ≥ 1 (the parser rejects a zero-parameter `\ -> e`); each parameter is a
    /// pattern (M1 admits a plain variable or `_`). Mirrors the Haskell
    /// compiler's `Src.Lambda [Pattern] Expr`.
    Lambda(Vec<Pattern>, Box<Expr>),
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
    /// A record literal `{ field = expr, ... }`. Fields are `(name, value)`
    /// pairs in source order; the field name is a located lowercase identifier.
    /// Mirrors the Haskell compiler's `Src.Record`, narrowed to the closed-record
    /// (no `{ r | ... }` extension) M1 subset.
    Record(Vec<(Located<Symbol>, Expr)>),
    /// A record field access `record.field` (`record` lowercase). The parser
    /// builds this from a dotted lowercase identifier (`p.x`) and nests it for a
    /// chain (`p.x.y` -> `Access (Access p x) y`). Mirrors `Src.Access`.
    Access(Box<Expr>, Located<Symbol>),
    /// A record update `{ base | field = expr, ... }`. The `base` is a located
    /// lowercase variable naming the record to copy (Sky restricts the update
    /// base to a bare variable, as Elm does); the field list carries each
    /// updated `(name, value)` pair in source order. Mirrors the Haskell
    /// compiler's `Src.Update (Located String) [(Located String, Expr)]`,
    /// narrowed to the closed-record M1 subset.
    Update(Located<Symbol>, Vec<(Located<Symbol>, Expr)>),
}

/// A single `let` binding: `<pat> = body`.
///
/// The binder is a [`Pattern`]: the common `name = body` case is a
/// [`Pattern_::PVar`], and M3b-2 also admits an irrefutable destructure
/// (`(a, b) = e`, `{ x } = e`) as a tuple / record pattern. A refutable binder
/// (a constructor pattern) parses here but is rejected fail-closed downstream —
/// a `let` binder must always match. M1 subset of the Haskell compiler's
/// `Sky.AST.Source.Def`, extended with the destructure form (`DestructDef`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LetBinding {
    pub pat: Pattern,
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
    /// A tuple pattern `(p0, p1, ...)`. Invariant: arity ≥ 2 — a parenthesised
    /// single pattern `(p)` is unwrapped to `p`, and empty parens `()` are not a
    /// pattern. Elements may be variables, wildcards, nested constructor
    /// patterns, or nested tuples. Mirrors the Haskell compiler's tuple pattern.
    PTuple(Vec<Pattern>),
    /// A record pattern `{ x, y }` (M3b-2). Field-pun only: every entry names a
    /// field of the scrutinee record and binds a variable of the same name, so
    /// the binder list is a non-empty set of located field names. Mirrors the
    /// Haskell compiler's `Src.PRecord [A.Located String]` — there is no
    /// `{ field = sub-pattern }` form (the Go reference rejects it at parse), so
    /// a record pattern is always irrefutable. The empty record `{}` is outside
    /// the grammar; a `PRecord` always carries at least one field.
    PRecord(Vec<Located<Symbol>>),
    /// An integer literal pattern `0`, `42` (M3b-3). Refutable. Int is an OPEN
    /// (infinite) type, so a `case` over int literals needs a wildcard / var
    /// catch-all to be exhaustive. Mirrors the Haskell compiler's `Src.PInt`.
    PInt(i64),
    /// A boolean literal pattern `True` / `False` (M3b-3). Refutable in
    /// isolation, but a `True` + `False` pair is an exhaustive cover of `Bool`
    /// (a closed two-constructor type). Mirrors the Haskell compiler's `Src.PBool`.
    PBool(bool),
    /// A character literal pattern `'a'` (M3b-3). The carried [`String`] is the
    /// source character text (single grapheme, or a `\`-escape pair). Refutable;
    /// Char is OPEN. Mirrors the Haskell compiler's `Src.PChr`.
    PChar(String),
    /// A string literal pattern `"hi"` (M3b-3). The carried [`String`] is the
    /// already-unescaped value. Refutable; String is OPEN. Mirrors the Haskell
    /// compiler's `Src.PStr`.
    PStr(String),
    /// An alias / `as` pattern `inner as name` (M3b-3). Matches `inner` and also
    /// binds the whole matched value to `name`. Mirrors the Haskell compiler's
    /// `Src.PAlias Pattern (A.Located String)`.
    PAlias(Box<Pattern>, Located<Symbol>),
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
    /// The unit type `()`.
    TUnit,
    /// An anonymous product (tuple) type `(T1, T2, ...)`. Invariant: arity ≥ 2 —
    /// a 0-tuple is [`Self::TUnit`] and a parenthesised single type `(T)`
    /// unwraps to `T` (neither is a `TTuple`).
    TTuple(Vec<Self>),
    /// A closed record type `{ field : T, ... }`. Fields are `(name, type)` pairs
    /// in source order. The empty record `{}` is outside the grammar, so a
    /// `TRecord` always carries at least one field. Mirrors the Haskell
    /// compiler's `Src.TRecord`, narrowed to the closed-record subset (no row
    /// variable / extension form).
    TRecord(Vec<(Symbol, Self)>),
}
