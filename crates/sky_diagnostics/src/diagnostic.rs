//! Typed compiler diagnostics. No stage ever returns a `String` error; every
//! failure is one of these enums. `CompilerBug.detail` is the only free-form
//! `String`, reserved for "this should never happen" contract violations.
//!
//! Payloads are **owned and structured**: a producer resolves every `Symbol`
//! and zonks every type at the failure point, so the reporter needs neither the
//! interner nor the type arena. Messages are rendered from data, never from a
//! pre-formatted string. Variants are **additive**: the coarse variants from
//! Milestone 0 (`ParseError::Unexpected`, `NameError::Unknown`, …) are retained
//! so existing call sites keep compiling while producers migrate.

use crate::code::{
    Code, SKY_I0001, SKY_I0010, SKY_I0011, SKY_I0100, SKY_I0101, SKY_I0102, SKY_I0103, SKY_I0200,
    SKY_I0201, SKY_I0202, SKY_I0203, SKY_L0100, SKY_L0101, SKY_L0102, SKY_L0103, SKY_L0104,
    SKY_L0105, SKY_L0106, SKY_L0107, SKY_L0108, SKY_L0110, SKY_L0111, SKY_L0112, SKY_L0113,
    SKY_L0114, SKY_L0115, SKY_L0116, SKY_L0117, SKY_L0118, SKY_L0119, SKY_L0120, SKY_L0121,
    SKY_L0122, SKY_L0123, SKY_L0124, SKY_L0125, SKY_L0126, SKY_L0127, SKY_L0128, SKY_L0200,
    SKY_N0001,
    SKY_N0002, SKY_N0003, SKY_N0004, SKY_N0005, SKY_N0010, SKY_N0011, SKY_N0012, SKY_N0013,
    SKY_N0020, SKY_N0021, SKY_N0022, SKY_N0023, SKY_N0024, SKY_N0025, SKY_N0026, SKY_N0027,
    SKY_P0001,
    SKY_P0002, SKY_P0003, SKY_P0010, SKY_P0011, SKY_P0012, SKY_P0013, SKY_P0014, SKY_P0015,
    SKY_P0016, SKY_P0017, SKY_P0020, SKY_P0021, SKY_P0030, SKY_P0031, SKY_P0040, SKY_P0041,
    SKY_P0050, SKY_P0060, SKY_P0061, SKY_P0062, SKY_T0001, SKY_T0002, SKY_T0003, SKY_T0004,
    SKY_T0010, SKY_T0011, SKY_T0012, SKY_T0013, SKY_T0014, SKY_T0015, SKY_T0016, Severity,
};
use crate::span::Span;

// ===========================================================================
// Plain-old-data payload enums
// ===========================================================================

/// A lexical token category, with no payload — the structural shape a parser
/// reports as "found" without carrying the lexeme.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum TokenKind {
    Module,
    Import,
    Exposing,
    As,
    Type,
    Case,
    Of,
    Let,
    In,
    If,
    Then,
    Else,
    LParen,
    RParen,
    LBrace,
    RBrace,
    /// `[` — opens a list literal / pattern.
    LBracket,
    /// `]` — closes a list literal / pattern.
    RBracket,
    /// `::` — the list cons operator / pattern head.
    ColonColon,
    Equals,
    Pipe,
    Colon,
    Arrow,
    /// A lambda lead-in `\`.
    Backslash,
    DotDot,
    /// A lone `.` field-access operator (`(r).field`).
    Dot,
    Comma,
    Underscore,
    Plus,
    /// The append operator `++`.
    PlusPlus,
    Minus,
    Star,
    Slash,
    SlashEq,
    /// `//` — the integer-division operator.
    SlashSlash,
    EqEq,
    Lt,
    Gt,
    Le,
    Ge,
    AmpAmp,
    PipePipe,
    /// The forward-pipe operator `|>`.
    PipeGt,
    /// The backward-pipe operator `<|`.
    LtPipe,
    Ident,
    Int,
    /// A floating-point literal `1.5`, `3.0`, `1.5e3`.
    Float,
    /// A string literal `"…"`.
    Str,
    /// A character literal `'…'`.
    Char,
    /// End of input.
    Eof,
}

/// A single grammatical item the parser expected at the failure position.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Expected {
    ModuleKeyword,
    ExposingKeyword,
    OfKeyword,
    InKeyword,
    ThenKeyword,
    ElseKeyword,
    Equals,
    Arrow,
    Pipe,
    Comma,
    Colon,
    LParen,
    RParen,
    RBrace,
    Identifier,
    Constructor,
    TypeAtom,
    Expression,
    Pattern,
}

/// The set of items that would have been accepted where a token was rejected.
/// Owned and bounded; ordered by the producer for deterministic rendering.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ExpectedSet(pub Box<[Expected]>);

/// The enclosing construct a parser was inside when input ran out or nesting
/// tripped a guard.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Construct {
    ModuleHeader,
    ExposingList,
    Definition,
    TypeDeclaration,
    CaseBranch,
    Type,
    ParenGroup,
    Expression,
    Pattern,
    Let,
    If,
    Tuple,
    Record,
    Lambda,
}

/// Which part of a module header is malformed.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum HeaderDefect {
    /// The file does not begin with `module`.
    NotModuleKeyword,
    /// The module name is missing.
    MissingName,
    /// The module name is not an identifier.
    NameNotIdentifier,
    /// The `exposing` keyword is missing.
    MissingExposing,
}

/// Which part of an `exposing (...)` list is malformed.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ExposingDefect {
    /// The opening `(` is missing.
    MissingOpenParen,
    /// An item separator is neither `,` nor `)`.
    BadSeparator,
    /// An exposed name is not an identifier.
    NameNotIdentifier,
    /// A `Type(..)` constructor list is malformed.
    MalformedCtorList,
}

/// Which part of a `type` declaration is malformed.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum TypeDeclDefect {
    /// The type name is missing.
    MissingName,
    /// The `=` before the constructors is missing.
    MissingEquals,
    /// A constructor name does not start with an uppercase letter.
    CtorNotUppercase,
    /// A constructor name is not an identifier.
    CtorNotIdentifier,
}

/// Which part of a `case … of` expression is malformed.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum CaseDefect {
    /// `of` is missing after the scrutinee.
    MissingOf,
    /// A branch is missing its `->`.
    MissingArrow,
    /// The case has zero branches.
    NoBranches,
    /// The first branch is not indented past `case`.
    FirstBranchNotIndented,
}

/// Which part of a `let … in` expression is malformed.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum LetDefect {
    /// `let` is immediately followed by `in` (or end of input) — no bindings.
    NoBindings,
    /// A binding name is not a lowercase identifier.
    BindingNameNotLower,
    /// A binding name is not followed by `=` or any binder-atom-start token.
    /// This covers e.g. `let x 2 in x`, where a literal sits where `=` was
    /// expected (function parameters `let f x = …` are desugared, not rejected).
    MissingEquals,
    /// The `in` keyword is missing after the bindings.
    MissingIn,
}

/// Which part of an `if … then … else …` expression is malformed.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum IfDefect {
    /// A condition is missing (e.g. `if then …` or end of input where the
    /// condition was expected).
    MissingCondition,
    /// The `then` keyword is missing after a condition.
    MissingThen,
    /// The `else` keyword is missing after a `then` branch.
    MissingElse,
}

// ===========================================================================
// Owned, pretty-printable type document
// ===========================================================================

/// A fully-resolved, interner-free rendering of a type.
///
/// Built by the type checker at the failure point (it zonks the type and
/// resolves every name), so the reporter renders this without touching
/// `VarId`s or the interner.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TyDoc {
    /// A type constructor application, e.g. `List Int` or `Sky.Core.Maybe a`.
    Con {
        module: Box<str>,
        name: Box<str>,
        args: Box<[Self]>,
    },
    /// A function type `a -> b`.
    Fun(Box<Self>, Box<Self>),
    /// A type variable, e.g. `a`.
    Var(Box<str>),
    /// The unit type `()`.
    Unit,
    /// An anonymous product (tuple) type `(T1, T2, ...)`. Invariant: arity ≥ 2.
    Tuple(Box<[Self]>),
    /// A closed record type `{ x : Int, y : Bool }`. Fields are `(name, type)`
    /// pairs in field-name order (the producer sorts them, so rendering is
    /// deterministic).
    Record(Box<[(Box<str>, Self)]>),
}

// ===========================================================================
// Stage error enums (additive)
// ===========================================================================

/// Errors raised during lexing / parsing.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ParseError {
    /// Coarse "a token was unexpected" (Milestone 0 — retained additively).
    Unexpected,
    /// Coarse "recursion-depth guard tripped" (Milestone 0 — retained).
    TooDeep,
    /// A token was found where the grammar wanted one of `expected`. [SKY-P0001]
    UnexpectedToken {
        found: TokenKind,
        expected: ExpectedSet,
    },
    /// Input ended while `construct` still required more tokens. [SKY-P0002]
    UnexpectedEof { construct: Construct },
    /// Nesting of `construct` exceeded `limit`. [SKY-P0003]
    NestingTooDeep { construct: Construct, limit: u16 },
    /// A byte that is not a recognised M0 character. [SKY-P0010]
    UnknownChar(char),
    /// A lone `.` not part of `..` or a qualified name. [SKY-P0011]
    StrayDot,
    /// A digit immediately followed by an identifier character. [SKY-P0012]
    NumberJoinedToName(char),
    /// An integer literal that does not fit in `i64`. [SKY-P0013]
    IntLiteralOutOfRange,
    /// A float literal whose magnitude overflows `f64` to infinity
    /// (e.g. `1e400`). [SKY-P0016]
    FloatLiteralOutOfRange,
    /// A string literal `"…` whose closing `"` is missing before end of input
    /// (or before the line ends). [SKY-P0014]
    UnterminatedString,
    /// A character literal `'…` that is malformed — unterminated, empty (`''`),
    /// or carrying more than one character before the closing `'`. [SKY-P0015]
    MalformedChar,
    /// A block comment `{- … ` whose closing `-}` is missing before end of
    /// input. Nesting is supported (`{- {- -} -}`), so the scanner counts
    /// depth; depth > 0 at EOF triggers this error. [SKY-P0017]
    UnterminatedBlockComment,
    /// The module header is malformed. [SKY-P0020]
    MalformedModuleHeader(HeaderDefect),
    /// The `exposing (...)` list is malformed. [SKY-P0021]
    MalformedExposingList(ExposingDefect),
    /// A value binding's patterns are not followed by `=`. [SKY-P0030]
    MissingEquals { binding: Box<str> },
    /// A `type` declaration is malformed. [SKY-P0031]
    MalformedTypeDeclaration(TypeDeclDefect),
    /// Type arguments applied to a non-constructor. [SKY-P0040]
    TypeArgsOnNonConstructor,
    /// A token that cannot begin a type. [SKY-P0041]
    ExpectedType,
    /// A `(` opened something that never closed; `opener` is the `(` span. [SKY-P0050]
    UnclosedDelimiter { opener: Span },
    /// A `case … of` expression is malformed. [SKY-P0060]
    MalformedCase(CaseDefect),
    /// A `let … in` expression is malformed. [SKY-P0061]
    MalformedLet(LetDefect),
    /// An `if … then … else …` expression is malformed. [SKY-P0062]
    MalformedIf(IfDefect),
}

/// Errors raised during name resolution / canonicalisation.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum NameError {
    /// Coarse "a name did not resolve" (Milestone 0 — retained additively).
    Unknown,
    /// A bare value name resolves to nothing. [SKY-N0001]
    ValueNotFound {
        name: Box<str>,
        suggestions: Box<[Box<str>]>,
    },
    /// A type name is undefined. [SKY-N0002]
    TypeNotFound {
        name: Box<str>,
        suggestions: Box<[Box<str>]>,
    },
    /// A constructor is undefined/misspelled. [SKY-N0003]
    ConstructorNotFound {
        name: Box<str>,
        suggestions: Box<[Box<str>]>,
    },
    /// A qualifier names no module/import alias. [SKY-N0004]
    UnknownModule {
        qualifier: Box<str>,
        suggestions: Box<[Box<str>]>,
    },
    /// The qualifier resolves but the member is absent. [SKY-N0005]
    NoSuchMember {
        module: Box<str>,
        member: Box<str>,
        suggestions: Box<[Box<str>]>,
    },
    /// Two top-level values share a name; `first` is the earlier span. [SKY-N0010]
    DuplicateValue { name: Box<str>, first: Span },
    /// Two constructors share a name; `first` is the earlier span. [SKY-N0011]
    DuplicateConstructor { name: Box<str>, first: Span },
    /// Two types share a name; `first` is the earlier span. [SKY-N0012]
    DuplicateType { name: Box<str>, first: Span },
    /// A `type alias` is applied with the wrong number of type arguments —
    /// `Pair Int Bool` for a one-parameter `Pair a`, or a bare `Pair` where one
    /// argument is required. A type alias must be fully applied. [SKY-N0013]
    AliasArity {
        name: Box<str>,
        expected: usize,
        found: usize,
    },
    /// A local module named in an `import` cannot be found under `source_root`.
    /// `suggestions` lists close matches by Levenshtein distance. [SKY-N0020]
    ModuleNotFound {
        name: Box<str>,
        suggestions: Box<[Box<str>]>,
    },
    /// The import graph for the project contains a cycle; `path` lists the
    /// module names in cycle order (last element imports the first). [SKY-N0021]
    ImportCycle { path: Box<[Box<str>]> },
    /// An `import M exposing (x)` names a member `x` that `M` does not expose.
    /// `suggestions` lists close matches among `M`'s public exports. [SKY-N0022]
    NameNotExposed {
        module: Box<str>,
        name: Box<str>,
        suggestions: Box<[Box<str>]>,
    },
    /// The `module` declaration at the top of a `.sky` file does not match the
    /// path I derived from the file's location under `source_root`. [SKY-N0023]
    ModulePathMismatch {
        declared: Box<str>,
        expected: Box<str>,
    },
    /// Two `import` statements bring the same unqualified name into scope;
    /// `modules` lists the origins. [SKY-N0024]
    AmbiguousImport {
        name: Box<str>,
        modules: Box<[Box<str>]>,
    },
    /// A local module's name starts with `Sky` or `Std`, which are reserved for
    /// the standard library. [SKY-N0025]
    ReservedNamespace { name: Box<str> },
    /// A user `type` / `type alias` declaration reuses a name the compiler
    /// reserves for a built-in type constructor (`Int`, `Maybe`, `Html`, `Cmd`,
    /// `Length`, …). The lowerer matches these names ahead of the user-enum
    /// lookup, so accepting the shadow would silently override the user type and
    /// miscompile with no diagnostic; it is rejected at the declaration instead.
    /// [SKY-N0026]
    ReservedBuiltinType { name: Box<str> },
    /// Two `import` statements register the same qualifier (an explicit
    /// `as Alias`, or two module paths sharing a last segment) against
    /// DIFFERENT dep modules — `Utils.format` / `Http.get` would otherwise
    /// silently resolve to whichever import came last in source order, with
    /// no diagnostic. Re-importing the SAME dep module under the same
    /// qualifier (a diamond dependency) is NOT an error — only a genuine
    /// clash between two distinct dep modules is. `first` is the earlier
    /// import's span. [SKY-N0027]
    DuplicateQualifier { qualifier: Box<str>, first: Span },
}

/// Class label for the #90 T3 higher-order-kernel callback-result obligation.
///
/// `Maybe`/`Result` `map`/`map2..5`/`mapError`/`andMap` apply their callback
/// at one exact arity, so the callback's result must not itself be a
/// function. Shared between the constructor (`sky_types::super_unsatisfied`)
/// and the renderer's tailored [`TypeError::SuperTypeUnsatisfied`] sentence so
/// the two sites cannot drift — the generic "`X` is not a `<class>` type"
/// template would read as a confusing double negative for this label.
pub const HOF_KERNEL_RESULT_CLASS: &str =
    "non-function callback result (Maybe/Result higher-order kernel)";

/// Errors raised during type inference / checking.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TypeError {
    /// Coarse "two types failed to unify" (Milestone 0 — retained additively).
    Mismatch,
    /// Coarse "solver step budget exhausted" (Milestone 0 — retained).
    BudgetExceeded,
    /// Two types fail to unify, with rendered expected/found. [SKY-T0001]
    ///
    /// `TyDoc`s are boxed so the `Diagnostic` enum stays small (the `Err`
    /// half of every `DResult` in the compiler).
    TypeMismatch {
        expected: Box<TyDoc>,
        found: Box<TyDoc>,
        /// Optional secondary span of the definition that fixed the type.
        definition: Option<Span>,
        /// The diverging field/row path, e.g. `["user", "age"]`, if applicable.
        path: Box<[Box<str>]>,
    },
    /// Occurs-check failure: `var` would have to equal a type containing it. [SKY-T0002]
    InfiniteType { var: Box<str>, ty: Box<TyDoc> },
    /// The solver step budget `budget` was exhausted. [SKY-T0003]
    StepBudgetExceeded { budget: u64 },
    /// A typed binding has more parameter patterns than its annotation has
    /// arrows. [SKY-T0004]
    TooManyParameters {
        binding: Box<str>,
        signature: Box<TyDoc>,
    },
    /// A case does not cover every constructor; `missing` lists them. [SKY-T0010]
    NonExhaustiveCase { missing: Box<[Box<str>]> },
    /// Two arms cover the same constructor (warning). [SKY-T0011]
    RedundantCaseBranch { constructor: Box<str> },
    /// `Live.app` carries a non-empty `routes` list but the Model type has no
    /// `page` field, so the routes are forwarded to the non-routed path and
    /// never update the Model (warning — the program still compiles, matching
    /// the Go reference's silent no-op). Usually a mis-named routed-page field.
    /// [SKY-L0124]
    RoutedAppMissingPageField {
        /// How many routes were declared.
        route_count: usize,
    },
    /// A `record.field` access whose `field` is not present in the (closed)
    /// record type `record` — or whose base is not a record at all. [SKY-T0012]
    NoSuchField { field: Box<str>, record: Box<TyDoc> },
    /// A constructor pattern binds a number of payload sub-patterns that differs
    /// from the constructor's declared field count (`Just` with no payload, or
    /// `Node l r` for a three-field `Node`). [SKY-T0013]
    CtorPatternArity {
        ctor: Box<str>,
        expected: usize,
        found: usize,
    },
    /// A generic binding whose body constrains a type variable to a Sky
    /// super-type — `Number` (`+ - *`) or `Comparable` (`< > <= >=`) — is used
    /// at a type that does not provide those operations (`double True`, where
    /// `double` needs `Number`), or at a type that stays non-concrete (the
    /// obligation cannot be propagated across the use). `class` names the
    /// super-type; `found` is the offending type. A `class` equal to
    /// [`HOF_KERNEL_RESULT_CLASS`] renders through a tailored sentence — the
    /// generic "`X` is not a `<class>` type" template reads as a confusing
    /// double negative for that internal arity obligation. [SKY-T0014]
    SuperTypeUnsatisfied { class: Box<str>, found: Box<TyDoc> },
    /// A **parameter** pattern (lambda param, function-def head, or `let`
    /// binder) is **refutable** — it can fail to match some value of its type
    /// (`\(Just x) ->`, `\1 ->`, `\[a] ->`, `f (Just x) =`). A binding position
    /// must be irrefutable: it binds *every* value of its type and never
    /// discriminates. Rejecting it here — before lowering — makes a
    /// runtime match-failure on a well-typed program unrepresentable (no
    /// emitted panic arm, no `DoS`/500 surface). The offending sub-pattern's span
    /// rides on the wrapping [`Diagnostic::Type`]. [SKY-T0015]
    RefutablePatternParameter,
    /// A `Task` type constructor applied to a number of type arguments other than
    /// 1 (the internal unary form `Task a`) or 2 (the canonical user annotation
    /// `Task Error a`). Reachable from source because canonicalisation validates
    /// arity only for type *aliases* (`NameError::AliasArity`), never for a
    /// non-alias type-constructor application like `Task Error Int Bool` (3 args)
    /// or a bare `Task` (0 args). Converts a former `CompilerBug` ICE into a clean
    /// fail-closed diagnostic naming the found argument count. [SKY-T0016]
    ///
    /// `carrier` is the async-carrier constructor name — `"Task"` (expects 1
    /// or 2 args), `"Cmd"` / `"Sub"` (expect exactly 1) — so the rendered
    /// message names the actual type the user mis-applied, not always `Task`.
    TaskArity {
        carrier: &'static str,
        found: usize,
    },
}

/// A language feature that the Milestone-0 lowerer does not yet support. Each
/// maps to an `SKY-L01##` code; the `[feature: …]` tag matches the spec.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Feature {
    /// Wildcard/variable/literal patterns. [SKY-L0100]
    CasePatternKinds,
    /// Binary operators other than `+`/`-`. [SKY-L0101]
    BinOps,
    /// A value whose type stays fully polymorphic — the solver never pinned it
    /// to a concrete instance, so the lowerer cannot monomorphise it (M2a emits
    /// generic *functions*, but cannot yet represent an under-determined
    /// polymorphic *value*). [SKY-L0102]
    Polymorphism,
    /// Function types in argument/return position of a value annotation. [SKY-L0103]
    HigherOrderValues,
    /// Task types other than `Task ()`. [SKY-L0104]
    TaskResults,
    /// Non-variable function parameters. [SKY-L0105]
    ParamPatterns,
    /// Unannotated top-level functions with parameters. [SKY-L0106]
    UntypedFunctions,
    /// Bare function references / non-name callees. [SKY-L0107]
    FirstClassFunctions,
    /// Kernel calls beyond the supported set. [SKY-L0108]
    Kernels,
    /// A call whose argument count differs from the callee's declared arity:
    /// partial application (`add 2` where `add` takes two) or over-application
    /// across the arity boundary (`f 1 2` where `f` takes one). [SKY-L0110]
    PartialOverApplication,
    /// A functional update `{ r | … }` on a record whose type is generic (a field
    /// typed by a type variable). The backend copies the base record with
    /// `.clone()`, which needs the type parameter to be `Clone`-bounded —
    /// bounded generics are M2d, so this is a not-yet gap rather than broken
    /// Rust. Field access + construction on generic records DO work (M2c).
    /// [SKY-L0111]
    BoundedRecordUpdate,
    /// A RECORD sub-pattern nested inside a constructor payload or a tuple
    /// element (`Just { x }`, `( { x }, y )`). M3b-2 lowers nested variable /
    /// wildcard / constructor / tuple sub-patterns, and a record pattern at the
    /// `case` scrutinee or a `let` destructure — but a record pattern in a
    /// nested carrier needs that carrier's record type threaded to the lowerer,
    /// which lands later. The plain nested shapes (`Just (a, b)`,
    /// `Node (Node …) x r`) are accepted; only the nested-record carrier is
    /// gated here. [SKY-L0112]
    NestedPayloadPatterns,
    /// An `as`-alias in a REFUTABLE match-arm position whose inner pattern
    /// itself needs Rust-level runtime dispatch (a nested constructor,
    /// literal, or list/cons pattern anywhere) — `Just ((Ok x) as w)`. The
    /// common alias shape (`(a, b) as w`, dispatch-free) is fully supported;
    /// only a dispatch-NEEDING inner is gated here, because honoring it
    /// soundly by value would double-move a non-`Copy` payload (the exact
    /// #99 bug) and honoring it by reference would require matching the
    /// whole arm by reference — a materially larger redesign. [SKY-L0128]
    AliasOverRefutablePayload,
    /// A data constructor named as a first-class function *value* — referenced
    /// bare (`map Just xs`) or partially applied (`Node l 1` for a three-field
    /// `Node`). M3a lowers a *saturated* construction (`Just 5`, `Node l 1 r`);
    /// constructor-as-function awaits the same first-class-value machinery as a
    /// partially-applied top-level function. [SKY-L0113]
    CtorAsFunction,
    /// A function value stored in a CONSTRUCTOR PAYLOAD — declared
    /// (`type Box = Mk (Int -> Int)`) or laundered there through a type variable
    /// (`type Box a = Mk a` applied as `Mk (\n -> n + 1)`). The generated Rust
    /// enum derives `Clone`/`Debug`/`PartialEq` + `SkyStringify`, none of which a
    /// `Box<dyn Fn>` payload field satisfies, so accepting it would emit
    /// cargo-failing Rust. The sibling of [`Self::FirstClassFunctions`] (a
    /// function in a *record* field), split out so the message names the
    /// constructor-payload carrier and blames the construction site. [SKY-L0114]
    CtorPayloadFunction,
    /// A tuple pattern used in a position the lowerer cannot yet handle: a
    /// `case` on a tuple with MORE THAN ONE arm (needs product/literal-pattern
    /// exhaustiveness), or a tuple destructure binder (a single-arm tuple `case`
    /// or a tuple function parameter) whose element is REFUTABLE — a constructor
    /// or literal — so the binding could fail at run time. M3b-1 supports a
    /// single irrefutable tuple destructure (elements are variables / wildcards /
    /// nested irrefutable tuples); the richer shapes land later. [SKY-L0115]
    TuplePatternMatch,
    /// A refutable pattern-discrimination shape the lowerer cannot yet route to
    /// a Rust `match`. Several `case` arms head-matching the same CONSTRUCTOR and
    /// discriminating on their nested sub-patterns (`Som (Som x)` then `Som Non`
    /// then `Non`) ARE supported — each arm lowers one-to-one to a Rust arm in
    /// source order. This gate is reserved for the discrimination shapes that
    /// still lack their carrier: cons / list patterns and guarded arms. The
    /// exhaustiveness checker validates the `case` first (a non-exhaustive one is
    /// SKY-T0010), so an unsupported shape reaching here is gated cleanly rather
    /// than mis-lowered. [SKY-L0116]
    NestedCtorDiscrimination,
    /// A `Set Float` or `Dict Float v`. Sky's `Float` is `comparable`, so the
    /// type checker accepts it (the typing follows Sky); but the Rust backings
    /// — `BTreeSet<f64>` / `HashMap<f64, V>` — cannot exist, because `f64`
    /// implements neither `Ord` (no total order: NaN) nor `Hash` / `Eq`. This
    /// is a deliberate backend divergence, not an unimplemented feature: a
    /// `Float`-keyed collection has no sound Rust representation in the standard
    /// library, so it is rejected here at lowering rather than emitting Rust
    /// `cargo` rejects. Divergence from Sky, rationale: Rust backend capability.
    /// [SKY-L0117]
    FloatKeyedCollection,
    /// `Live.appRouted` (the URL-routing variant of the `Sky.Live` entry point)
    /// is not yet wired on the Rust backend. Use the non-routed `Live.app` with
    /// `init`/`update`/`view`/`subscriptions` until routing support lands.
    /// [SKY-L0118]
    RoutedLiveApp,
    /// The cfg record for an app entry point (`Live.app` / `Tui.app` /
    /// `Tui.program` / `Webview.app`) — or, for `Webview.app`, its nested
    /// `window` record and `window.size` tuple — was written as a let-bound
    /// variable (or any non-record expression) rather than an inline record
    /// literal. The Rust backend reads the cfg's field expressions directly at
    /// the call site to emit the runtime entry call, so a non-literal cfg has no
    /// fields to read. Inline the record until non-literal cfg lowering lands.
    /// [SKY-L0119]
    LetBoundAppCfg,
    /// A function/task/decoder value captured by a closure can only be
    /// called, not forwarded; bind the result outside the closure or wrap
    /// the forwarding in a named top-level function. [SKY-L0125]
    NonCloneCapture,
    /// A binding whose type embeds a function (a bare function value, or one
    /// held inside a `Maybe`/`Result`/user-union payload — #90) was used more
    /// than once in a value-consuming (non-callee) position. `Box<dyn Fn>` is
    /// not `Clone`, so a second consuming use would double-move in the
    /// emitted Rust (E0382). Calling the function is unlimited (a call
    /// borrows, never moves); only a second non-call use is rejected. A
    /// narrow, conservative gate — superseded once a general last-use clone
    /// pass (an extension of [`Self::NonCloneCapture`]'s analysis) lands.
    /// [SKY-L0127]
    FunctionValueReuse,
}

/// The app shape whose entry point rejected an inadmissible Model. Drives the
/// required-trait wording rendered for [`LowerError::InadmissibleAppModel`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum AppShape {
    /// `Std.Live` / `Sky.Live` — the Model is persisted to the session store, so
    /// it must be `serde`-serialisable (as well as `Clone` + `PartialEq`).
    Live,
    /// `Std.Tui` / `Sky.Tui` — the Model is kept in memory, so it must be
    /// `Clone`.
    Tui,
    /// `Std.Webview` / `Sky.Webview` — the Model is kept in memory, so it must
    /// be `Clone`.
    Webview,
    /// `Std.Cli` / `Sky.Cli` — the Model is kept in memory, so it must be
    /// `Clone`.
    Cli,
}

/// The category of the non-admissible payload found inside a Model.
///
/// The leaf whose rendered Rust type lacks the trait the app entry requires. The
/// renderer owns the exact wording; this enum keeps the presentation-free
/// classification.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ModelLeaf {
    /// A first-class function (`a -> b`).
    Function,
    /// A `Cmd` command.
    Command,
    /// A `Sub` subscription.
    Subscription,
    /// A `Task` effect.
    Task,
    /// A JSON `Decoder`.
    Decoder,
    /// An opaque `Db` / server / live request or response handle.
    Handle,
    /// A view value — `Html` / `Element` / a UI `Attribute` / a `Color` or other
    /// `Std.Ui` plain value. Clonable and comparable, but not serialisable; only
    /// reachable as the offending leaf for a `Sky.Live` Model.
    ViewValue,
}

/// Errors raised during lowering / emission: "not supported yet" or
/// "inadmissible for the target" — distinct from `CompilerBug` ("the compiler is
/// broken").
///
/// Not `Copy`: [`LowerError::InadmissibleAppModel`] carries an owned field name.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum LowerError {
    /// A feature the M0 lowerer does not implement. [SKY-L01##]
    Unsupported(Feature),
    /// A `Live`/`Tui`/`Webview` app-entry Model type whose Rust rendering does
    /// not satisfy the runtime bound the entry requires (`Live` needs
    /// `serde::Serialize + serde::de::DeserializeOwned + Clone + PartialEq`;
    /// `Tui`/`Webview` need `Clone`). `app` drives the wording, `field` names the
    /// offending Model field (empty when the Model is not a record), and `leaf`
    /// categorises the payload. Converts a would-be `cargo` trait-bound failure
    /// into a fail-closed `skyc` error. [SKY-L0120]
    InadmissibleAppModel {
        app: AppShape,
        field: Box<str>,
        leaf: ModelLeaf,
    },
    /// A `Live`/`Tui`/`Webview` app-entry Msg type whose Rust rendering does
    /// not satisfy the runtime bound the entry requires (`Live`/`Tui`/`Webview`
    /// all need `Clone + Send + 'static`; `Live` additionally needs `Sync +
    /// Debug`). The predicate used is `ir_type_is_derivable` (NOT serde), so
    /// `Html`/`Element`/`Color`-carrying Msg variants are accepted (they derive
    /// `Clone + Debug + PartialEq`). Converts a would-be `cargo` trait-bound
    /// failure into a fail-closed `skyc` error. [SKY-L0122]
    InadmissibleAppMsg {
        app: AppShape,
        field: Box<str>,
        leaf: ModelLeaf,
    },
    /// Expression nesting exceeded the backend's bounded emit depth. [SKY-L0200]
    BackendNestingTooDeep { limit: u16 },
    /// `JsonDec.succeed` / `Db.Decode.succeed` constructor has more than 10
    /// parameters, which exceeds the `curry1`..`curry10` helpers in the runtime.
    /// [SKY-L0121]
    DecodeSucceedArityTooHigh { n: usize },
    /// A `Live.route` pattern has a different number of `:param` segments than
    /// the page-constructor has payload fields. The extra params would be
    /// silently discarded or the constructor could never be fully applied.
    /// [SKY-L0122]
    RouteParamCountMismatch {
        /// The URL pattern string (e.g. `"/apps/:id/:slug"`).
        pattern: Box<str>,
        /// How many `:param` segments the pattern contains.
        param_count: usize,
        /// How many payload fields the page constructor declares.
        ctor_payload_count: usize,
    },
    /// A `Live.route` page builder is not a page constructor, inline lambda, or
    /// named function — the Rust backend cannot emit a type-directed params
    /// closure for a let-bound variable or computed expression. [SKY-L0123]
    RouteBuilderUnsupportedShape,
    /// A `Live.route` page-constructor payload field has a type that cannot be
    /// decoded from a URL `:param` string (only `String`, `Int`, `Float`, and
    /// `Bool` are supported). [SKY-L0123]
    RouteParamUnsupportedType {
        /// Zero-based index of the offending constructor payload field.
        field_index: usize,
        /// Short display name of the unsupported IR type.
        type_name: Box<str>,
    },
}

// ===========================================================================
// The diagnostic currency
// ===========================================================================

/// The single typed error currency of the compiler.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Diagnostic {
    Parse {
        span: Span,
        msg: ParseError,
    },
    Name {
        span: Span,
        msg: NameError,
    },
    Type {
        span: Span,
        msg: TypeError,
    },
    /// A feature that is not supported yet (distinct from a compiler bug).
    Lower {
        span: Span,
        msg: LowerError,
    },
    /// A violated internal invariant — illegal IR, missing region type, etc.
    /// `where_` names the stage; `detail` is the only free-form message.
    CompilerBug {
        where_: &'static str,
        detail: String,
    },
}

/// Result alias used throughout the compiler.
pub type DResult<T> = Result<T, Diagnostic>;

// ===========================================================================
// Help lines (structured, payload-derived)
// ===========================================================================

/// The role a secondary span plays in a diagnostic.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SpanRole {
    /// The earlier of two conflicting definitions.
    FirstDefinition,
    /// The opening delimiter of an unclosed group.
    Opener,
    /// The definition that fixed an inferred type.
    Definition,
}

/// A fixed, payload-free guidance line. Keeping these as an enum (not a
/// `String`) preserves the no-stringly-typed-channel invariant; the renderer
/// owns the wording.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Hint {
    /// Show the canonical module header `module Main exposing (main)`.
    ModuleHeaderExample,
    /// Suggest `..` or `Module.name` for a stray dot.
    UseDotDotOrQualified,
    /// Suggest separating a number and a name with a space.
    SeparateWithSpace,
    /// State the `i64` integer-literal range.
    IntegerLiteralRange,
    /// State the `f64` float-literal magnitude limit.
    FloatLiteralRange,
    /// Suggest adding a top-level type signature.
    AddTypeSignature,
    /// Explain how to raise the solver budget via `SKY_SOLVER_BUDGET`.
    RaiseSolverBudget,
    /// Explain that a nesting bound is deliberate (fail-fast).
    NestingBoundDeliberate,
    /// List the forms a type atom can take.
    TypeAtomForms,
    /// State that constructors must be uppercase.
    ConstructorMustBeUppercase,
    /// State that a feature is not supported yet (carries the feature).
    FeatureNotSupported(Feature),
    /// Explain that a parameter pattern must be irrefutable and suggest binding
    /// the whole value and using `case` (SKY-T0015).
    IrrefutableParameterRequired,
}

/// How confidently a [`Suggestion`] can be applied to source, mirroring rustc's
/// model. Governs whether `skyc fix` may auto-apply the edit.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Applicability {
    /// The replacement is correct and self-contained; eligible for auto-patch.
    MachineApplicable,
    /// The replacement is a best guess that may not be what the author meant.
    MaybeIncorrect,
    /// The replacement still contains placeholders the author must fill in.
    HasPlaceholders,
}

/// A typed, span-scoped source edit the compiler proposes as a fix.
///
/// The span is the region to replace; `replacement` is the literal text to write
/// there. Only [`Applicability::MachineApplicable`] suggestions are auto-applied;
/// the others are shown but require explicit per-edit confirmation.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Suggestion {
    /// The source region the edit replaces.
    pub span: Span,
    /// The literal text to substitute for the region.
    pub replacement: Box<str>,
    /// How safe the edit is to apply automatically.
    pub applicability: Applicability,
}

/// One line of help under a diagnostic. Names are carried as owned `Box<str>`;
/// everything else is a POD enum, so help text is rendered from data.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum HelpLine {
    /// "did you mean `<name>`?"
    DidYouMean(Box<str>),
    /// A free-form `note:` line whose text is built by the producer. Used when
    /// the message must carry producer-specific detail (e.g. the offending Model
    /// field name for [`LowerError::InadmissibleAppModel`]) and the diagnostic
    /// has no source span to hang a caret label on.
    Note(Box<str>),
    /// A fixed guidance line.
    Hint(Hint),
    /// Point at a related source location.
    SecondarySpan { span: Span, role: SpanRole },
    /// Name a constructor missing from an exhaustive match.
    MissingConstructor(Box<str>),
    /// A concrete, span-scoped fix the reader can apply (and `skyc fix` may
    /// auto-apply when [`Applicability::MachineApplicable`]).
    Suggest(Suggestion),
}

// ===========================================================================
// Total accessors
// ===========================================================================

impl Diagnostic {
    /// The stable error code for this diagnostic. Total: every variant — and
    /// every inner enum variant — has an explicit arm.
    #[must_use]
    pub fn code(&self) -> Code {
        match self {
            Self::Parse { msg, .. } => parse_code(msg),
            Self::Name { msg, .. } => name_code(msg),
            Self::Type { msg, .. } => type_code(msg),
            Self::Lower { msg, .. } => lower_code(msg),
            Self::CompilerBug { where_, .. } => bug_code(where_),
        }
    }

    /// The severity of this diagnostic.
    #[must_use]
    pub const fn severity(&self) -> Severity {
        match self {
            Self::Parse { .. } | Self::Name { .. } | Self::Lower { .. } => Severity::Error,
            Self::Type { msg, .. } => match msg {
                TypeError::RedundantCaseBranch { .. }
                | TypeError::RoutedAppMissingPageField { .. } => Severity::Warning,
                TypeError::Mismatch
                | TypeError::BudgetExceeded
                | TypeError::TypeMismatch { .. }
                | TypeError::InfiniteType { .. }
                | TypeError::StepBudgetExceeded { .. }
                | TypeError::TooManyParameters { .. }
                | TypeError::NonExhaustiveCase { .. }
                | TypeError::NoSuchField { .. }
                | TypeError::CtorPatternArity { .. }
                | TypeError::SuperTypeUnsatisfied { .. }
                | TypeError::RefutablePatternParameter
                | TypeError::TaskArity { .. } => Severity::Error,
            },
            Self::CompilerBug { .. } => Severity::Bug,
        }
    }

    /// The primary source span. `CompilerBug` has no span and returns
    /// [`Span::DUMMY`].
    #[must_use]
    pub const fn primary_span(&self) -> Span {
        match self {
            Self::Parse { span, .. }
            | Self::Name { span, .. }
            | Self::Type { span, .. }
            | Self::Lower { span, .. } => *span,
            Self::CompilerBug { .. } => Span::DUMMY,
        }
    }

    /// The structured help lines for this diagnostic, derived from its payload.
    /// Total over every variant.
    #[must_use]
    pub fn help(&self) -> Vec<HelpLine> {
        match self {
            Self::Parse { msg, .. } => parse_help(msg),
            Self::Name { msg, span } => name_help(msg, *span),
            Self::Type { msg, .. } => type_help(msg),
            Self::Lower { msg, .. } => lower_help(msg),
            Self::CompilerBug { .. } => Vec::new(),
        }
    }
}

// --- code() helpers --------------------------------------------------------

const fn parse_code(msg: &ParseError) -> Code {
    match msg {
        ParseError::Unexpected | ParseError::UnexpectedToken { .. } => SKY_P0001,
        ParseError::UnexpectedEof { .. } => SKY_P0002,
        ParseError::TooDeep | ParseError::NestingTooDeep { .. } => SKY_P0003,
        ParseError::UnknownChar(_) => SKY_P0010,
        ParseError::StrayDot => SKY_P0011,
        ParseError::NumberJoinedToName(_) => SKY_P0012,
        ParseError::IntLiteralOutOfRange => SKY_P0013,
        ParseError::FloatLiteralOutOfRange => SKY_P0016,
        ParseError::UnterminatedString => SKY_P0014,
        ParseError::MalformedChar => SKY_P0015,
        ParseError::UnterminatedBlockComment => SKY_P0017,
        ParseError::MalformedModuleHeader(_) => SKY_P0020,
        ParseError::MalformedExposingList(_) => SKY_P0021,
        ParseError::MissingEquals { .. } => SKY_P0030,
        ParseError::MalformedTypeDeclaration(_) => SKY_P0031,
        ParseError::TypeArgsOnNonConstructor => SKY_P0040,
        ParseError::ExpectedType => SKY_P0041,
        ParseError::UnclosedDelimiter { .. } => SKY_P0050,
        ParseError::MalformedCase(_) => SKY_P0060,
        ParseError::MalformedLet(_) => SKY_P0061,
        ParseError::MalformedIf(_) => SKY_P0062,
    }
}

const fn name_code(msg: &NameError) -> Code {
    match msg {
        NameError::Unknown | NameError::ValueNotFound { .. } => SKY_N0001,
        NameError::TypeNotFound { .. } => SKY_N0002,
        NameError::ConstructorNotFound { .. } => SKY_N0003,
        NameError::UnknownModule { .. } => SKY_N0004,
        NameError::NoSuchMember { .. } => SKY_N0005,
        NameError::DuplicateValue { .. } => SKY_N0010,
        NameError::DuplicateConstructor { .. } => SKY_N0011,
        NameError::DuplicateType { .. } => SKY_N0012,
        NameError::AliasArity { .. } => SKY_N0013,
        NameError::ModuleNotFound { .. } => SKY_N0020,
        NameError::ImportCycle { .. } => SKY_N0021,
        NameError::NameNotExposed { .. } => SKY_N0022,
        NameError::ModulePathMismatch { .. } => SKY_N0023,
        NameError::AmbiguousImport { .. } => SKY_N0024,
        NameError::ReservedNamespace { .. } => SKY_N0025,
        NameError::ReservedBuiltinType { .. } => SKY_N0026,
        NameError::DuplicateQualifier { .. } => SKY_N0027,
    }
}

const fn type_code(msg: &TypeError) -> Code {
    match msg {
        TypeError::Mismatch | TypeError::TypeMismatch { .. } => SKY_T0001,
        TypeError::InfiniteType { .. } => SKY_T0002,
        TypeError::BudgetExceeded | TypeError::StepBudgetExceeded { .. } => SKY_T0003,
        TypeError::TooManyParameters { .. } => SKY_T0004,
        TypeError::NonExhaustiveCase { .. } => SKY_T0010,
        TypeError::RedundantCaseBranch { .. } => SKY_T0011,
        TypeError::RoutedAppMissingPageField { .. } => SKY_L0124,
        TypeError::NoSuchField { .. } => SKY_T0012,
        TypeError::CtorPatternArity { .. } => SKY_T0013,
        TypeError::SuperTypeUnsatisfied { .. } => SKY_T0014,
        TypeError::RefutablePatternParameter => SKY_T0015,
        TypeError::TaskArity { .. } => SKY_T0016,
    }
}

const fn lower_code(msg: &LowerError) -> Code {
    match msg {
        LowerError::Unsupported(f) => feature_code(*f),
        LowerError::InadmissibleAppModel { .. } => SKY_L0120,
        LowerError::InadmissibleAppMsg { .. } => SKY_L0125,
        LowerError::BackendNestingTooDeep { .. } => SKY_L0200,
        LowerError::DecodeSucceedArityTooHigh { .. } => SKY_L0121,
        LowerError::RouteParamCountMismatch { .. } => SKY_L0122,
        LowerError::RouteBuilderUnsupportedShape
        | LowerError::RouteParamUnsupportedType { .. } => SKY_L0123,
    }
}

const fn feature_code(f: Feature) -> Code {
    match f {
        Feature::CasePatternKinds => SKY_L0100,
        Feature::BinOps => SKY_L0101,
        Feature::Polymorphism => SKY_L0102,
        Feature::HigherOrderValues => SKY_L0103,
        Feature::TaskResults => SKY_L0104,
        Feature::ParamPatterns => SKY_L0105,
        Feature::UntypedFunctions => SKY_L0106,
        Feature::FirstClassFunctions => SKY_L0107,
        Feature::Kernels => SKY_L0108,
        Feature::PartialOverApplication => SKY_L0110,
        Feature::BoundedRecordUpdate => SKY_L0111,
        Feature::NestedPayloadPatterns => SKY_L0112,
        Feature::AliasOverRefutablePayload => SKY_L0128,
        Feature::CtorAsFunction => SKY_L0113,
        Feature::CtorPayloadFunction => SKY_L0114,
        Feature::TuplePatternMatch => SKY_L0115,
        Feature::NestedCtorDiscrimination => SKY_L0116,
        Feature::FloatKeyedCollection => SKY_L0117,
        Feature::RoutedLiveApp => SKY_L0118,
        Feature::LetBoundAppCfg => SKY_L0119,
        Feature::NonCloneCapture => SKY_L0126,
        Feature::FunctionValueReuse => SKY_L0127,
    }
}

/// Maps a `CompilerBug.where_` tag to a stable `SKY-I####`. Unknown tags fall
/// back to the generic [`SKY_I0001`]; producers opt into a specific code by
/// stamping one of the recognised tags.
fn bug_code(where_: &str) -> Code {
    match where_ {
        "intern.resolve" => SKY_I0010,
        "intern.capacity" => SKY_I0011,
        "ir.match.unknown_variant" => SKY_I0100,
        "ir.match.duplicate_arm" => SKY_I0101,
        "ir.match.non_exhaustive" => SKY_I0102,
        "ir.match.arm_enum_mismatch" => SKY_I0103,
        "backend.no_rust_name" => SKY_I0200,
        "backend.dangling_symbol" => SKY_I0201,
        "backend.type_name_collision" => SKY_I0202,
        "backend.golden_anchor" => SKY_I0203,
        _ => SKY_I0001,
    }
}

// --- help() helpers --------------------------------------------------------

fn parse_help(msg: &ParseError) -> Vec<HelpLine> {
    match msg {
        ParseError::MalformedModuleHeader(_) => vec![HelpLine::Hint(Hint::ModuleHeaderExample)],
        ParseError::StrayDot => vec![HelpLine::Hint(Hint::UseDotDotOrQualified)],
        ParseError::NumberJoinedToName(_) => vec![HelpLine::Hint(Hint::SeparateWithSpace)],
        ParseError::IntLiteralOutOfRange => vec![HelpLine::Hint(Hint::IntegerLiteralRange)],
        ParseError::FloatLiteralOutOfRange => vec![HelpLine::Hint(Hint::FloatLiteralRange)],
        ParseError::ExpectedType => vec![HelpLine::Hint(Hint::TypeAtomForms)],
        ParseError::MalformedTypeDeclaration(_) => {
            vec![HelpLine::Hint(Hint::ConstructorMustBeUppercase)]
        }
        ParseError::TooDeep | ParseError::NestingTooDeep { .. } => {
            vec![HelpLine::Hint(Hint::NestingBoundDeliberate)]
        }
        ParseError::UnclosedDelimiter { opener } => vec![HelpLine::SecondarySpan {
            span: *opener,
            role: SpanRole::Opener,
        }],
        ParseError::Unexpected
        | ParseError::UnexpectedToken { .. }
        | ParseError::UnexpectedEof { .. }
        | ParseError::UnknownChar(_)
        | ParseError::UnterminatedString
        | ParseError::MalformedChar
        | ParseError::UnterminatedBlockComment
        | ParseError::MalformedExposingList(_)
        | ParseError::MissingEquals { .. }
        | ParseError::TypeArgsOnNonConstructor
        | ParseError::MalformedCase(_)
        | ParseError::MalformedLet(_)
        | ParseError::MalformedIf(_) => Vec::new(),
    }
}

fn name_help(msg: &NameError, span: Span) -> Vec<HelpLine> {
    match msg {
        NameError::ValueNotFound { suggestions, .. }
        | NameError::TypeNotFound { suggestions, .. }
        | NameError::ConstructorNotFound { suggestions, .. }
        | NameError::UnknownModule { suggestions, .. }
        | NameError::NoSuchMember { suggestions, .. }
        | NameError::ModuleNotFound { suggestions, .. }
        | NameError::NameNotExposed { suggestions, .. } => did_you_mean(suggestions, span),
        NameError::DuplicateValue { first, .. }
        | NameError::DuplicateConstructor { first, .. }
        | NameError::DuplicateType { first, .. }
        | NameError::DuplicateQualifier { first, .. } => vec![HelpLine::SecondarySpan {
            span: *first,
            role: SpanRole::FirstDefinition,
        }],
        NameError::Unknown
        | NameError::AliasArity { .. }
        | NameError::ImportCycle { .. }
        | NameError::ModulePathMismatch { .. }
        | NameError::AmbiguousImport { .. }
        | NameError::ReservedNamespace { .. }
        | NameError::ReservedBuiltinType { .. } => Vec::new(),
    }
}

fn type_help(msg: &TypeError) -> Vec<HelpLine> {
    match msg {
        TypeError::TypeMismatch { definition, .. } => definition.map_or_else(Vec::new, |span| {
            vec![HelpLine::SecondarySpan {
                span,
                role: SpanRole::Definition,
            }]
        }),
        TypeError::BudgetExceeded | TypeError::StepBudgetExceeded { .. } => {
            vec![HelpLine::Hint(Hint::RaiseSolverBudget)]
        }
        TypeError::NonExhaustiveCase { missing } => missing
            .iter()
            .map(|c| HelpLine::MissingConstructor(c.clone()))
            .collect(),
        TypeError::RefutablePatternParameter => {
            vec![HelpLine::Hint(Hint::IrrefutableParameterRequired)]
        }
        TypeError::RoutedAppMissingPageField { route_count } => vec![HelpLine::Note(
            format!(
                "the `routes` list has {route_count} route(s) but the Model has no \
                 `page` field, so routing is disabled and every URL serves the same \
                 app. The routed-page field must be named exactly `page` (of the \
                 `Page` ADT whose constructors appear as route destinations). Rename \
                 the field to `page`, or remove the `routes` list if routing is not \
                 needed."
            )
            .into_boxed_str(),
        )],
        TypeError::Mismatch
        | TypeError::InfiniteType { .. }
        | TypeError::TooManyParameters { .. }
        | TypeError::RedundantCaseBranch { .. }
        | TypeError::NoSuchField { .. }
        | TypeError::CtorPatternArity { .. }
        | TypeError::SuperTypeUnsatisfied { .. }
        | TypeError::TaskArity { .. } => Vec::new(),
    }
}

/// The human message for [`LowerError::InadmissibleAppModel`], shared by the
/// `note:` help line (span-free path) and the render caret label (when a span is
/// present). Names the offending field, the leaf kind, and the required trait.
#[must_use]
pub fn inadmissible_model_message(app: AppShape, field: &str, leaf: ModelLeaf) -> String {
    let (shape, requirement) = match app {
        AppShape::Live => (
            "Sky.Live",
            "serialisable (it is persisted to the session store)",
        ),
        AppShape::Tui => ("Sky.Tui", "clonable"),
        AppShape::Webview => ("Sky.Webview", "clonable"),
        AppShape::Cli => ("Sky.Cli", "clonable"),
    };
    let leaf_phrase = match leaf {
        ModelLeaf::Function => "a function",
        ModelLeaf::Command => "a command (`Cmd`)",
        ModelLeaf::Subscription => "a subscription (`Sub`)",
        ModelLeaf::Task => "a task (`Task`)",
        ModelLeaf::Decoder => "a decoder (`Decoder`)",
        ModelLeaf::Handle => "an opaque handle (`Db` / server / live request)",
        ModelLeaf::ViewValue => "a view value (`Html` / `Element` / `Color`)",
    };
    if field.is_empty() {
        format!(
            "a {shape} Model must be {requirement}, but this Model is {leaf_phrase}, \
             which is not — keep only plain data in the Model"
        )
    } else {
        format!(
            "a {shape} Model must be {requirement}, but its field `{field}` is {leaf_phrase}, \
             which is not — keep only plain data in the Model"
        )
    }
}

/// The human message for [`LowerError::InadmissibleAppMsg`], shared by the
/// `note:` help line (span-free path) and the render caret label (when a span is
/// present). Names the offending variant/field, the leaf kind, and the required
/// trait. Mirrors [`inadmissible_model_message`] but uses the Msg wording.
#[must_use]
pub fn inadmissible_msg_message(app: AppShape, field: &str, leaf: ModelLeaf) -> String {
    let shape = match app {
        AppShape::Live => "Sky.Live",
        AppShape::Tui => "Sky.Tui",
        AppShape::Webview => "Sky.Webview",
        AppShape::Cli => "Sky.Cli",
    };
    let leaf_phrase = match leaf {
        ModelLeaf::Function => "a function",
        ModelLeaf::Command => "a command (`Cmd`)",
        ModelLeaf::Subscription => "a subscription (`Sub`)",
        ModelLeaf::Task => "a task (`Task`)",
        ModelLeaf::Decoder => "a decoder (`Decoder`)",
        ModelLeaf::Handle => "an opaque handle (`Db` / server / live request)",
        ModelLeaf::ViewValue => "a view value (`Html` / `Element` / `Color`)",
    };
    if field.is_empty() {
        format!(
            "a {shape} Msg must be clonable and sendable, but this Msg is {leaf_phrase}, \
             which is not — keep only plain data in Msg variants"
        )
    } else {
        format!(
            "a {shape} Msg must be clonable and sendable, but its variant/field `{field}` \
             is {leaf_phrase}, which is not — keep only plain data in Msg variants"
        )
    }
}

fn lower_help(msg: &LowerError) -> Vec<HelpLine> {
    match msg {
        LowerError::Unsupported(Feature::UntypedFunctions) => {
            vec![HelpLine::Hint(Hint::AddTypeSignature)]
        }
        LowerError::Unsupported(f) => vec![HelpLine::Hint(Hint::FeatureNotSupported(*f))],
        // The Model gate has no source span (the IR is span-free at emit), so the
        // field-naming message is carried as a `note:` line here — the caret
        // label at [`crate::render`] only renders when a snippet is present.
        LowerError::InadmissibleAppModel { app, field, leaf } => vec![HelpLine::Note(
            inadmissible_model_message(*app, field, *leaf).into_boxed_str(),
        )],
        // Same span-free note pattern as the Model gate.
        LowerError::InadmissibleAppMsg { app, field, leaf } => vec![HelpLine::Note(
            inadmissible_msg_message(*app, field, *leaf).into_boxed_str(),
        )],
        LowerError::BackendNestingTooDeep { .. } => {
            vec![HelpLine::Hint(Hint::NestingBoundDeliberate)]
        }
        LowerError::DecodeSucceedArityTooHigh { n } => vec![HelpLine::Note(
            format!(
                "the constructor passed to `succeed` has {n} parameters; \
                 the runtime's `curry1`..`curry10` helpers cap at 10. \
                 Split the record into multiple smaller decoders and combine \
                 them with `andThen`, or reduce the field count below 10."
            )
            .into_boxed_str(),
        )],
        LowerError::RouteParamCountMismatch {
            pattern,
            param_count,
            ctor_payload_count,
        } => vec![HelpLine::Note(
            format!(
                "pattern `{pattern}` has {param_count} `:param` segment(s) but the \
                 page constructor has {ctor_payload_count} payload field(s). \
                 Add or remove `:param` segments in the pattern to match the \
                 constructor's arity, or use a nullary constructor for routes \
                 without parameters."
            )
            .into_boxed_str(),
        )],
        LowerError::RouteBuilderUnsupportedShape => vec![HelpLine::Note(
            "inline the constructor or lambda directly at the `Live.route` call site; \
             a let-bound variable or computed expression cannot be used as a page builder."
                .into(),
        )],
        LowerError::RouteParamUnsupportedType {
            field_index,
            type_name,
        } => vec![HelpLine::Note(
            format!(
                "payload field {field_index} has type `{type_name}`, which cannot be \
                 decoded from a URL `:param` string. Change the field type to `String`, \
                 `Int`, `Float`, or `Bool`, or use a nullary constructor for routes \
                 without `:param` segments."
            )
            .into_boxed_str(),
        )],
    }
}

/// Turns already-sorted suggestion names into help lines. A single candidate is
/// confident enough to offer as a [`Applicability::MachineApplicable`]
/// suggestion over `span` (the misspelled name's region); two or more stay
/// non-committal "did you mean" lines. The producer is responsible for the
/// stable `(Levenshtein, name)` ordering.
fn did_you_mean(suggestions: &[Box<str>], span: Span) -> Vec<HelpLine> {
    match suggestions {
        [only] => vec![HelpLine::Suggest(Suggestion {
            span,
            replacement: only.clone(),
            applicability: Applicability::MachineApplicable,
        })],
        many => many
            .iter()
            .map(|s| HelpLine::DidYouMean(s.clone()))
            .collect(),
    }
}
