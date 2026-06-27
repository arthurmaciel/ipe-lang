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
    SKY_L0105, SKY_L0106, SKY_L0107, SKY_L0108, SKY_L0200, SKY_N0001, SKY_N0002, SKY_N0003,
    SKY_N0004, SKY_N0005, SKY_N0010, SKY_N0011, SKY_N0012, SKY_P0001, SKY_P0002, SKY_P0003,
    SKY_P0010, SKY_P0011, SKY_P0012, SKY_P0013, SKY_P0020, SKY_P0021, SKY_P0030, SKY_P0031,
    SKY_P0040, SKY_P0041, SKY_P0050, SKY_P0060, SKY_T0001, SKY_T0002, SKY_T0003, SKY_T0004,
    SKY_T0010, SKY_T0011, Severity,
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
    LParen,
    RParen,
    Equals,
    Pipe,
    Colon,
    Arrow,
    DotDot,
    Comma,
    Underscore,
    Plus,
    Minus,
    Ident,
    Int,
    /// End of input.
    Eof,
}

/// A single grammatical item the parser expected at the failure position.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Expected {
    ModuleKeyword,
    ExposingKeyword,
    OfKeyword,
    Equals,
    Arrow,
    Pipe,
    Comma,
    Colon,
    LParen,
    RParen,
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
}

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
}

/// A language feature that the Milestone-0 lowerer does not yet support. Each
/// maps to an `SKY-L01##` code; the `[feature: …]` tag matches the spec.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Feature {
    /// Wildcard/variable/literal patterns. [SKY-L0100]
    CasePatternKinds,
    /// Binary operators other than `+`/`-`. [SKY-L0101]
    BinOps,
    /// Type variables in annotations. [SKY-L0102]
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
}

/// Errors raised during lowering: "not supported yet" — distinct from
/// `CompilerBug` ("the compiler is broken").
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum LowerError {
    /// A feature the M0 lowerer does not implement. [SKY-L01##]
    Unsupported(Feature),
    /// Expression nesting exceeded the backend's bounded emit depth. [SKY-L0200]
    BackendNestingTooDeep { limit: u16 },
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
            Self::Lower { msg, .. } => lower_code(*msg),
            Self::CompilerBug { where_, .. } => bug_code(where_),
        }
    }

    /// The severity of this diagnostic.
    #[must_use]
    pub const fn severity(&self) -> Severity {
        match self {
            Self::Parse { .. } | Self::Name { .. } | Self::Lower { .. } => Severity::Error,
            Self::Type { msg, .. } => match msg {
                TypeError::RedundantCaseBranch { .. } => Severity::Warning,
                TypeError::Mismatch
                | TypeError::BudgetExceeded
                | TypeError::TypeMismatch { .. }
                | TypeError::InfiniteType { .. }
                | TypeError::StepBudgetExceeded { .. }
                | TypeError::TooManyParameters { .. }
                | TypeError::NonExhaustiveCase { .. } => Severity::Error,
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
            Self::Lower { msg, .. } => lower_help(*msg),
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
        ParseError::MalformedModuleHeader(_) => SKY_P0020,
        ParseError::MalformedExposingList(_) => SKY_P0021,
        ParseError::MissingEquals { .. } => SKY_P0030,
        ParseError::MalformedTypeDeclaration(_) => SKY_P0031,
        ParseError::TypeArgsOnNonConstructor => SKY_P0040,
        ParseError::ExpectedType => SKY_P0041,
        ParseError::UnclosedDelimiter { .. } => SKY_P0050,
        ParseError::MalformedCase(_) => SKY_P0060,
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
    }
}

const fn lower_code(msg: LowerError) -> Code {
    match msg {
        LowerError::Unsupported(f) => feature_code(f),
        LowerError::BackendNestingTooDeep { .. } => SKY_L0200,
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
        | ParseError::MalformedExposingList(_)
        | ParseError::MissingEquals { .. }
        | ParseError::TypeArgsOnNonConstructor
        | ParseError::MalformedCase(_) => Vec::new(),
    }
}

fn name_help(msg: &NameError, span: Span) -> Vec<HelpLine> {
    match msg {
        NameError::ValueNotFound { suggestions, .. }
        | NameError::TypeNotFound { suggestions, .. }
        | NameError::ConstructorNotFound { suggestions, .. }
        | NameError::UnknownModule { suggestions, .. }
        | NameError::NoSuchMember { suggestions, .. } => did_you_mean(suggestions, span),
        NameError::DuplicateValue { first, .. }
        | NameError::DuplicateConstructor { first, .. }
        | NameError::DuplicateType { first, .. } => vec![HelpLine::SecondarySpan {
            span: *first,
            role: SpanRole::FirstDefinition,
        }],
        NameError::Unknown => Vec::new(),
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
        TypeError::Mismatch
        | TypeError::InfiniteType { .. }
        | TypeError::TooManyParameters { .. }
        | TypeError::RedundantCaseBranch { .. } => Vec::new(),
    }
}

fn lower_help(msg: LowerError) -> Vec<HelpLine> {
    match msg {
        LowerError::Unsupported(Feature::UntypedFunctions) => {
            vec![HelpLine::Hint(Hint::AddTypeSignature)]
        }
        LowerError::Unsupported(f) => vec![HelpLine::Hint(Hint::FeatureNotSupported(f))],
        LowerError::BackendNestingTooDeep { .. } => {
            vec![HelpLine::Hint(Hint::NestingBoundDeliberate)]
        }
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
