//! Source AST — the raw parse tree before name resolution.
//!
//! This is the Rust port of the supported subset of the Haskell compiler's
//! `Ipê.AST.Source` (which is itself a derivative work of elm/compiler's
//! `AST.Source`, BSD-3-Clause). Only the nodes the supported grammar
//! exercises are modelled; unsupported source grammar (FFI imports, infix
//! decls) is deliberately omitted until needed.
//!
//! Identifiers are interned [`Symbol`]s; every node that has a source location
//! is wrapped in [`Located`].

use ipe_diagnostics::Located;
use ipe_intern::Symbol;

/// A parsed module.
//
// `Eq` is not derived: `values` hold [`Value`]s whose bodies may carry an `f64`
// float literal, so the tree is only `PartialEq`.
#[derive(Clone, PartialEq, Debug)]
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
    /// Dotted module-name segments, e.g. `Ipe.Prelude`.
    pub name: Located<Vec<Symbol>>,
    /// Optional `as Alias`.
    pub alias: Option<Symbol>,
    pub exposing: Located<Exposing>,
}

/// A top-level value / function declaration.
//
// `Eq` is not derived: `body` is an [`Expr`], which may carry an `f64` float
// literal and so is only `PartialEq`.
#[derive(Clone, PartialEq, Debug)]
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
/// Mirrors the Haskell compiler's `Ipe.AST.Source.Alias`, narrowed to the
/// supported subset. The aliased type `body` is expanded away at
/// canonicalisation, so no
/// stage after name resolution ever observes the alias name.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TypeAlias {
    pub name: Located<Symbol>,
    /// Declared type parameters (`a` in `type alias F a = …`). Empty for a
    /// non-parametric alias; a non-empty list is bound, at each use site, to the
    /// type arguments supplied there and substituted into `body` during
    /// canonicalisation. A use site whose argument count differs from this list's
    /// length is a [`ipe_diagnostics::NameError::AliasArity`] error.
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

/// Expression node.
///
/// `Eq` is not derived: [`Expr_::Float`] carries an `f64`, which is only
/// `PartialEq`. No consumer keys a map / set on an expression, so `PartialEq`
/// (structural `==`, used in tests) is sufficient.
#[derive(Clone, PartialEq, Debug)]
pub enum Expr_ {
    /// An unqualified name reference (local var, top-level binding, or
    /// constructor used as a value — name resolution decides which).
    VarLocal(Symbol),
    /// A qualified name reference `Qualifier.name`.
    VarQual(Symbol, Symbol),
    /// An integer literal.
    Int(i64),
    /// A floating-point literal `1.5`, `3.0`, `1.5e3`, `2e-2`. The carried
    /// [`f64`] is the parsed value (the lexer resolves the lexeme), mirroring
    /// the Haskell compiler's `Src.Float`.
    Float(f64),
    /// A string literal `"hello"`. The carried [`String`] is the literal's
    /// already-unescaped value (the lexer resolves escape sequences), so the
    /// downstream stages see the runtime string verbatim. Mirrors the Haskell
    /// compiler's `Src.Str`.
    Str(String),
    /// A triple-quoted string `"""..."""`. The carried [`String`] is the RAW
    /// content — the lexer does NOT resolve escape sequences or `{{expr}}`
    /// interpolation markers. The canonicaliser desugars these into a `++` chain
    /// of string literals and `Basics.toString`-wrapped expressions, mirroring
    /// the Haskell compiler's `Src.MultilineStr` → `desugarMultiline` path in
    /// `Ipe.Canonicalise.Expression`.
    MultilineStr(String),
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
    /// pattern (a plain variable or `_`). Mirrors the Haskell
    /// compiler's `Src.Lambda [Pattern] Expr`.
    Lambda(Vec<Pattern>, Box<Expr>),
    /// A precedence-climbed binary-operator chain: a sequence of
    /// `(operand, operator)` pairs followed by the final operand.
    Binops(Vec<(Expr, Located<Symbol>)>, Box<Expr>),
    /// `let <bindings> in <body>`. Models simple value bindings only
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
    /// the empty parens `()` are the unit value (unsupported, rejected at
    /// the parser). Mirrors the Haskell compiler's `Src.Tuple e1 e2 [rest]`,
    /// flattened to one vector here.
    Tuple(Vec<Expr>),
    /// A list literal `[]` / `[a, b, c]`. Elements are in source order; the empty
    /// list carries an empty vector. The cons operator `::` is NOT a `List` node —
    /// it flows through [`Expr_::Binops`] like the other right-associative
    /// operators and is re-associated at canonicalisation. Mirrors the Haskell
    /// compiler's `Src.List`.
    List(Vec<Expr>),
    /// A record literal `{ field = expr, ... }`. Fields are `(name, value)`
    /// pairs in source order; the field name is a located lowercase identifier.
    /// Mirrors the Haskell compiler's `Src.Record`, narrowed to the closed-record
    /// (no `{ r | ... }` extension) subset.
    Record(Vec<(Located<Symbol>, Expr)>),
    /// A record field access `record.field` (`record` lowercase). The parser
    /// builds this from a dotted lowercase identifier (`p.x`) and nests it for a
    /// chain (`p.x.y` -> `Access (Access p x) y`). Mirrors `Src.Access`.
    Access(Box<Expr>, Located<Symbol>),
    /// A record update `{ base | field = expr, ... }`. The `base` is a located
    /// lowercase variable naming the record to copy (Ipê restricts the update
    /// base to a bare variable, as Elm does); the field list carries each
    /// updated `(name, value)` pair in source order. Mirrors the Haskell
    /// compiler's `Src.Update (Located String) [(Located String, Expr)]`,
    /// narrowed to the closed-record subset.
    Update(Located<Symbol>, Vec<(Located<Symbol>, Expr)>),
}

/// A single `let` binding: `<pat> = body`.
///
/// The binder is a [`Pattern`]: the common `name = body` case is a
/// [`Pattern_::PVar`], and an irrefutable destructure
/// (`(a, b) = e`, `{ x } = e`) is admitted as a tuple / record pattern. A
/// refutable binder (a constructor pattern) parses here but is rejected
/// fail-closed downstream — a `let` binder must always match. Subset of the
/// Haskell compiler's `Ipe.AST.Source.Def`, extended with the destructure
/// form (`DestructDef`).
// `Eq` is not derived: `body` is an [`Expr`], only `PartialEq` (float literals).
#[derive(Clone, PartialEq, Debug)]
pub struct LetBinding {
    pub pat: Pattern,
    pub body: Expr,
}

/// A pattern with its source span.
pub type Pattern = Located<Pattern_>;

/// Pattern node.
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
    /// A record pattern `{ x, y }`. Field-pun only: every entry names a
    /// field of the scrutinee record and binds a variable of the same name, so
    /// the binder list is a non-empty set of located field names. Mirrors the
    /// Haskell compiler's `Src.PRecord [A.Located String]` — there is no
    /// `{ field = sub-pattern }` form (the Go reference rejects it at parse), so
    /// a record pattern is always irrefutable. The empty record `{}` is outside
    /// the grammar; a `PRecord` always carries at least one field.
    PRecord(Vec<Located<Symbol>>),
    /// An integer literal pattern `0`, `42`. Refutable. Int is an OPEN
    /// (infinite) type, so a `case` over int literals needs a wildcard / var
    /// catch-all to be exhaustive. Mirrors the Haskell compiler's `Src.PInt`.
    PInt(i64),
    /// A boolean literal pattern `True` / `False`. Refutable in
    /// isolation, but a `True` + `False` pair is an exhaustive cover of `Bool`
    /// (a closed two-constructor type). Mirrors the Haskell compiler's `Src.PBool`.
    PBool(bool),
    /// A character literal pattern `'a'`. The carried [`String`] is the
    /// source character text (single grapheme, or a `\`-escape pair). Refutable;
    /// Char is OPEN. Mirrors the Haskell compiler's `Src.PChr`.
    PChar(String),
    /// A string literal pattern `"hi"`. The carried [`String`] is the
    /// already-unescaped value. Refutable; String is OPEN. Mirrors the Haskell
    /// compiler's `Src.PStr`.
    PStr(String),
    /// An alias / `as` pattern `inner as name`. Matches `inner` and also
    /// binds the whole matched value to `name`. Mirrors the Haskell compiler's
    /// `Src.PAlias Pattern (A.Located String)`.
    PAlias(Box<Pattern>, Located<Symbol>),
    /// A list pattern `[]` / `[a, b, c]`. The empty list is the nil cover;
    /// a fixed-length `[a, b]` matches a list of exactly that length. Mirrors the
    /// Haskell compiler's `Src.PList`.
    PList(Vec<Pattern>),
    /// A cons pattern `head :: tail` — the right-associative list
    /// deconstruction. `(x :: xs)` binds the first element to `head` and the rest
    /// to `tail`. Mirrors the Haskell compiler's `Src.PCons`.
    PCons(Box<Pattern>, Box<Pattern>),
    /// An or-pattern `p1 | p2 | …` — matches if ANY alternative matches. Every
    /// alternative binds the identical set of variables at identical types
    /// (enforced in canon/types); each alternative is an arbitrary sub-pattern
    /// and recurses. Invariant: length ≥ 2 — the parser never wraps a lone
    /// pattern, mirroring the arity-≥-2 invariant on [`Self::PTuple`]. The `|`
    /// binds looser than everything else in a pattern (ctor application, `::`,
    /// `as`), so each alternative is a complete cons/`as` pattern.
    POr(Vec<Pattern>),
}

/// Type-annotation node.
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
    /// in source order. The empty record `{}` is valid and produces an empty
    /// field list. Mirrors the Haskell compiler's `Src.TRecord`, narrowed to the
    /// closed-record subset (no row variable / extension form).
    TRecord(Vec<(Symbol, Self)>),
    /// A row-polymorphic (open) record type `{ r | field : T, ... }`. The row
    /// variable `r` names the unnamed tail of extra fields the record may
    /// carry; the named fields are the ones the annotation constrains. A value
    /// of this type is any record carrying *at least* the listed fields.
    /// Mirrors the Haskell compiler's `Src.TRecord fields (Just rowVar)`.
    TRecordOpen(Symbol, Vec<(Symbol, Self)>),
}
