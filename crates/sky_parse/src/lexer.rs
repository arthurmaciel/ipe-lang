//! Layout-aware lexer for the Milestone-0 subset of Sky.
//!
//! This is a Rust port of the relevant pieces of the Haskell compiler's
//! `Sky.Parse.{Primitives,Space,Number,Symbol,Variable}` — narrowed to the
//! token shapes the M0 golden program exercises. Rather than emit explicit
//! layout tokens, each token carries its 1-based `line`/`col`, and the parser
//! reconstructs block structure from columns (see [`crate::layout`]).
//!
//! The lexer never panics on arbitrary bytes: any byte it cannot classify
//! yields a typed, coded [`ParseError`] — [`ParseError::UnknownChar`] for an
//! unrecognised byte, [`ParseError::StrayDot`] for a lone `.`,
//! [`ParseError::NumberJoinedToName`] for `123abc`, and
//! [`ParseError::IntLiteralOutOfRange`] for an `i64` overflow.

use sky_diagnostics::{DResult, Diagnostic, ParseError, Span};

/// A lexical token kind (M0 subset).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Tok {
    // Keywords.
    Module,
    Import,
    Exposing,
    As,
    Type,
    Case,
    Of,
    // Punctuation / operators.
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
    // Literals / names.
    /// A (possibly dotted) identifier, e.g. `count`, `Msg`, `String.fromInt`.
    Ident(String),
    /// An integer literal.
    Int(i64),
}

/// A token with its source position.
#[derive(Clone, PartialEq, Eq, Debug)]
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

    /// Skip whitespace and line comments (`-- … <newline>`).
    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(c) if c == ' ' || c == '\t' || c == '\r' || c == '\n' => self.advance(),
                Some('-') if self.peek2() == Some('-') => {
                    while let Some(c) = self.peek() {
                        if c == '\n' {
                            break;
                        }
                        self.advance();
                    }
                }
                _ => break,
            }
        }
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
        _ => None,
    }
}

/// Lex `src` into tokens, or fail with a typed diagnostic.
pub fn lex(src: &str) -> DResult<Vec<Token>> {
    let mut lx = Lexer::new(src);
    let mut out = Vec::new();
    loop {
        lx.skip_trivia();
        let Some(c) = lx.peek() else { break };
        let line = lx.line;
        let col = lx.col;
        let lo = lx.offset();

        let kind = if c.is_ascii_digit() {
            lex_number(&mut lx, lo)?
        } else if is_ident_start(c) {
            lex_ident(&mut lx)
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

fn lex_number(lx: &mut Lexer, lo: u32) -> DResult<Tok> {
    let mut text = String::new();
    while let Some(c) = lx.peek() {
        if c.is_ascii_digit() {
            text.push(c);
            lx.advance();
        } else if is_ident_start(c) {
            // A digit immediately followed by a letter is not a valid M0 token.
            let at = lx.offset();
            return Err(Diagnostic::Parse {
                span: Span::new(at, at + char_width(c)),
                msg: ParseError::NumberJoinedToName(c),
            });
        } else {
            break;
        }
    }
    // The loop only pushed ASCII digits, so the sole parse failure is an `i64`
    // overflow — never an empty or malformed literal.
    let hi = lx.offset();
    let n = text.parse::<i64>().map_err(|_| Diagnostic::Parse {
        span: Span::new(lo, hi),
        msg: ParseError::IntLiteralOutOfRange,
    })?;
    Ok(Tok::Int(n))
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

/// Lex a punctuation / operator token. `c` is the already-peeked first
/// character and `lo` is its byte offset (the start of the token).
fn lex_symbol(lx: &mut Lexer, c: char, lo: u32) -> DResult<Tok> {
    let kind = match c {
        '(' => {
            lx.advance();
            Tok::LParen
        }
        ')' => {
            lx.advance();
            Tok::RParen
        }
        '=' => {
            lx.advance();
            Tok::Equals
        }
        '|' => {
            lx.advance();
            Tok::Pipe
        }
        ':' => {
            lx.advance();
            Tok::Colon
        }
        ',' => {
            lx.advance();
            Tok::Comma
        }
        '+' => {
            lx.advance();
            Tok::Plus
        }
        '-' => {
            lx.advance();
            if lx.peek() == Some('>') {
                lx.advance();
                Tok::Arrow
            } else {
                Tok::Minus
            }
        }
        '.' => {
            lx.advance();
            if lx.peek() == Some('.') {
                lx.advance();
                Tok::DotDot
            } else {
                // A lone `.` is not part of `..` and not attached to an ident.
                return Err(Diagnostic::Parse {
                    span: Span::new(lo, lx.offset()),
                    msg: ParseError::StrayDot,
                });
            }
        }
        other => {
            return Err(Diagnostic::Parse {
                span: Span::new(lo, lo + char_width(other)),
                msg: ParseError::UnknownChar(other),
            });
        }
    };
    Ok(kind)
}
