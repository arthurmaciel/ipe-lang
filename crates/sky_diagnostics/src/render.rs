//! Human-facing diagnostic rendering: the rustc/Elm 4-band layout.
//!
//! [`render`] turns a [`Diagnostic`] plus its source file into a string:
//!
//! 1. **header** — `error[CODE]: title` / `warning[CODE]: …` /
//!    `internal compiler error[CODE]: …`.
//! 2. **location** — ` --> file:line:col` derived from the primary span. A
//!    [`Span::DUMMY`] primary span (e.g. a [`Diagnostic::CompilerBug`]) suppresses
//!    the location and the source snippet entirely.
//! 3. **snippet** — the offending source line with a right-aligned line-number
//!    gutter, the primary span underlined with `^` and an inline payload-derived
//!    label, plus any secondary span underlined with `-` and its own label.
//! 4. **help / note** — the structured [`Diagnostic::help`] lines, then a
//!    `= note:` footer pointing at `skyc explain <CODE>`.
//!
//! The function is pure and **deterministic**: every list is walked in producer
//! order, there is no `HashMap` iteration, byte→line/col is a checked, clamped
//! scan, and slicing goes through [`str::get`] so a `DUMMY` or out-of-bounds span
//! renders gracefully rather than panicking. Colour (ANSI) is added only when
//! stderr is a terminal and `NO_COLOR` is unset; it never changes the byte
//! content beyond the escape sequences themselves.

use core::fmt::Write as _;

use crate::code::{ISSUE_TRACKER_URL, Severity, title};
use crate::diagnostic::{
    CaseDefect, Diagnostic, Expected, ExpectedSet, ExposingDefect, Feature, HeaderDefect, HelpLine,
    Hint, IfDefect, LetDefect, LowerError, NameError, ParseError, SpanRole, Suggestion, TokenKind,
    TyDoc, TypeDeclDefect, TypeError,
};
use crate::span::Span;

// ANSI escape sequences. Colour only ever *wraps* text, so stripping these
// yields byte-identical output to the plain (non-tty / `NO_COLOR`) path.
const RED: &str = "\x1b[31;1m";
const YELLOW: &str = "\x1b[33;1m";
const BLUE: &str = "\x1b[34;1m";
const RESET: &str = "\x1b[0m";

/// Render a diagnostic against its source file.
///
/// `file` is the path shown in the location line; `source` is its full contents,
/// used to extract the offending line and compute line/column. Both are only
/// read, never trusted to be well-formed: an empty `source`, a `DUMMY` span, or a
/// span past end-of-file all render without panicking.
#[must_use]
pub fn render(d: &Diagnostic, file: &str, source: &str) -> String {
    let color = color_enabled();
    let code = d.code();
    let severity = d.severity();
    let mut out = String::new();

    // Band 1 — header.
    let header_core = format!("{}[{}]", severity_word(severity), code.as_str());
    out.push_str(&paint(color, severity_color(severity), &header_core));
    out.push_str(": ");
    out.push_str(title(code));
    out.push('\n');

    // Split the help lines: secondary spans belong to the snippet band, the
    // rest to the help/note band.
    let help = d.help();
    let mut secondaries: Vec<(Span, SpanRole)> = Vec::new();
    let mut other_help: Vec<&HelpLine> = Vec::new();
    for line in &help {
        match line {
            HelpLine::SecondarySpan { span, role } => secondaries.push((*span, *role)),
            other => other_help.push(other),
        }
    }

    let primary = d.primary_span();
    let has_snippet = primary != Span::DUMMY;

    // Gutter width is the digit-count of the largest line number we will print.
    let gutter = if has_snippet {
        let mut max_line = locate(source, primary.lo).line;
        for (span, _) in &secondaries {
            if *span != Span::DUMMY {
                max_line = max_line.max(locate(source, span.lo).line);
            }
        }
        max_line.to_string().len()
    } else {
        1
    };
    let bar_pad = " ".repeat(gutter);

    if has_snippet {
        // Band 2 — location.
        let ploc = locate(source, primary.lo);
        out.push_str(&bar_pad);
        let _ = writeln!(out, "--> {file}:{}:{}", ploc.line, ploc.col);

        // Band 3 — snippet.
        out.push_str(&bar_pad);
        out.push_str(" |\n");
        let plabel = primary_label(d).unwrap_or_default();
        let primary_style = UnderlineStyle {
            glyph: '^',
            color_seq: severity_color(severity),
            label: &plabel,
        };
        push_span_block(&mut out, source, primary, &primary_style, gutter, color);
        for (span, role) in &secondaries {
            if *span != Span::DUMMY {
                out.push_str(&bar_pad);
                out.push_str(" |\n");
                let style = UnderlineStyle {
                    glyph: '-',
                    color_seq: BLUE,
                    label: role_label(*role),
                };
                push_span_block(&mut out, source, *span, &style, gutter, color);
            }
        }
    }

    // Band 4 — help + note footer.
    let mut footer: Vec<String> = Vec::new();
    for line in &other_help {
        let text = match line {
            // A span-scoped suggestion can show the text it replaces, so it is
            // rendered here (with source) rather than in the source-free
            // `help_text`.
            HelpLine::Suggest(s) => Some(suggestion_text(s, source)),
            other => help_text(other),
        };
        if let Some(text) = text {
            footer.push(text);
        }
    }
    // Humble messaging: an internal compiler error (every `SKY-I*`) is a gap in
    // Sky, not the reader's fault. Apologise plainly, Elm-style, and point at
    // the one issue tracker — never a raw backtrace, never false confidence.
    if severity == Severity::Bug {
        if let Diagnostic::CompilerBug { detail, .. } = d
            && !detail.is_empty()
        {
            footer.push(format!("note: {detail}"));
        }
        footer.push("note: this is a bug in Sky, please report it".to_string());
        footer.push(format!(
            "note: I'm not sure what went wrong here — sorry about that. This is likely a gap \
             in the Sky Rust compiler. Please report it (with this source + `skyc --version`) \
             at: {ISSUE_TRACKER_URL}"
        ));
    }
    // Every coded diagnostic keeps the explain-page pointer as its last note.
    footer.push(format!(
        "note: run `skyc explain {}` for more information",
        code.as_str()
    ));

    if has_snippet {
        out.push_str(&bar_pad);
        out.push_str(" |\n");
    }
    for line in &footer {
        out.push_str(&bar_pad);
        out.push_str(" = ");
        out.push_str(line);
        out.push('\n');
    }

    out
}

// ===========================================================================
// Source-position arithmetic (checked, clamped, panic-free)
// ===========================================================================

/// How a span's underline is drawn: the glyph, its colour, and the trailing
/// label. Bundled so [`push_span_block`] stays within the argument budget.
struct UnderlineStyle<'a> {
    glyph: char,
    color_seq: &'a str,
    label: &'a str,
}

/// A resolved source position plus the byte bounds of its line.
struct Loc {
    /// 1-based line number.
    line: usize,
    /// 1-based column, counted in characters.
    col: usize,
    /// Byte offset of the first character on the line.
    line_start: usize,
    /// Byte offset just past the last character on the line (before any `\n`).
    line_end: usize,
}

/// Locate a byte offset within `source`, clamping out-of-range and mid-character
/// offsets to the nearest valid char boundary. Never panics.
fn locate(source: &str, raw: u32) -> Loc {
    let byte = floor_boundary(source, raw as usize);
    let before = slice(source, 0, byte);
    let line = before.bytes().filter(|&b| b == b'\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let col = slice(source, line_start, byte).chars().count() + 1;
    let rest = slice(source, line_start, source.len());
    let line_len = rest.find('\n').unwrap_or(rest.len());
    Loc {
        line,
        col,
        line_start,
        line_end: line_start + line_len,
    }
}

/// The largest char boundary `<= b` (and `<= source.len()`).
fn floor_boundary(source: &str, b: usize) -> usize {
    let mut b = b.min(source.len());
    while b > 0 && !source.is_char_boundary(b) {
        b -= 1;
    }
    b
}

/// Slice `source[lo..hi]`, yielding `""` for any out-of-range or inverted range
/// instead of panicking. Replaces raw indexing (denied workspace-wide).
fn slice(source: &str, lo: usize, hi: usize) -> &str {
    source.get(lo..hi).unwrap_or("")
}

/// Emit one snippet block: the source line, then an underline of `glyph`s under
/// the span (clamped to the line) with `label` trailing.
fn push_span_block(
    out: &mut String,
    source: &str,
    span: Span,
    style: &UnderlineStyle,
    gutter: usize,
    color: bool,
) {
    let loc = locate(source, span.lo);
    let line_text = slice(source, loc.line_start, loc.line_end);
    let _ = writeln!(out, "{:>gutter$} | {line_text}", loc.line);

    let lo_byte = floor_boundary(source, (span.lo as usize).min(loc.line_end)).max(loc.line_start);
    let hi_byte = floor_boundary(source, (span.hi as usize).min(loc.line_end)).max(lo_byte);
    let width = slice(source, lo_byte, hi_byte).chars().count().max(1);

    let leading = " ".repeat(loc.col.saturating_sub(1));
    let underline = paint(
        color,
        style.color_seq,
        &style.glyph.to_string().repeat(width),
    );
    out.push_str(&" ".repeat(gutter));
    out.push_str(" | ");
    out.push_str(&leading);
    out.push_str(&underline);
    if !style.label.is_empty() {
        out.push(' ');
        out.push_str(style.label);
    }
    out.push('\n');
}

// ===========================================================================
// Colour
// ===========================================================================

fn color_enabled() -> bool {
    use std::io::IsTerminal;
    std::env::var_os("NO_COLOR").is_none() && std::io::stderr().is_terminal()
}

fn paint(color: bool, seq: &str, text: &str) -> String {
    if color {
        format!("{seq}{text}{RESET}")
    } else {
        text.to_string()
    }
}

const fn severity_word(s: Severity) -> &'static str {
    match s {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Bug => "internal compiler error",
    }
}

const fn severity_color(s: Severity) -> &'static str {
    match s {
        Severity::Error | Severity::Bug => RED,
        Severity::Warning => YELLOW,
    }
}

// ===========================================================================
// Primary inline labels (payload-derived)
// ===========================================================================

/// The inline label printed after the primary `^` underline, derived from the
/// diagnostic's payload. `None` means "carets only, no label".
fn primary_label(d: &Diagnostic) -> Option<String> {
    match d {
        Diagnostic::Parse { msg, .. } => parse_label(msg),
        Diagnostic::Name { msg, .. } => name_label(msg),
        Diagnostic::Type { msg, .. } => type_label(msg),
        Diagnostic::Lower { msg, .. } => Some(lower_label(*msg)),
        Diagnostic::CompilerBug { .. } => None,
    }
}

fn parse_label(msg: &ParseError) -> Option<String> {
    match msg {
        ParseError::UnexpectedToken { found, expected } => Some(format!(
            "found {}, expected {}",
            token_kind_str(*found),
            expected_set_str(expected)
        )),
        ParseError::UnexpectedEof { .. } => Some("input ended here".to_string()),
        ParseError::NestingTooDeep { limit, .. } => {
            Some(format!("nested past the limit of {limit}"))
        }
        ParseError::UnknownChar(c) => Some(format!("unknown character {}", char_repr(*c))),
        ParseError::StrayDot => Some("stray `.`".to_string()),
        ParseError::NumberJoinedToName(c) => {
            Some(format!("a number cannot be followed by {}", char_repr(*c)))
        }
        ParseError::IntLiteralOutOfRange => {
            Some("this integer does not fit in 64 bits".to_string())
        }
        ParseError::FloatLiteralOutOfRange => {
            Some("this float is too large to represent".to_string())
        }
        ParseError::UnterminatedString => Some("this string is never closed".to_string()),
        ParseError::MalformedChar => Some("this character literal is malformed".to_string()),
        ParseError::MalformedModuleHeader(defect) => Some(header_defect_str(*defect).to_string()),
        ParseError::MalformedExposingList(defect) => Some(exposing_defect_str(*defect).to_string()),
        ParseError::MissingEquals { binding } => Some(format!("`{binding}` needs an `=` here")),
        ParseError::MalformedTypeDeclaration(defect) => {
            Some(type_decl_defect_str(*defect).to_string())
        }
        ParseError::TypeArgsOnNonConstructor => {
            Some("only a type constructor can take arguments".to_string())
        }
        ParseError::ExpectedType => Some("expected a type here".to_string()),
        ParseError::UnclosedDelimiter { .. } => Some("this delimiter is never closed".to_string()),
        ParseError::MalformedCase(defect) => Some(case_defect_str(*defect).to_string()),
        ParseError::MalformedLet(defect) => Some(let_defect_str(*defect).to_string()),
        ParseError::MalformedIf(defect) => Some(if_defect_str(*defect).to_string()),
        ParseError::Unexpected | ParseError::TooDeep => None,
    }
}

fn name_label(msg: &NameError) -> Option<String> {
    match msg {
        NameError::ValueNotFound { .. } => Some("not found in scope".to_string()),
        NameError::TypeNotFound { .. } => Some("unknown type".to_string()),
        NameError::ConstructorNotFound { .. } => Some("unknown constructor".to_string()),
        NameError::UnknownModule { qualifier, .. } => Some(format!("unknown module `{qualifier}`")),
        NameError::NoSuchMember { module, member, .. } => {
            Some(format!("`{module}` has no member `{member}`"))
        }
        NameError::DuplicateValue { name, .. }
        | NameError::DuplicateConstructor { name, .. }
        | NameError::DuplicateType { name, .. } => Some(format!("`{name}` is redefined here")),
        NameError::AliasArity {
            name,
            expected,
            found,
        } => Some(format!(
            "`{name}` takes {expected} type argument(s), but {found} were given"
        )),
        NameError::Unknown => None,
    }
}

fn type_label(msg: &TypeError) -> Option<String> {
    match msg {
        TypeError::TypeMismatch {
            expected,
            found,
            path,
            ..
        } => {
            let mut label = format!(
                "expected {}, found {}",
                ty_to_string(expected),
                ty_to_string(found)
            );
            if !path.is_empty() {
                let joined: Vec<&str> = path.iter().map(AsRef::as_ref).collect();
                let _ = write!(label, " (at {})", joined.join("."));
            }
            Some(label)
        }
        TypeError::InfiniteType { var, ty } => {
            Some(format!("`{var}` would have to equal {}", ty_to_string(ty)))
        }
        TypeError::TooManyParameters { binding, signature } => {
            Some(format!("`{binding}` has type {}", ty_to_string(signature)))
        }
        TypeError::NonExhaustiveCase { .. } => Some("this case is not exhaustive".to_string()),
        TypeError::RedundantCaseBranch { constructor } => {
            Some(format!("`{constructor}` is already handled"))
        }
        TypeError::NoSuchField { field, record } => Some(format!(
            "type {} has no field `{field}`",
            ty_to_string(record)
        )),
        TypeError::CtorPatternArity {
            ctor,
            expected,
            found,
        } => Some(format!(
            "`{ctor}` binds {found} field(s) but its declaration has {expected}"
        )),
        TypeError::SuperTypeUnsatisfied { class, found } => {
            Some(format!("{} is not a {class} type", ty_to_string(found)))
        }
        TypeError::Mismatch | TypeError::BudgetExceeded | TypeError::StepBudgetExceeded { .. } => {
            None
        }
    }
}

fn lower_label(msg: LowerError) -> String {
    match msg {
        LowerError::Unsupported(f) => feature_label(f).to_string(),
        LowerError::BackendNestingTooDeep { limit } => {
            format!("nested past the backend limit of {limit}")
        }
    }
}

const fn role_label(role: SpanRole) -> &'static str {
    match role {
        SpanRole::FirstDefinition => "first defined here",
        SpanRole::Opener => "the unclosed delimiter opened here",
        SpanRole::Definition => "the type was fixed here",
    }
}

// ===========================================================================
// Help / note text
// ===========================================================================

/// Render a non-secondary help line into a `<kind>: <text>` string (the leading
/// `= ` is added by the caller). `None` drops the line.
fn help_text(line: &HelpLine) -> Option<String> {
    match line {
        HelpLine::DidYouMean(name) => Some(format!("help: did you mean `{name}`?")),
        HelpLine::Hint(hint) => Some(format!("help: {}", hint_text(*hint))),
        HelpLine::MissingConstructor(name) => {
            Some(format!("help: this case does not handle `{name}`"))
        }
        // Source-free fallback: the source-aware [`suggestion_text`] is used in
        // the render footer, but this arm keeps `help_text` total over `HelpLine`.
        HelpLine::Suggest(s) => Some(format!("help: replace with `{}`", s.replacement)),
        HelpLine::SecondarySpan { .. } => None,
    }
}

/// Render a suggestion as a `help: replace ... with ...` line, reading the old
/// text from `source` over the suggestion's span. Falls back to the source-free
/// wording when the span is empty or out of range.
fn suggestion_text(s: &Suggestion, source: &str) -> String {
    let lo = floor_boundary(source, s.span.lo as usize);
    let hi = floor_boundary(source, s.span.hi as usize).max(lo);
    let original = slice(source, lo, hi);
    if original.is_empty() {
        format!("help: replace with `{}`", s.replacement)
    } else {
        format!("help: replace `{original}` with `{}`", s.replacement)
    }
}

fn hint_text(hint: Hint) -> String {
    match hint {
        Hint::ModuleHeaderExample => {
            "a module header looks like `module Main exposing (main)`".to_string()
        }
        Hint::UseDotDotOrQualified => {
            "use `..` for a range, or `Module.name` for a qualified name".to_string()
        }
        Hint::SeparateWithSpace => "separate the number and the name with a space".to_string(),
        Hint::IntegerLiteralRange => {
            "integer literals must fit between -9223372036854775808 and 9223372036854775807"
                .to_string()
        }
        Hint::FloatLiteralRange => {
            "float literals must not exceed f64's maximum magnitude (~1.8e308)".to_string()
        }
        Hint::AddTypeSignature => {
            "add a top-level type signature, e.g. `f : Int -> Int`".to_string()
        }
        Hint::RaiseSolverBudget => {
            "raise the budget with `SKY_SOLVER_BUDGET=<n>` (0 disables the limit)".to_string()
        }
        Hint::NestingBoundDeliberate => {
            "this limit is deliberate, to fail fast on pathologically nested input".to_string()
        }
        Hint::TypeAtomForms => {
            "a type is a name, a type variable, or a parenthesised type".to_string()
        }
        Hint::ConstructorMustBeUppercase => {
            "constructor names must start with an uppercase letter".to_string()
        }
        Hint::FeatureNotSupported(f) => feature_label(f).to_string(),
    }
}

const fn feature_label(f: Feature) -> &'static str {
    match f {
        Feature::CasePatternKinds => {
            "case patterns other than nullary constructors are not supported yet \
             [feature: case-pattern-kinds]"
        }
        Feature::BinOps => {
            "operators other than `+` and `-` are not supported yet [feature: binops]"
        }
        Feature::Polymorphism => {
            "this value stays fully polymorphic — its concrete type is never \
             determined, so it cannot be compiled yet [feature: polymorphism]"
        }
        Feature::HigherOrderValues => {
            "function-valued parameters and returns are not supported yet \
             [feature: higher-order-values]"
        }
        Feature::TaskResults => "only `Task ()` is supported yet [feature: task-results]",
        Feature::ParamPatterns => {
            "parameter destructuring is not supported yet [feature: param-patterns]"
        }
        Feature::UntypedFunctions => {
            "top-level functions need a type signature [feature: untyped-functions]"
        }
        Feature::FirstClassFunctions => {
            "storing a function value in a record field is not supported yet \
             [feature: first-class-functions]"
        }
        Feature::Kernels => "this kernel function is not available yet [feature: kernels]",
        Feature::PartialOverApplication => {
            "partial application and over-application are not supported yet \
             [feature: partial-over-application]"
        }
        Feature::BoundedRecordUpdate => {
            "updating a generic record is not supported yet — it needs a \
             `Clone`-bounded type parameter (bounded generics are M2d) \
             [feature: bounded-record-update]"
        }
        Feature::NestedPayloadPatterns => {
            "a record pattern is supported at a `case` scrutinee or a `let` \
             destructure, but not yet nested inside a constructor payload or a \
             tuple element — that needs the carrier's record type threaded to the \
             lowerer [feature: nested-payload-patterns]"
        }
        Feature::CtorAsFunction => {
            "a data constructor used as a function value (referenced bare or \
             partially applied) is not supported yet — apply it to all its \
             fields at once [feature: ctor-as-function]"
        }
        Feature::CtorPayloadFunction => {
            "storing a function value in a constructor payload is not supported \
             yet [feature: ctor-payload-function]"
        }
        Feature::TuplePatternMatch => {
            "a tuple pattern is supported only as a single irrefutable destructure \
             (one `case` arm or a function parameter, with variable / `_` \
             elements) for now — matching a tuple with multiple arms or a \
             refutable element is not supported yet [feature: tuple-pattern-match]"
        }
        Feature::NestedCtorDiscrimination => {
            "this refutable pattern-discrimination shape is not supported yet — \
             discriminating with cons / list patterns or guarded arms needs \
             machinery that is not in place yet \
             [feature: nested-ctor-discrimination]"
        }
        Feature::FloatKeyedCollection => {
            "a `Set Float` / `Dict Float _` has no sound Rust representation: \
             `f64` is neither `Ord` (NaN has no total order) nor `Hash` / `Eq`, \
             which `BTreeSet` / `HashMap` require — use an `Int`, `Char`, or \
             `String` element / key instead. Divergence from Sky, rationale: \
             Rust backend capability [feature: float-keyed-collection]"
        }
    }
}

// ===========================================================================
// Token / payload stringifiers
// ===========================================================================

const fn token_kind_str(t: TokenKind) -> &'static str {
    match t {
        TokenKind::Module => "`module`",
        TokenKind::Import => "`import`",
        TokenKind::Exposing => "`exposing`",
        TokenKind::As => "`as`",
        TokenKind::Type => "`type`",
        TokenKind::Case => "`case`",
        TokenKind::Of => "`of`",
        TokenKind::Let => "`let`",
        TokenKind::In => "`in`",
        TokenKind::If => "`if`",
        TokenKind::Then => "`then`",
        TokenKind::Else => "`else`",
        TokenKind::LParen => "`(`",
        TokenKind::RParen => "`)`",
        TokenKind::LBrace => "`{`",
        TokenKind::RBrace => "`}`",
        TokenKind::LBracket => "`[`",
        TokenKind::RBracket => "`]`",
        TokenKind::ColonColon => "`::`",
        TokenKind::Equals => "`=`",
        TokenKind::Pipe => "`|`",
        TokenKind::Colon => "`:`",
        TokenKind::Arrow => "`->`",
        TokenKind::Backslash => "`\\`",
        TokenKind::DotDot => "`..`",
        TokenKind::Dot => "`.`",
        TokenKind::Comma => "`,`",
        TokenKind::Underscore => "`_`",
        TokenKind::Plus => "`+`",
        TokenKind::PlusPlus => "`++`",
        TokenKind::Minus => "`-`",
        TokenKind::Star => "`*`",
        TokenKind::Slash => "`/`",
        TokenKind::SlashEq => "`/=`",
        TokenKind::EqEq => "`==`",
        TokenKind::Lt => "`<`",
        TokenKind::Gt => "`>`",
        TokenKind::Le => "`<=`",
        TokenKind::Ge => "`>=`",
        TokenKind::AmpAmp => "`&&`",
        TokenKind::PipePipe => "`||`",
        TokenKind::PipeGt => "`|>`",
        TokenKind::LtPipe => "`<|`",
        TokenKind::Ident => "an identifier",
        TokenKind::Int => "a number",
        TokenKind::Float => "a floating-point number",
        TokenKind::Str => "a string literal",
        TokenKind::Char => "a character literal",
        TokenKind::Eof => "end of input",
    }
}

const fn expected_str(e: Expected) -> &'static str {
    match e {
        Expected::ModuleKeyword => "`module`",
        Expected::ExposingKeyword => "`exposing`",
        Expected::OfKeyword => "`of`",
        Expected::InKeyword => "`in`",
        Expected::ThenKeyword => "`then`",
        Expected::ElseKeyword => "`else`",
        Expected::Equals => "`=`",
        Expected::Arrow => "`->`",
        Expected::Pipe => "`|`",
        Expected::Comma => "`,`",
        Expected::Colon => "`:`",
        Expected::LParen => "`(`",
        Expected::RParen => "`)`",
        Expected::RBrace => "`}`",
        Expected::Identifier => "an identifier",
        Expected::Constructor => "a constructor",
        Expected::TypeAtom => "a type",
        Expected::Expression => "an expression",
        Expected::Pattern => "a pattern",
    }
}

/// Render an expected set in producer order: `a`, `a or b`, `a, b, or c`.
fn expected_set_str(set: &ExpectedSet) -> String {
    let items: Vec<&str> = set.0.iter().map(|e| expected_str(*e)).collect();
    match items.as_slice() {
        [] => "something else".to_string(),
        [only] => (*only).to_string(),
        [a, b] => format!("{a} or {b}"),
        [rest @ .., last] => format!("{}, or {last}", rest.join(", ")),
    }
}

const fn header_defect_str(d: HeaderDefect) -> &'static str {
    match d {
        HeaderDefect::NotModuleKeyword => "a file must begin with `module`",
        HeaderDefect::MissingName => "the module name is missing",
        HeaderDefect::NameNotIdentifier => "the module name must be an identifier",
        HeaderDefect::MissingExposing => "the `exposing` keyword is missing",
    }
}

const fn exposing_defect_str(d: ExposingDefect) -> &'static str {
    match d {
        ExposingDefect::MissingOpenParen => "the opening `(` is missing",
        ExposingDefect::BadSeparator => "expected `,` or `)` here",
        ExposingDefect::NameNotIdentifier => "an exposed name must be an identifier",
        ExposingDefect::MalformedCtorList => "this `Type(..)` constructor list is malformed",
    }
}

const fn type_decl_defect_str(d: TypeDeclDefect) -> &'static str {
    match d {
        TypeDeclDefect::MissingName => "the type name is missing",
        TypeDeclDefect::MissingEquals => "expected `=` before the constructors",
        TypeDeclDefect::CtorNotUppercase => {
            "a constructor name must start with an uppercase letter"
        }
        TypeDeclDefect::CtorNotIdentifier => "expected a constructor name here",
    }
}

const fn case_defect_str(d: CaseDefect) -> &'static str {
    match d {
        CaseDefect::MissingOf => "expected `of` after the scrutinee",
        CaseDefect::MissingArrow => "expected `->` in this branch",
        CaseDefect::NoBranches => "this `case` has no branches",
        CaseDefect::FirstBranchNotIndented => "the first branch must be indented past `case`",
    }
}

const fn let_defect_str(d: LetDefect) -> &'static str {
    match d {
        LetDefect::NoBindings => "this `let` has no bindings before `in`",
        LetDefect::BindingNameNotLower => "a let binding name must be a lowercase identifier",
        LetDefect::MissingEquals => "expected `=` after the binding name",
        LetDefect::MissingIn => "expected `in` after the let bindings",
    }
}

const fn if_defect_str(d: IfDefect) -> &'static str {
    match d {
        IfDefect::MissingCondition => "this `if` is missing its condition",
        IfDefect::MissingThen => "expected `then` after the condition",
        IfDefect::MissingElse => "expected `else` after the `then` branch",
    }
}

/// A human-safe representation of a character for inline display.
fn char_repr(c: char) -> String {
    if c.is_control() || c == ' ' {
        format!("U+{:04X}", c as u32)
    } else {
        format!("`{c}`")
    }
}

// ===========================================================================
// Type rendering
// ===========================================================================

/// Render a resolved type document at the top precedence level.
fn ty_to_string(t: &TyDoc) -> String {
    match t {
        TyDoc::Unit => "()".to_string(),
        TyDoc::Var(v) => v.to_string(),
        TyDoc::Con { module, name, args } => {
            let head = if module.is_empty() {
                name.to_string()
            } else {
                format!("{module}.{name}")
            };
            if args.is_empty() {
                head
            } else {
                let rendered: Vec<String> = args.iter().map(ty_arg).collect();
                format!("{head} {}", rendered.join(" "))
            }
        }
        TyDoc::Fun(a, b) => format!("{} -> {}", ty_fun_lhs(a), ty_to_string(b)),
        TyDoc::Tuple(elems) => {
            let rendered: Vec<String> = elems.iter().map(ty_to_string).collect();
            format!("({})", rendered.join(", "))
        }
        TyDoc::Record(fields) => {
            if fields.is_empty() {
                "{}".to_string()
            } else {
                let rendered: Vec<String> = fields
                    .iter()
                    .map(|(name, ty)| format!("{name} : {}", ty_to_string(ty)))
                    .collect();
                format!("{{ {} }}", rendered.join(", "))
            }
        }
    }
}

/// Render a type in argument position, parenthesising applications and arrows.
fn ty_arg(t: &TyDoc) -> String {
    match t {
        TyDoc::Con { args, .. } if !args.is_empty() => format!("({})", ty_to_string(t)),
        TyDoc::Fun(..) => format!("({})", ty_to_string(t)),
        _ => ty_to_string(t),
    }
}

/// Render a type on the left of an arrow, parenthesising nested arrows.
fn ty_fun_lhs(t: &TyDoc) -> String {
    match t {
        TyDoc::Fun(..) => format!("({})", ty_to_string(t)),
        _ => ty_to_string(t),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code::{SKY_I0001, SKY_N0001, SKY_P0050, SKY_T0001};
    use crate::diagnostic::{Diagnostic, Expected, ExpectedSet, ParseError};

    fn con(name: &str) -> TyDoc {
        TyDoc::Con {
            module: "".into(),
            name: name.into(),
            args: Box::new([]),
        }
    }

    #[test]
    fn type_mismatch_shows_caret_and_expected_found() {
        let src = "module Main exposing (main)\n\nmain =\n    foo\n";
        // Underline `foo` on line 4 (bytes 40..43).
        let d = Diagnostic::Type {
            span: Span::new(40, 43),
            msg: TypeError::TypeMismatch {
                expected: Box::new(con("Int")),
                found: Box::new(TyDoc::Con {
                    module: "".into(),
                    name: "List".into(),
                    args: Box::new([con("String")]),
                }),
                definition: None,
                path: Box::new([]),
            },
        };
        let out = render(&d, "test.sky", src);

        assert!(
            out.starts_with("error[SKY-T0001]: type mismatch\n"),
            "header:\n{out}"
        );
        assert!(out.contains("--> test.sky:4:5"), "location:\n{out}");
        assert!(out.contains("4 |     foo"), "source line:\n{out}");
        assert!(
            out.contains("^^^ expected Int, found List String"),
            "underline:\n{out}"
        );
        assert!(
            out.contains("= note: run `skyc explain SKY-T0001` for more information"),
            "footer:\n{out}"
        );
        // No ANSI in the non-tty test environment.
        assert!(!out.contains('\x1b'), "must be plain in tests:\n{out}");
    }

    #[test]
    fn dummy_span_compiler_bug_renders_header_help_footer_only() {
        let d = Diagnostic::CompilerBug {
            where_: "lower",
            detail: "no region type".into(),
        };
        let out = render(&d, "test.sky", "anything");

        assert!(out.starts_with("internal compiler error[SKY-I0001]: internal compiler error\n"));
        // No location / snippet band for a DUMMY span.
        assert!(!out.contains("-->"), "no location:\n{out}");
        assert!(!out.contains(" | "), "no snippet:\n{out}");
        assert!(
            out.contains("= note: no region type"),
            "detail surfaced:\n{out}"
        );
        assert!(out.contains("= note: this is a bug in Sky, please report it"));
        assert!(out.contains("= note: run `skyc explain SKY-I0001` for more information"));
        let _ = SKY_I0001;
    }

    #[test]
    fn out_of_bounds_span_clamps_without_panicking() {
        let src = "main =\n    foo\n"; // 15 bytes.
        let d = Diagnostic::Type {
            span: Span::new(11, 9999), // hi far past EOF.
            msg: TypeError::TypeMismatch {
                expected: Box::new(TyDoc::Unit),
                found: Box::new(con("Int")),
                definition: None,
                path: Box::new([]),
            },
        };
        let out = render(&d, "f.sky", src);
        assert!(out.contains("--> f.sky:2:5"), "clamped location:\n{out}");
        // Underline is bounded by the end of the line, not the bogus hi.
        assert!(out.contains("^^^"), "clamped underline:\n{out}");
        assert!(
            !out.contains("^^^^^^^^^^"),
            "underline must not run past EOL:\n{out}"
        );
    }

    #[test]
    fn fully_out_of_range_lo_does_not_panic() {
        // Both ends past EOF, and an empty source.
        let d = Diagnostic::Parse {
            span: Span::new(500, 600),
            msg: ParseError::IntLiteralOutOfRange,
        };
        let _ = render(&d, "empty.sky", "");
        let d2 = Diagnostic::Type {
            span: Span::new(500, 600),
            msg: TypeError::Mismatch,
        };
        let out = render(&d2, "empty.sky", "");
        assert!(out.contains("error[SKY-T0001]"));
    }

    #[test]
    fn secondary_span_renders_its_own_underline_and_label() {
        let src = "main =\n    ( foo\n";
        let d = Diagnostic::Parse {
            span: Span::new(15, 16), // somewhere on line 2.
            msg: ParseError::UnclosedDelimiter {
                opener: Span::new(11, 12),
            },
        };
        let out = render(&d, "p.sky", src);
        assert!(out.contains('^'), "primary underline:\n{out}");
        assert!(
            out.contains("- the unclosed delimiter opened here"),
            "secondary:\n{out}"
        );
        let _ = SKY_P0050;
    }

    #[test]
    fn did_you_mean_ordering_is_deterministic() {
        let d = Diagnostic::Name {
            span: Span::new(0, 6),
            msg: NameError::ValueNotFound {
                name: "lenght".into(),
                suggestions: Box::new(["length".into(), "list".into()]),
            },
        };
        let out = render(&d, "n.sky", "lenght\n");
        let first = out.find("did you mean `length`?").unwrap_or(usize::MAX);
        let second = out.find("did you mean `list`?").unwrap_or(0);
        assert!(first < second, "producer order preserved:\n{out}");
        // Stable across runs: re-render must be byte-identical.
        assert_eq!(out, render(&d, "n.sky", "lenght\n"));
        let _ = SKY_N0001;
    }

    #[test]
    fn unexpected_token_label_lists_expected_set() {
        let d = Diagnostic::Parse {
            span: Span::new(0, 1),
            msg: ParseError::UnexpectedToken {
                found: TokenKind::Int,
                expected: ExpectedSet(Box::new([
                    Expected::Identifier,
                    Expected::Constructor,
                    Expected::LParen,
                ])),
            },
        };
        let out = render(&d, "t.sky", "5\n");
        assert!(
            out.contains("found a number, expected an identifier, a constructor, or `(`"),
            "{out}"
        );
    }

    #[test]
    fn warning_header_word_for_redundant_branch() {
        let d = Diagnostic::Type {
            span: Span::new(0, 3),
            msg: TypeError::RedundantCaseBranch {
                constructor: "Red".into(),
            },
        };
        let out = render(&d, "w.sky", "Red\n");
        assert!(
            out.starts_with("warning[SKY-T0011]: redundant case branch\n"),
            "{out}"
        );
        let _ = SKY_T0001;
    }

    #[test]
    fn single_candidate_renders_machine_applicable_replacement() {
        // One suggestion → a span-scoped "replace `lenght` with `length`".
        let src = "lenght\n";
        let d = Diagnostic::Name {
            span: Span::new(0, 6),
            msg: NameError::ValueNotFound {
                name: "lenght".into(),
                suggestions: Box::new(["length".into()]),
            },
        };
        let out = render(&d, "n.sky", src);
        assert!(
            out.contains("= help: replace `lenght` with `length`"),
            "suggestion:\n{out}"
        );
        // Re-render is byte-identical (deterministic).
        assert_eq!(out, render(&d, "n.sky", src));
    }

    #[test]
    fn compiler_bug_emits_apology_and_tracker_url() {
        let d = Diagnostic::CompilerBug {
            where_: "lower",
            detail: "no region type".into(),
        };
        let out = render(&d, "x.sky", "anything");
        assert!(
            out.contains("sorry about that"),
            "Elm-style apology:\n{out}"
        );
        assert!(
            out.contains(crate::code::ISSUE_TRACKER_URL),
            "tracker URL:\n{out}"
        );
        // Footer still ends with the explain pointer.
        assert!(
            out.trim_end()
                .ends_with("note: run `skyc explain SKY-I0001` for more information"),
            "explain pointer last:\n{out}"
        );
    }

    #[test]
    fn nested_type_args_parenthesise() {
        let inner = TyDoc::Con {
            module: "".into(),
            name: "Maybe".into(),
            args: Box::new([con("Int")]),
        };
        let outer = TyDoc::Con {
            module: "".into(),
            name: "List".into(),
            args: Box::new([inner]),
        };
        assert_eq!(ty_to_string(&outer), "List (Maybe Int)");
        let f = TyDoc::Fun(Box::new(con("Int")), Box::new(con("Bool")));
        let g = TyDoc::Fun(Box::new(f), Box::new(con("Char")));
        assert_eq!(ty_to_string(&g), "(Int -> Bool) -> Char");
    }
}
