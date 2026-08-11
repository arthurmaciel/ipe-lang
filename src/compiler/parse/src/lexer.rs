//! Layout-aware lexer for the supported subset of Ipê.
//!
//! This is a Rust port of the relevant pieces of the Haskell compiler's
//! `Ipe.Parse.{Primitives,Space,Number,Symbol,Variable}` — narrowed to the
//! token shapes the supported subset exercises. Rather than emit explicit
//! layout tokens, each token carries its 1-based `line`/`col`, and the parser
//! reconstructs block structure from columns (see [`crate::layout`]).
//!
//! The lexer never panics on arbitrary bytes: any byte it cannot classify
//! yields a typed, coded [`ParseError`] — [`ParseError::UnknownChar`] for an
//! unrecognised byte, [`ParseError::StrayDot`] for a lone `.`,
//! [`ParseError::NumberJoinedToName`] for `123abc`, and
//! [`ParseError::IntLiteralOutOfRange`] for an `i64` overflow.

use ipe_diagnostics::{DResult, Diagnostic, ParseError, Span};

/// A lexical token kind.
///
/// `Eq` is intentionally NOT derived: [`Tok::Float`] carries an `f64`, which is
/// only `PartialEq` (IEEE-754 `NaN` is not reflexive). Token comparison only
/// ever needs `==` against a fixed non-float variant (`t.kind == Tok::RParen`),
/// which `PartialEq` provides; nothing keys a map or set on a `Tok`.
#[derive(Clone, PartialEq, Debug)]
pub enum Tok {
    // Keywords.
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
    Do,
    DoParallel,
    // Punctuation / operators.
    LParen,
    RParen,
    LBrace,
    RBrace,
    /// `[` — opens a list literal `[a, b, c]` or a list pattern.
    LBracket,
    /// `]` — closes a list literal / pattern.
    RBracket,
    /// `::` — the right-associative list cons operator (`x :: xs`) and the cons
    /// pattern head. Lexed as ONE token (a maximal munch of `:`), never two
    /// adjacent [`Tok::Colon`].
    ColonColon,
    Equals,
    Pipe,
    Colon,
    Arrow,
    /// A `do`-block bind lead-in `<-` (`p <- task`).
    LeftArrow,
    /// A lambda lead-in `\` (`\x -> e`).
    Backslash,
    DotDot,
    /// A lone `.` introducing a field access on a non-identifier expression,
    /// e.g. the `.` in `(record).field`. Bare `ident.field` runs are still one
    /// [`Tok::Ident`]; this token only appears when the dot follows a closing
    /// delimiter or other non-identifier token.
    Dot,
    Comma,
    Underscore,
    Plus,
    /// The string/list append operator `++`. Lexed as ONE token (a maximal
    /// munch of `+`), never as two adjacent [`Tok::Plus`], so the parser sees
    /// the binary operator the reference grammar defines at precedence 5.
    PlusPlus,
    Minus,
    Star,
    Slash,
    SlashEq,
    /// The integer-division operator `//`. Lexed as ONE token (maximal munch of
    /// `/`), never as two adjacent [`Tok::Slash`].
    SlashSlash,
    EqEq,
    Lt,
    Gt,
    Le,
    Ge,
    AmpAmp,
    PipePipe,
    /// The forward-pipe operator `|>`. Lexed as ONE token (maximal munch of
    /// `|`), so `|>` never reaches the parser as `Pipe` then `Gt`.
    PipeGt,
    /// The backward-pipe operator `<|`. Lexed as ONE token (maximal munch of
    /// `<`), so `<|` never reaches the parser as `Lt` then `Pipe`.
    LtPipe,
    /// The forward-composition operator `>>`. Lexed as ONE token (maximal munch
    /// of `>`), so `>>` never reaches the parser as two adjacent [`Tok::Gt`].
    GtGt,
    /// The backward-composition operator `<<`. Lexed as ONE token (maximal munch
    /// of `<`), so `<<` never reaches the parser as two adjacent [`Tok::Lt`].
    LtLt,
    // Literals / names.
    /// A (possibly dotted) identifier, e.g. `count`, `Msg`, `String.fromInt`.
    Ident(String),
    /// An integer literal.
    Int(i64),
    /// A floating-point literal `1.5`, `3.0`, `1.5e3`, `2e-2`. The carried
    /// [`f64`] is the parsed value; the lexer only builds well-formed Elm-style
    /// float lexemes (a leading digit is required, so `.5` is not a float), so
    /// the downstream stages see a ready numeric value.
    Float(f64),
    /// A string literal `"hello"`. The carried [`String`] is the already
    /// UNESCAPED value (escape sequences such as `\n` / `\"` are resolved here),
    /// so downstream stages see the runtime string verbatim.
    Str(String),
    /// A triple-quoted string literal `"""..."""`.
    ///
    /// `raw` is the RAW content — escape sequences (`\n`, `\\`) and `{{expr}}`
    /// interpolation markers are NOT resolved here; the canonicaliser handles
    /// them downstream.
    ///
    /// `anchor` is the 1-based source column of the first non-whitespace content
    /// character (the anchor column A). The canonicaliser strips up to `A - 1`
    /// leading whitespace characters from every physical line after the first,
    /// so an indented `"""…"""` block does not carry its source margin into the
    /// runtime value.
    TripleStr {
        raw: String,
        anchor: u32,
    },
    /// A character literal `'a'`. The carried [`String`] is the single UNESCAPED
    /// character's text (exactly one `char`), so `'\n'` carries a one-character
    /// newline string. The backend renders it as a Rust `char` literal.
    Char(String),
}

/// A token with its source position.
///
/// `Eq` is not derived: the `kind` field is a [`Tok`], which carries an `f64`
/// in [`Tok::Float`] and so is only `PartialEq` (see [`Tok`]).
#[derive(Clone, PartialEq, Debug)]
pub struct Token {
    pub kind: Tok,
    /// 1-based line.
    pub line: u32,
    /// 1-based column.
    pub col: u32,
    pub span: Span,
}

struct Lexer {
    /// `(byte_offset, char)` pairs for the whole source.
    chars: Vec<(usize, char)>,
    pos: usize,
    line: u32,
    col: u32,
}

impl Lexer {
    fn new(src: &str) -> Self {
        Self {
            chars: src.char_indices().collect(),
            pos: 0,
            line: 1,
            col: 1,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).map(|&(_, c)| c)
    }

    fn peek2(&self) -> Option<char> {
        self.chars.get(self.pos + 1).map(|&(_, c)| c)
    }

    /// The character two positions ahead of the cursor, used to confirm a digit
    /// follows a signed exponent (`e-2`) before committing to a float lexeme.
    fn peek3(&self) -> Option<char> {
        self.chars.get(self.pos + 2).map(|&(_, c)| c)
    }

    /// Byte offset of the current char, or end-of-input.
    fn offset(&self) -> u32 {
        self.chars.get(self.pos).map_or_else(
            || {
                self.chars.last().map_or(0, |&(o, c)| {
                    u32::try_from(o + c.len_utf8()).unwrap_or(u32::MAX)
                })
            },
            |&(o, _)| u32::try_from(o).unwrap_or(u32::MAX),
        )
    }

    fn advance(&mut self) {
        if let Some(c) = self.peek() {
            if c == '\n' {
                self.line = self.line.saturating_add(1);
                self.col = 1;
            } else {
                self.col = self.col.saturating_add(1);
            }
            self.pos += 1;
        }
    }

    /// Skip whitespace, line comments (`-- … <newline>`), and block comments
    /// (`{- … -}`, nestable). Returns `Ok(())` on success; returns
    /// [`ParseError::UnterminatedBlockComment`] when a `{-` opener has no
    /// matching `-}` before end of input.
    fn skip_trivia(&mut self) -> DResult<()> {
        loop {
            match self.peek() {
                Some(c) if c == ' ' || c == '\t' || c == '\r' || c == '\n' => self.advance(),
                // Line comment: `-- … \n`. Consume everything up to (but not
                // including) the newline, then loop so the newline itself is
                // consumed as whitespace on the next iteration.
                Some('-') if self.peek2() == Some('-') => {
                    while let Some(c) = self.peek() {
                        if c == '\n' {
                            break;
                        }
                        self.advance();
                    }
                }
                // Block comment: `{- … -}`, nestable. A `{-` inside a comment
                // increments the depth; a `-}` decrements it; depth 0 ends.
                Some('{') if self.peek2() == Some('-') => {
                    let lo = self.offset();
                    self.advance(); // consume `{`
                    self.advance(); // consume `-`
                    let mut depth: u32 = 1;
                    loop {
                        match self.peek() {
                            None => {
                                return Err(Diagnostic::Parse {
                                    span: Span::new(lo, self.offset()),
                                    msg: ParseError::UnterminatedBlockComment,
                                });
                            }
                            Some('{') if self.peek2() == Some('-') => {
                                self.advance();
                                self.advance();
                                depth = depth.saturating_add(1);
                            }
                            Some('-') if self.peek2() == Some('}') => {
                                self.advance();
                                self.advance();
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            Some(_) => self.advance(),
                        }
                    }
                }
                _ => break,
            }
        }
        Ok(())
    }
}

/// The UTF-8 byte width of `c` as a `u32`, clamped to `1` on the impossible
/// overflow so a span can always be formed without a panic.
fn char_width(c: char) -> u32 {
    u32::try_from(c.len_utf8()).unwrap_or(1)
}

const fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

const fn is_ident_continue(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn keyword(text: &str) -> Option<Tok> {
    match text {
        "module" => Some(Tok::Module),
        "import" => Some(Tok::Import),
        "exposing" => Some(Tok::Exposing),
        "as" => Some(Tok::As),
        "type" => Some(Tok::Type),
        "case" => Some(Tok::Case),
        "of" => Some(Tok::Of),
        "let" => Some(Tok::Let),
        "in" => Some(Tok::In),
        "if" => Some(Tok::If),
        "then" => Some(Tok::Then),
        "else" => Some(Tok::Else),
        "do" => Some(Tok::Do),
        "doParallel" => Some(Tok::DoParallel),
        _ => None,
    }
}

/// Lex `src` into tokens, or fail with a typed diagnostic.
pub fn lex(src: &str) -> DResult<Vec<Token>> {
    let mut lx = Lexer::new(src);
    let mut out = Vec::new();
    loop {
        lx.skip_trivia()?;
        let Some(c) = lx.peek() else { break };
        let line = lx.line;
        let col = lx.col;
        let lo = lx.offset();

        let kind = if c.is_ascii_digit() {
            lex_number(&mut lx, lo)?
        } else if is_ident_start(c) {
            lex_ident(&mut lx)
        } else if c == '"' {
            lex_string(&mut lx, lo)?
        } else if c == '\'' {
            lex_char(&mut lx, lo)?
        } else {
            lex_symbol(&mut lx, c, lo)?
        };

        let hi = lx.offset();
        out.push(Token {
            kind,
            line,
            col,
            span: Span::new(lo, hi),
        });
    }
    Ok(out)
}

/// Consume a run of ASCII digits into `out` (zero or more). Each digit advances
/// the cursor; the first non-digit stops the run without consuming it.
fn consume_digits(lx: &mut Lexer, out: &mut String) {
    while let Some(c) = lx.peek() {
        if c.is_ascii_digit() {
            out.push(c);
            lx.advance();
        } else {
            break;
        }
    }
}

/// Lex a numeric literal — an integer `42` or an Elm-style float `1.5` / `3.0` /
/// `1.5e3` / `2e-2`. The caller peeked an ASCII digit, so the integer part is
/// always present (a leading digit is required: `.5` is NOT a float — the bare
/// `.` lexes separately).
///
/// A `.` is taken as a fractional point only when a digit follows it, so `1..5`
/// keeps its `..` range token and `1.foo` keeps the `.` as a field access. An
/// `e`/`E` is taken as an exponent only when a digit (after an optional sign)
/// follows, so a trailing `e` falls through to the joined-name check. A literal
/// with a fraction OR an exponent is a [`Tok::Float`]; otherwise a [`Tok::Int`].
fn lex_number(lx: &mut Lexer, lo: u32) -> DResult<Tok> {
    let mut text = String::new();
    let mut is_float = false;

    // Integer part: one or more ASCII digits (the caller guaranteed the first).
    consume_digits(lx, &mut text);

    // Fractional part: a `.` immediately followed by a digit.
    if lx.peek() == Some('.') && lx.peek2().is_some_and(|c| c.is_ascii_digit()) {
        is_float = true;
        text.push('.');
        lx.advance(); // consume the `.`
        consume_digits(lx, &mut text);
    }

    // Exponent part: `e`/`E`, an optional `+`/`-`, then one or more digits.
    if let Some(exp @ ('e' | 'E')) = lx.peek() {
        let signed = matches!(lx.peek2(), Some('+' | '-'));
        let digit_follows = if signed {
            lx.peek3().is_some_and(|c| c.is_ascii_digit())
        } else {
            lx.peek2().is_some_and(|c| c.is_ascii_digit())
        };
        if digit_follows {
            is_float = true;
            text.push(exp);
            lx.advance(); // consume `e`/`E`
            if let Some(sign @ ('+' | '-')) = lx.peek() {
                text.push(sign);
                lx.advance(); // consume the sign
            }
            consume_digits(lx, &mut text);
        }
    }

    // A digit run immediately followed by a letter / underscore (`123abc`,
    // `1.5x`) is not a valid token — surface the joined-name error at the
    // offending character.
    if let Some(c) = lx.peek()
        && is_ident_start(c)
    {
        let at = lx.offset();
        return Err(Diagnostic::Parse {
            span: Span::new(at, at + char_width(c)),
            msg: ParseError::NumberJoinedToName(c),
        });
    }

    let hi = lx.offset();
    if is_float {
        // `text` is a well-formed float lexeme by construction (a digit run, an
        // optional `.`-digit run, an optional `e[+-]`-digit run), so `f64`
        // parsing cannot fail — an out-of-range magnitude reads back as
        // `inf`, never an error. A failure here would be a lexer contract
        // breach, so it is a compiler bug rather than user error.
        let f = text.parse::<f64>().map_err(|e| Diagnostic::CompilerBug {
            where_: "ipe_parse::lex_number",
            detail: format!("well-formed float lexeme {text:?} failed to parse: {e}"),
        })?;
        // A magnitude past `f64::MAX` (e.g. `1e400`) parses to `inf`, which is
        // not the number the source spelled. Rejecting it — rather than
        // silently accepting infinity — restores parity with the Go reference
        // (Go's lexer errors on the same literal) and the principle of least
        // surprise. Finite literals (including a genuine `0.0`) pass through.
        if !f.is_finite() {
            return Err(Diagnostic::Parse {
                span: Span::new(lo, hi),
                msg: ParseError::FloatLiteralOutOfRange,
            });
        }
        Ok(Tok::Float(f))
    } else {
        // The integer part only pushed ASCII digits, so the sole parse failure
        // is an `i64` overflow — never an empty or malformed literal.
        let n = text.parse::<i64>().map_err(|_| Diagnostic::Parse {
            span: Span::new(lo, hi),
            msg: ParseError::IntLiteralOutOfRange,
        })?;
        Ok(Tok::Int(n))
    }
}

/// Lex a string literal.  The opening `"` is the current character.
///
/// If the next two characters are also `"` (i.e. the source spells `"""`),
/// dispatches to [`lex_triple_string`] which terminates only on a closing
/// `"""`. Otherwise lexes a single-line `"…"` string: escape sequences are
/// resolved into the runtime value; an unrecognised escape is kept verbatim
/// (backslash + char) so a typo surfaces as wrong text rather than lost data,
/// matching the Go reference's `unescapeString`. Reaching end of input before
/// the closing `"` is [`ParseError::UnterminatedString`].
fn lex_string(lx: &mut Lexer, lo: u32) -> DResult<Tok> {
    lx.advance(); // consume opening `"`

    // Detect triple-quoted string `"""…"""`.
    // After consuming the first `"`, a second and third `"` immediately
    // following means we are opening a multiline string, not an empty `""`
    // followed by a third `"`.
    if lx.peek() == Some('"') && lx.peek2() == Some('"') {
        lx.advance(); // consume second `"`
        lx.advance(); // consume third `"`
        return lex_triple_string(lx, lo);
    }

    // Single-line string `"…"`.
    let mut value = String::new();
    loop {
        match lx.peek() {
            None => {
                let hi = lx.offset();
                return Err(Diagnostic::Parse {
                    span: Span::new(lo, hi),
                    msg: ParseError::UnterminatedString,
                });
            }
            Some('"') => {
                lx.advance();
                return Ok(Tok::Str(value));
            }
            Some('\\') => {
                lx.advance();
                push_escape(lx, &mut value);
            }
            Some(c) => {
                value.push(c);
                lx.advance();
            }
        }
    }
}

/// Lex a triple-quoted string `"""…"""`.  The opening `"""` has already been
/// consumed by [`lex_string`].
///
/// The closing terminator is exactly three consecutive `"` characters.  A lone
/// `"` or a pair `""` not followed by a third `"` is treated as literal content
/// — this is the critical invariant that fixes the bug where inline HTML
/// (`class="card"`) terminated the string early.
///
/// Raw content is returned without escape processing: `{{expr}}` interpolation,
/// `\{{` literal-brace escapes, and `\\` collapse are handled downstream by the
/// canonicaliser (mirroring `Ipe.Parse.String.findTripleClose` in the Haskell
/// reference, which performs no escape resolution).
///
/// The anchor column A is the source column of the first non-whitespace content
/// character. The lexer's `col` cursor tracks the current source column; a
/// newline resets it to column 1 in [`Lexer::advance`], so the column recorded
/// at the first character that is neither a newline nor leading indentation is
/// A. The margin the canonicaliser strips is exactly this indentation, so
/// leading spaces and tabs are skipped when locating A. A body that is only
/// whitespace (or empty) anchors at column 1, which strips nothing downstream.
///
/// Reaching end of input before `"""` is [`ParseError::UnterminatedString`].
fn lex_triple_string(lx: &mut Lexer, lo: u32) -> DResult<Tok> {
    let mut value = String::new();
    let mut anchor: Option<u32> = None;
    loop {
        match lx.peek() {
            None => {
                let hi = lx.offset();
                return Err(Diagnostic::Parse {
                    span: Span::new(lo, hi),
                    msg: ParseError::UnterminatedString,
                });
            }
            Some('"') => {
                // Check whether the current `"` starts the closing `"""`.
                // peek() is `"` (confirmed by this arm); peek2() and peek3()
                // must also be `"` for a triple-close.
                if lx.peek2() == Some('"') && lx.peek3() == Some('"') {
                    lx.advance(); // consume first `"`  of `"""`
                    lx.advance(); // consume second `"` of `"""`
                    lx.advance(); // consume third `"`  of `"""`
                    return Ok(Tok::TripleStr {
                        raw: value,
                        anchor: anchor.unwrap_or(1),
                    });
                }
                // A literal `"` is content, so it fixes the anchor.
                anchor.get_or_insert(lx.col);
                value.push('"');
                lx.advance();
            }
            Some(c) => {
                if c != '\n' && c != '\r' && c != ' ' && c != '\t' {
                    anchor.get_or_insert(lx.col);
                }
                value.push(c);
                lx.advance();
            }
        }
    }
}

/// Lex a character literal `'c'` or `'\n'`. The opening `'` is the current char.
/// Exactly one character (or one escape sequence) must precede the closing `'`;
/// an empty `''`, a multi-character body, or a missing closing quote is
/// [`ParseError::MalformedChar`]. The carried value is the single unescaped
/// character's text.
fn lex_char(lx: &mut Lexer, lo: u32) -> DResult<Tok> {
    lx.advance(); // consume opening `'`
    let mut value = String::new();
    match lx.peek() {
        Some('\\') => {
            lx.advance();
            push_escape(lx, &mut value);
        }
        Some('\'') | None => {
            // Empty `''` or unterminated — malformed.
            let hi = lx.offset();
            return Err(Diagnostic::Parse {
                span: Span::new(lo, hi),
                msg: ParseError::MalformedChar,
            });
        }
        Some(c) => {
            value.push(c);
            lx.advance();
        }
    }
    // A single closing quote must follow; anything else (a second character or
    // end of input) is malformed.
    if lx.peek() == Some('\'') {
        lx.advance();
        // Enforce the single-char invariant the backend relies on: an
        // unrecognised escape (e.g. `'\q'`) resolves to backslash + char, two
        // scalar values. Recognised escapes (`\n \t \r \\ \" \' \0`) and plain
        // chars are all exactly one scalar value, so no valid program regresses.
        if value.chars().count() != 1 {
            return Err(Diagnostic::Parse {
                span: Span::new(lo, lx.offset()),
                msg: ParseError::MalformedChar,
            });
        }
        Ok(Tok::Char(value))
    } else {
        let hi = lx.offset();
        Err(Diagnostic::Parse {
            span: Span::new(lo, hi),
            msg: ParseError::MalformedChar,
        })
    }
}

/// Resolve one escape sequence (the leading `\` already consumed) and push its
/// value into `out`. An unrecognised escape is kept as backslash + char so the
/// user can see the typo rather than silently losing data, mirroring the Go
/// reference. End of input after a lone `\` pushes just the backslash.
fn push_escape(lx: &mut Lexer, out: &mut String) {
    match lx.peek() {
        Some('n') => {
            out.push('\n');
            lx.advance();
        }
        Some('t') => {
            out.push('\t');
            lx.advance();
        }
        Some('r') => {
            out.push('\r');
            lx.advance();
        }
        Some('\\') => {
            out.push('\\');
            lx.advance();
        }
        Some('"') => {
            out.push('"');
            lx.advance();
        }
        Some('\'') => {
            out.push('\'');
            lx.advance();
        }
        Some('0') => {
            out.push('\0');
            lx.advance();
        }
        Some(other) => {
            out.push('\\');
            out.push(other);
            lx.advance();
        }
        None => out.push('\\'),
    }
}

fn lex_ident(lx: &mut Lexer) -> Tok {
    let mut text = String::new();
    // First segment.
    while let Some(c) = lx.peek() {
        if is_ident_continue(c) {
            text.push(c);
            lx.advance();
        } else {
            break;
        }
    }
    // Dotted continuation: `.seg` runs, but never `..`.
    while lx.peek() == Some('.') && lx.peek2().is_some_and(is_ident_start) {
        text.push('.');
        lx.advance();
        while let Some(c) = lx.peek() {
            if is_ident_continue(c) {
                text.push(c);
                lx.advance();
            } else {
                break;
            }
        }
    }
    if text == "_" {
        return Tok::Underscore;
    }
    keyword(&text).unwrap_or(Tok::Ident(text))
}

/// Consume the current char, then optionally a following `second` char,
/// yielding `two` when the pair matches and `one` otherwise. Factors the
/// shared "one-or-two-character operator" shape (`=`/`==`, `<`/`<=`, `-`/`->`,
/// …) so each call site is a single line.
fn one_or_two(lx: &mut Lexer, second: char, two: Tok, one: Tok) -> Tok {
    lx.advance();
    if lx.peek() == Some(second) {
        lx.advance();
        two
    } else {
        one
    }
}

/// Lex a punctuation / operator token. `c` is the already-peeked first
/// character and `lo` is its byte offset (the start of the token).
fn lex_symbol(lx: &mut Lexer, c: char, lo: u32) -> DResult<Tok> {
    let kind = match c {
        '(' => one_char(lx, Tok::LParen),
        ')' => one_char(lx, Tok::RParen),
        '{' => one_char(lx, Tok::LBrace),
        '}' => one_char(lx, Tok::RBrace),
        '[' => one_char(lx, Tok::LBracket),
        ']' => one_char(lx, Tok::RBracket),
        // `::` (list cons) is a maximal munch of `:`; a lone `:` is the type
        // annotation colon.
        ':' => one_or_two(lx, ':', Tok::ColonColon, Tok::Colon),
        '\\' => one_char(lx, Tok::Backslash),
        ',' => one_char(lx, Tok::Comma),
        '+' => one_or_two(lx, '+', Tok::PlusPlus, Tok::Plus),
        '*' => one_char(lx, Tok::Star),
        '=' => one_or_two(lx, '=', Tok::EqEq, Tok::Equals),
        // `|` has three forms: `||`, `|>`, and bare `|`.
        '|' => {
            lx.advance();
            match lx.peek() {
                Some('|') => {
                    lx.advance();
                    Tok::PipePipe
                }
                Some('>') => {
                    lx.advance();
                    Tok::PipeGt
                }
                _ => Tok::Pipe,
            }
        }
        '-' => one_or_two(lx, '>', Tok::Arrow, Tok::Minus),
        // `/` has three forms: `//` (integer division), `/=` (not-equal), bare `/` (division).
        // Maximal-munch: consume the first `/` then peek the next char.
        '/' => {
            lx.advance();
            match lx.peek() {
                Some('/') => {
                    lx.advance();
                    Tok::SlashSlash
                }
                Some('=') => {
                    lx.advance();
                    Tok::SlashEq
                }
                _ => Tok::Slash,
            }
        }
        // `<` has five forms: `<=`, `<|`, `<<`, `<-`, and bare `<`.
        '<' => {
            lx.advance();
            match lx.peek() {
                Some('=') => {
                    lx.advance();
                    Tok::Le
                }
                Some('|') => {
                    lx.advance();
                    Tok::LtPipe
                }
                Some('<') => {
                    lx.advance();
                    Tok::LtLt
                }
                Some('-') => {
                    lx.advance();
                    Tok::LeftArrow
                }
                _ => Tok::Lt,
            }
        }
        // `>` has three forms: `>=`, `>>`, and bare `>`.
        '>' => {
            lx.advance();
            match lx.peek() {
                Some('=') => {
                    lx.advance();
                    Tok::Ge
                }
                Some('>') => {
                    lx.advance();
                    Tok::GtGt
                }
                _ => Tok::Gt,
            }
        }
        // `&` and `.` are valid ONLY as their two-char forms (`&&`, `..`);
        // a lone first char is a typed lex error rather than a token.
        '&' => return two_char_only(lx, lo, '&', Tok::AmpAmp, ParseError::UnknownChar('&')),
        '.' => return lex_dot(lx, lo),
        other => {
            return Err(Diagnostic::Parse {
                span: Span::new(lo, lo + char_width(other)),
                msg: ParseError::UnknownChar(other),
            });
        }
    };
    Ok(kind)
}

/// Consume the current char and yield `tok` (single-character token).
fn one_char(lx: &mut Lexer, tok: Tok) -> Tok {
    lx.advance();
    tok
}

/// Lex a `.`-led token. `..` is [`Tok::DotDot`] (range / spread); a `.`
/// immediately followed by an identifier start is [`Tok::Dot`] (field access on
/// a non-identifier expression, e.g. `(r).value`); a lone `.` is the typed lex
/// error [`ParseError::StrayDot`]. `lo` is the byte offset of the first `.`.
fn lex_dot(lx: &mut Lexer, lo: u32) -> DResult<Tok> {
    lx.advance(); // consume the first `.`
    match lx.peek() {
        Some('.') => {
            lx.advance();
            Ok(Tok::DotDot)
        }
        Some(c) if is_ident_start(c) => Ok(Tok::Dot),
        _ => Err(Diagnostic::Parse {
            span: Span::new(lo, lx.offset()),
            msg: ParseError::StrayDot,
        }),
    }
}

/// Lex a token that is only valid as a two-character pair: consume the first
/// char, require `second`, and yield `two`; a missing `second` is the typed
/// lex error `err`.
fn two_char_only(lx: &mut Lexer, lo: u32, second: char, two: Tok, err: ParseError) -> DResult<Tok> {
    lx.advance();
    if lx.peek() == Some(second) {
        lx.advance();
        Ok(two)
    } else {
        Err(Diagnostic::Parse {
            span: Span::new(lo, lx.offset()),
            msg: err,
        })
    }
}
