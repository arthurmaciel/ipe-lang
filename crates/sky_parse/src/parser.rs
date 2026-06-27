//! Recursive-descent parser for the Milestone-0 subset of Sky.
//!
//! Port of `Sky.Parse.{Module,Declaration,Type,Pattern,Expression}` narrowed to
//! the M0 grammar: a module header, imports, `type` unions, top-level value
//! bindings with optional type annotations, `case … of`, function application,
//! and `+`/`-` binary-operator chains.
//!
//! Every raise site emits a **coded, structured** [`ParseError`]: the generic
//! "expected X, found Y" family funnels through [`ParseError::UnexpectedToken`]
//! (SKY-P0001) carrying the found [`TokenKind`] and an [`ExpectedSet`];
//! truncated input becomes [`ParseError::UnexpectedEof`] (SKY-P0002) tagged with
//! the enclosing [`Construct`]; and each construct (module header, exposing list,
//! definition, type declaration, `case`, parenthesised group) has its own
//! defect-precise variant. Recursion is bounded by [`MAX_DEPTH`]; every
//! recursive entry threads a depth counter and fails with
//! [`ParseError::NestingTooDeep`] (SKY-P0003) before the native stack can
//! overflow on adversarial input.
//!
//! Qualified upper-case names in **type** and **pattern** position are rejected
//! with a typed error rather than collapsed into a non-reference AST: M0 does
//! not yet model `Module.Type` annotations or `Module.Ctor` patterns, so the
//! parser fails fast instead of silently dropping the qualifier (which the
//! canonicaliser would then resolve against the wrong name).

use sky_diagnostics::{
    CaseDefect, Construct, DResult, Diagnostic, Expected, ExpectedSet, ExposingDefect,
    HeaderDefect, IfDefect, LetDefect, Located, ParseError, Span, TokenKind, TypeDeclDefect,
};
use sky_intern::{Interner, Symbol};
use sky_syntax::{
    Ctor, Exposed, Exposing, Expr, Expr_, Import, LetBinding, Module, Pattern, Pattern_, Privacy,
    TypeAnnotation, Union, Value,
};

use crate::layout;
use crate::lexer::{Tok, Token};

/// Maximum recursion depth before the parser bails with
/// [`ParseError::NestingTooDeep`].
pub const MAX_DEPTH: u32 = 256;

pub struct Parser<'a> {
    toks: Vec<Token>,
    pos: usize,
    interner: &'a mut Interner,
}

/// One parsed top-level declaration, before annotations are matched to values.
enum Decl {
    Union(Located<Union>),
    Annotation(Symbol, Located<TypeAnnotation>),
    Value {
        name: Located<Symbol>,
        patterns: Vec<Pattern>,
        body: Expr,
    },
}

/// Map a concrete lexer token to its payload-free [`TokenKind`] category — the
/// "found" shape a [`ParseError::UnexpectedToken`] reports.
const fn tok_kind(t: &Tok) -> TokenKind {
    match t {
        Tok::Module => TokenKind::Module,
        Tok::Import => TokenKind::Import,
        Tok::Exposing => TokenKind::Exposing,
        Tok::As => TokenKind::As,
        Tok::Type => TokenKind::Type,
        Tok::Case => TokenKind::Case,
        Tok::Of => TokenKind::Of,
        Tok::Let => TokenKind::Let,
        Tok::In => TokenKind::In,
        Tok::If => TokenKind::If,
        Tok::Then => TokenKind::Then,
        Tok::Else => TokenKind::Else,
        Tok::LParen => TokenKind::LParen,
        Tok::RParen => TokenKind::RParen,
        Tok::Equals => TokenKind::Equals,
        Tok::Pipe => TokenKind::Pipe,
        Tok::Colon => TokenKind::Colon,
        Tok::Arrow => TokenKind::Arrow,
        Tok::DotDot => TokenKind::DotDot,
        Tok::Comma => TokenKind::Comma,
        Tok::Underscore => TokenKind::Underscore,
        Tok::Plus => TokenKind::Plus,
        Tok::Minus => TokenKind::Minus,
        Tok::Star => TokenKind::Star,
        Tok::Slash => TokenKind::Slash,
        Tok::SlashEq => TokenKind::SlashEq,
        Tok::EqEq => TokenKind::EqEq,
        Tok::Lt => TokenKind::Lt,
        Tok::Gt => TokenKind::Gt,
        Tok::Le => TokenKind::Le,
        Tok::Ge => TokenKind::Ge,
        Tok::AmpAmp => TokenKind::AmpAmp,
        Tok::PipePipe => TokenKind::PipePipe,
        Tok::Ident(_) => TokenKind::Ident,
        Tok::Int(_) => TokenKind::Int,
    }
}

impl<'a> Parser<'a> {
    pub const fn new(toks: Vec<Token>, interner: &'a mut Interner) -> Self {
        Self {
            toks,
            pos: 0,
            interner,
        }
    }

    // ---- token cursor -----------------------------------------------------

    fn peek(&self) -> Option<&Token> {
        self.toks.get(self.pos)
    }

    fn peek_kind(&self) -> Option<&Tok> {
        self.peek().map(|t| &t.kind)
    }

    /// Consume the next token. On end-of-input the error names `construct`, the
    /// enclosing grammar production that still required more tokens.
    fn bump(&mut self, construct: Construct) -> DResult<Token> {
        let tok = self
            .toks
            .get(self.pos)
            .cloned()
            .ok_or_else(|| Diagnostic::Parse {
                span: self.eof_err_span(),
                msg: ParseError::UnexpectedEof { construct },
            })?;
        self.pos += 1;
        Ok(tok)
    }

    /// Byte span to point at when input has run out: the end of the last token.
    fn eof_err_span(&self) -> Span {
        self.toks.last().map_or(Span::DUMMY, |t| t.span)
    }

    /// Byte span to point at "here": the next token, or end-of-input.
    fn err_here_span(&self) -> Span {
        self.peek().map_or_else(|| self.eof_err_span(), |t| t.span)
    }

    fn too_deep(&self, construct: Construct) -> Diagnostic {
        Diagnostic::Parse {
            span: self.err_here_span(),
            msg: ParseError::NestingTooDeep {
                construct,
                limit: u16::try_from(MAX_DEPTH).unwrap_or(u16::MAX),
            },
        }
    }

    // ---- typed-error constructors -----------------------------------------

    /// "found `<tok>`, expected `<set>`" — the SKY-P0001 funnel.
    fn unexpected_token(tok: &Token, expected: &[Expected]) -> Diagnostic {
        Diagnostic::Parse {
            span: tok.span,
            msg: ParseError::UnexpectedToken {
                found: tok_kind(&tok.kind),
                expected: ExpectedSet(expected.into()),
            },
        }
    }

    const fn malformed_header(span: Span, defect: HeaderDefect) -> Diagnostic {
        Diagnostic::Parse {
            span,
            msg: ParseError::MalformedModuleHeader(defect),
        }
    }

    const fn malformed_exposing(span: Span, defect: ExposingDefect) -> Diagnostic {
        Diagnostic::Parse {
            span,
            msg: ParseError::MalformedExposingList(defect),
        }
    }

    const fn malformed_type_decl(span: Span, defect: TypeDeclDefect) -> Diagnostic {
        Diagnostic::Parse {
            span,
            msg: ParseError::MalformedTypeDeclaration(defect),
        }
    }

    const fn malformed_case(span: Span, defect: CaseDefect) -> Diagnostic {
        Diagnostic::Parse {
            span,
            msg: ParseError::MalformedCase(defect),
        }
    }

    const fn malformed_let(span: Span, defect: LetDefect) -> Diagnostic {
        Diagnostic::Parse {
            span,
            msg: ParseError::MalformedLet(defect),
        }
    }

    const fn malformed_if(span: Span, defect: IfDefect) -> Diagnostic {
        Diagnostic::Parse {
            span,
            msg: ParseError::MalformedIf(defect),
        }
    }

    /// Require a closing `)`. The primary span points where the `)` was
    /// expected; `opener` is carried as the secondary span (SKY-P0050).
    fn close_paren(&mut self, opener: Span, construct: Construct) -> DResult<()> {
        match self.peek() {
            Some(t) if t.kind == Tok::RParen => {
                self.bump(construct)?;
                Ok(())
            }
            Some(t) => Err(Diagnostic::Parse {
                span: t.span,
                msg: ParseError::UnclosedDelimiter { opener },
            }),
            None => Err(Diagnostic::Parse {
                span: self.eof_err_span(),
                msg: ParseError::UnclosedDelimiter { opener },
            }),
        }
    }

    fn intern(&mut self, s: &str) -> DResult<Symbol> {
        self.interner.intern(s)
    }

    // ---- module -----------------------------------------------------------

    pub fn parse_module(&mut self) -> DResult<Module> {
        // The file must begin with `module`.
        let module_tok = match self.peek() {
            Some(t) if t.kind == Tok::Module => self.bump(Construct::ModuleHeader)?,
            Some(t) => {
                return Err(Self::malformed_header(
                    t.span,
                    HeaderDefect::NotModuleKeyword,
                ));
            }
            None => {
                return Err(Self::malformed_header(
                    self.eof_err_span(),
                    HeaderDefect::NotModuleKeyword,
                ));
            }
        };
        let name = self.parse_module_name()?;

        // The `exposing` keyword must follow the module name.
        match self.peek() {
            Some(t) if t.kind == Tok::Exposing => {
                self.bump(Construct::ModuleHeader)?;
            }
            Some(t) => {
                return Err(Self::malformed_header(
                    t.span,
                    HeaderDefect::MissingExposing,
                ));
            }
            None => {
                return Err(Self::malformed_header(
                    self.eof_err_span(),
                    HeaderDefect::MissingExposing,
                ));
            }
        }
        let exposing = self.parse_exposing()?;

        let mut imports = Vec::new();
        while self.peek_kind() == Some(&Tok::Import) {
            imports.push(self.parse_import()?);
        }

        let mut decls = Vec::new();
        while self.peek().is_some() {
            decls.push(self.parse_decl()?);
        }

        let header_span = Self::span_merge(module_tok.span, name.span);
        let (values, unions) = Self::assemble(decls);
        Ok(Module {
            name,
            exposing: Located::new(header_span, exposing),
            imports,
            values,
            unions,
        })
    }

    /// Split decls into values (with annotations attached) and unions.
    fn assemble(decls: Vec<Decl>) -> (Vec<Located<Value>>, Vec<Located<Union>>) {
        let mut unions = Vec::new();
        let mut annotations: Vec<(Symbol, Located<TypeAnnotation>)> = Vec::new();
        let mut values = Vec::new();
        for d in decls {
            match d {
                Decl::Union(u) => unions.push(u),
                Decl::Annotation(name, ty) => annotations.push((name, ty)),
                Decl::Value {
                    name,
                    patterns,
                    body,
                } => {
                    let type_annotation = annotations
                        .iter()
                        .rev()
                        .find(|(n, _)| *n == name.value)
                        .map(|(_, ty)| ty.clone());
                    let span = name.span;
                    values.push(Located::new(
                        span,
                        Value {
                            name,
                            patterns,
                            body,
                            type_annotation,
                        },
                    ));
                }
            }
        }
        (values, unions)
    }

    /// The module name in the header: a single (possibly dotted) identifier.
    /// A missing or non-identifier name is a malformed-header defect.
    fn parse_module_name(&mut self) -> DResult<Located<Vec<Symbol>>> {
        let Some(tok) = self.peek().cloned() else {
            return Err(Self::malformed_header(
                self.eof_err_span(),
                HeaderDefect::MissingName,
            ));
        };
        let Tok::Ident(text) = &tok.kind else {
            return Err(Self::malformed_header(
                tok.span,
                HeaderDefect::NameNotIdentifier,
            ));
        };
        self.pos += 1; // consume the name token (already cloned above)
        let segs = text
            .split('.')
            .map(|s| self.interner.intern(s))
            .collect::<DResult<Vec<Symbol>>>()?;
        Ok(Located::new(tok.span, segs))
    }

    /// A dotted import name, e.g. `Sky.Core.Prelude`.
    fn parse_dotted_name(&mut self) -> DResult<Located<Vec<Symbol>>> {
        let tok = self.bump(Construct::ModuleHeader)?;
        match &tok.kind {
            Tok::Ident(text) => {
                let segs = text
                    .split('.')
                    .map(|s| self.interner.intern(s))
                    .collect::<DResult<Vec<Symbol>>>()?;
                Ok(Located::new(tok.span, segs))
            }
            _ => Err(Self::unexpected_token(&tok, &[Expected::Identifier])),
        }
    }

    fn parse_exposing(&mut self) -> DResult<Exposing> {
        // Opening `(`.
        match self.peek() {
            Some(t) if t.kind == Tok::LParen => {
                self.bump(Construct::ExposingList)?;
            }
            Some(t) => {
                return Err(Self::malformed_exposing(
                    t.span,
                    ExposingDefect::MissingOpenParen,
                ));
            }
            None => {
                return Err(Self::malformed_exposing(
                    self.eof_err_span(),
                    ExposingDefect::MissingOpenParen,
                ));
            }
        }
        if self.peek_kind() == Some(&Tok::DotDot) {
            self.bump(Construct::ExposingList)?;
            self.expect_exposing_close()?;
            return Ok(Exposing::All);
        }
        let mut items = Vec::new();
        loop {
            items.push(self.parse_exposed()?);
            match self.peek() {
                Some(t) if t.kind == Tok::Comma => {
                    self.bump(Construct::ExposingList)?;
                }
                Some(t) if t.kind == Tok::RParen => {
                    self.bump(Construct::ExposingList)?;
                    break;
                }
                Some(t) => {
                    return Err(Self::malformed_exposing(
                        t.span,
                        ExposingDefect::BadSeparator,
                    ));
                }
                None => {
                    return Err(Self::malformed_exposing(
                        self.eof_err_span(),
                        ExposingDefect::BadSeparator,
                    ));
                }
            }
        }
        Ok(Exposing::List(items))
    }

    /// Require the `)` that closes an `exposing (..)` clause.
    fn expect_exposing_close(&mut self) -> DResult<()> {
        match self.peek() {
            Some(t) if t.kind == Tok::RParen => {
                self.bump(Construct::ExposingList)?;
                Ok(())
            }
            Some(t) => Err(Self::malformed_exposing(
                t.span,
                ExposingDefect::BadSeparator,
            )),
            None => Err(Self::malformed_exposing(
                self.eof_err_span(),
                ExposingDefect::BadSeparator,
            )),
        }
    }

    fn parse_exposed(&mut self) -> DResult<Located<Exposed>> {
        let tok = self.bump(Construct::ExposingList)?;
        let Tok::Ident(text) = &tok.kind else {
            return Err(Self::malformed_exposing(
                tok.span,
                ExposingDefect::NameNotIdentifier,
            ));
        };
        let sym = self.interner.intern(text)?;
        let is_type = text.chars().next().is_some_and(|c| c.is_ascii_uppercase());
        if is_type {
            let privacy = self.parse_privacy()?;
            Ok(Located::new(tok.span, Exposed::Type(sym, privacy)))
        } else {
            Ok(Located::new(tok.span, Exposed::Value(sym)))
        }
    }

    fn parse_privacy(&mut self) -> DResult<Privacy> {
        if self.peek_kind() != Some(&Tok::LParen) {
            return Ok(Privacy::Private);
        }
        self.bump(Construct::ExposingList)?; // (
        if self.peek_kind() == Some(&Tok::DotDot) {
            self.bump(Construct::ExposingList)?;
            self.expect_ctor_list_close()?;
            return Ok(Privacy::Public);
        }
        let mut ctors = Vec::new();
        loop {
            let tok = self.bump(Construct::ExposingList)?;
            match &tok.kind {
                Tok::Ident(text) => ctors.push(self.interner.intern(text)?),
                _ => {
                    return Err(Self::malformed_exposing(
                        tok.span,
                        ExposingDefect::MalformedCtorList,
                    ));
                }
            }
            match self.peek() {
                Some(t) if t.kind == Tok::Comma => {
                    self.bump(Construct::ExposingList)?;
                }
                Some(t) if t.kind == Tok::RParen => {
                    self.bump(Construct::ExposingList)?;
                    break;
                }
                Some(t) => {
                    return Err(Self::malformed_exposing(
                        t.span,
                        ExposingDefect::MalformedCtorList,
                    ));
                }
                None => {
                    return Err(Self::malformed_exposing(
                        self.eof_err_span(),
                        ExposingDefect::MalformedCtorList,
                    ));
                }
            }
        }
        Ok(Privacy::PublicCtors(ctors))
    }

    /// Require the `)` that closes a `Type(..)` constructor list.
    fn expect_ctor_list_close(&mut self) -> DResult<()> {
        match self.peek() {
            Some(t) if t.kind == Tok::RParen => {
                self.bump(Construct::ExposingList)?;
                Ok(())
            }
            Some(t) => Err(Self::malformed_exposing(
                t.span,
                ExposingDefect::MalformedCtorList,
            )),
            None => Err(Self::malformed_exposing(
                self.eof_err_span(),
                ExposingDefect::MalformedCtorList,
            )),
        }
    }

    fn parse_import(&mut self) -> DResult<Import> {
        // The caller has already peeked `import`.
        self.bump(Construct::ModuleHeader)?;
        let name = self.parse_dotted_name()?;
        let alias = if self.peek_kind() == Some(&Tok::As) {
            self.bump(Construct::ModuleHeader)?;
            let tok = self.bump(Construct::ModuleHeader)?;
            match &tok.kind {
                Tok::Ident(text) => Some(self.interner.intern(text)?),
                _ => {
                    return Err(Self::unexpected_token(&tok, &[Expected::Identifier]));
                }
            }
        } else {
            None
        };
        let exposing = if self.peek_kind() == Some(&Tok::Exposing) {
            self.bump(Construct::ModuleHeader)?;
            let clause = self.parse_exposing()?;
            Located::new(name.span, clause)
        } else {
            Located::new(name.span, Exposing::List(Vec::new()))
        };
        Ok(Import {
            name,
            alias,
            exposing,
        })
    }

    // ---- declarations -----------------------------------------------------

    fn parse_decl(&mut self) -> DResult<Decl> {
        if self.peek_kind() == Some(&Tok::Type) {
            return self.parse_union().map(Decl::Union);
        }
        let tok = self.bump(Construct::Definition)?;
        let Tok::Ident(text) = &tok.kind else {
            return Err(Self::unexpected_token(&tok, &[Expected::Identifier]));
        };
        let binding_name: Box<str> = text.as_str().into();
        let name_sym = self.interner.intern(text)?;
        let name = Located::new(tok.span, name_sym);
        let name_col = tok.col;

        if self.peek_kind() == Some(&Tok::Colon) {
            self.bump(Construct::Definition)?; // :
            let ty = self.parse_type(name_col, 0)?;
            return Ok(Decl::Annotation(name_sym, ty));
        }

        let mut patterns = Vec::new();
        while self.peek_is_pattern_atom_start() {
            patterns.push(self.parse_pattern_atom(0)?);
        }

        // The patterns must be followed by `=` before the body.
        match self.peek() {
            Some(t) if t.kind == Tok::Equals => {
                self.bump(Construct::Definition)?;
            }
            Some(t) => {
                return Err(Diagnostic::Parse {
                    span: t.span,
                    msg: ParseError::MissingEquals {
                        binding: binding_name,
                    },
                });
            }
            None => {
                return Err(Diagnostic::Parse {
                    span: self.eof_err_span(),
                    msg: ParseError::MissingEquals {
                        binding: binding_name,
                    },
                });
            }
        }
        let body = self.parse_expr(name_col, 0)?;
        Ok(Decl::Value {
            name,
            patterns,
            body,
        })
    }

    fn parse_union(&mut self) -> DResult<Located<Union>> {
        // The caller has already peeked `type`.
        let type_tok = self.bump(Construct::TypeDeclaration)?;
        let union_col = type_tok.col;
        let name_tok = self.bump(Construct::TypeDeclaration)?;
        let Tok::Ident(name_text) = &name_tok.kind else {
            return Err(Self::malformed_type_decl(
                name_tok.span,
                TypeDeclDefect::MissingName,
            ));
        };
        let name = Located::new(name_tok.span, self.interner.intern(name_text)?);

        let mut vars = Vec::new();
        loop {
            let var = match self.peek() {
                Some(Token {
                    kind: Tok::Ident(text),
                    span,
                    ..
                }) if text.chars().next().is_some_and(|c| c.is_ascii_lowercase()) => {
                    Some((text.clone(), *span))
                }
                _ => None,
            };
            let Some((text, span)) = var else { break };
            let sym = self.intern(&text)?;
            vars.push(Located::new(span, sym));
            self.bump(Construct::TypeDeclaration)?;
        }

        // The `=` before the constructors.
        match self.peek() {
            Some(t) if t.kind == Tok::Equals => {
                self.bump(Construct::TypeDeclaration)?;
            }
            Some(t) => {
                return Err(Self::malformed_type_decl(
                    t.span,
                    TypeDeclDefect::MissingEquals,
                ));
            }
            None => {
                return Err(Self::malformed_type_decl(
                    self.eof_err_span(),
                    TypeDeclDefect::MissingEquals,
                ));
            }
        }

        let mut ctors = Vec::new();
        loop {
            ctors.push(self.parse_ctor(union_col)?);
            if self.peek_kind() == Some(&Tok::Pipe) {
                self.bump(Construct::TypeDeclaration)?;
            } else {
                break;
            }
        }
        let last_span = ctors.last().map_or(name.span, |c| c.span);
        let span = Self::span_merge(type_tok.span, last_span);
        Ok(Located::new(span, Union { name, vars, ctors }))
    }

    fn parse_ctor(&mut self, threshold: u32) -> DResult<Located<Ctor>> {
        let tok = self.bump(Construct::TypeDeclaration)?;
        let Tok::Ident(text) = &tok.kind else {
            return Err(Self::malformed_type_decl(
                tok.span,
                TypeDeclDefect::CtorNotIdentifier,
            ));
        };
        // Constructors must start with an uppercase letter.
        if !text.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
            return Err(Self::malformed_type_decl(
                tok.span,
                TypeDeclDefect::CtorNotUppercase,
            ));
        }
        let name = self.interner.intern(text)?;
        let mut args = Vec::new();
        while self.peek_is_type_atom_in_block(threshold) {
            args.push(self.parse_type_atom(0)?.value);
        }
        Ok(Located::new(tok.span, Ctor { name, args }))
    }

    // ---- types ------------------------------------------------------------

    fn parse_type(&mut self, threshold: u32, depth: u32) -> DResult<Located<TypeAnnotation>> {
        if depth > MAX_DEPTH {
            return Err(self.too_deep(Construct::Type));
        }
        let left = self.parse_type_app(threshold, depth + 1)?;
        let arrow_in_block = self
            .peek()
            .is_some_and(|t| t.kind == Tok::Arrow && layout::continues_block(t, threshold));
        if arrow_in_block {
            self.bump(Construct::Type)?;
            let right = self.parse_type(threshold, depth + 1)?;
            let span = Self::span_merge(left.span, right.span);
            Ok(Located::new(
                span,
                TypeAnnotation::TLambda(Box::new(left.value), Box::new(right.value)),
            ))
        } else {
            Ok(left)
        }
    }

    fn parse_type_app(&mut self, threshold: u32, depth: u32) -> DResult<Located<TypeAnnotation>> {
        if depth > MAX_DEPTH {
            return Err(self.too_deep(Construct::Type));
        }
        let head = self.parse_type_atom(depth + 1)?;
        let mut args = Vec::new();
        let mut end = head.span;
        while self.peek_is_type_atom_in_block(threshold) {
            let arg = self.parse_type_atom(depth + 1)?;
            end = arg.span;
            args.push(arg.value);
        }
        if args.is_empty() {
            return Ok(head);
        }
        match head.value {
            TypeAnnotation::TType(q, segs, _) => {
                let span = Self::span_merge(head.span, end);
                Ok(Located::new(span, TypeAnnotation::TType(q, segs, args)))
            }
            // A type variable / arrow cannot be applied in the M0 grammar.
            _ => Err(Diagnostic::Parse {
                span: head.span,
                msg: ParseError::TypeArgsOnNonConstructor,
            }),
        }
    }

    fn parse_type_atom(&mut self, depth: u32) -> DResult<Located<TypeAnnotation>> {
        if depth > MAX_DEPTH {
            return Err(self.too_deep(Construct::Type));
        }
        let tok = self.bump(Construct::Type)?;
        match &tok.kind {
            Tok::LParen => {
                let opener = tok.span;
                let inner = self.parse_type(0, depth + 1)?;
                self.close_paren(opener, Construct::Type)?;
                Ok(Located::new(tok.span, inner.value))
            }
            Tok::Ident(text) => {
                let first_upper = text.chars().next().is_some_and(|c| c.is_ascii_uppercase());
                if first_upper {
                    // M0 does not model qualified types (`Module.Type`). Reject a
                    // dotted upper-case name rather than build a non-reference AST.
                    if text.contains('.') {
                        return Err(Diagnostic::Parse {
                            span: tok.span,
                            msg: ParseError::ExpectedType,
                        });
                    }
                    let empty = self.interner.intern("")?;
                    let seg = self.interner.intern(text)?;
                    Ok(Located::new(
                        tok.span,
                        TypeAnnotation::TType(empty, vec![seg], Vec::new()),
                    ))
                } else {
                    let sym = self.interner.intern(text)?;
                    Ok(Located::new(tok.span, TypeAnnotation::TVar(sym)))
                }
            }
            // A token that cannot begin a type.
            _ => Err(Diagnostic::Parse {
                span: tok.span,
                msg: ParseError::ExpectedType,
            }),
        }
    }

    fn peek_is_type_atom_in_block(&self, threshold: u32) -> bool {
        self.peek().is_some_and(|t| {
            layout::continues_block(t, threshold) && matches!(t.kind, Tok::LParen | Tok::Ident(_))
        })
    }

    // ---- expressions ------------------------------------------------------

    fn parse_expr(&mut self, threshold: u32, depth: u32) -> DResult<Expr> {
        if depth > MAX_DEPTH {
            return Err(self.too_deep(Construct::Expression));
        }
        let first = self.parse_app(threshold, depth + 1)?;
        let mut ops: Vec<(Expr, Located<Symbol>)> = Vec::new();
        let mut operand = first;
        while let Some((op, op_span)) = self.peek_binop(threshold) {
            let op_sym = self.intern(op)?;
            self.bump(Construct::Expression)?;
            ops.push((operand, Located::new(op_span, op_sym)));
            operand = self.parse_app(threshold, depth + 1)?;
        }
        if ops.is_empty() {
            Ok(operand)
        } else {
            let lo = ops.first().map_or(operand.span, |(e, _)| e.span);
            let span = Self::span_merge(lo, operand.span);
            Ok(Located::new(span, Expr_::Binops(ops, Box::new(operand))))
        }
    }

    fn parse_app(&mut self, threshold: u32, depth: u32) -> DResult<Expr> {
        if depth > MAX_DEPTH {
            return Err(self.too_deep(Construct::Expression));
        }
        let head = self.parse_atom(threshold, depth + 1)?;
        let mut args = Vec::new();
        let mut end = head.span;
        while self.peek_is_simple_atom_in_block(threshold) {
            let arg = self.parse_atom(threshold, depth + 1)?;
            end = arg.span;
            args.push(arg);
        }
        if args.is_empty() {
            Ok(head)
        } else {
            let span = Self::span_merge(head.span, end);
            Ok(Located::new(span, Expr_::Call(Box::new(head), args)))
        }
    }

    const fn is_simple_atom_start(kind: &Tok) -> bool {
        matches!(kind, Tok::LParen | Tok::Int(_) | Tok::Ident(_))
    }

    /// Peek a binary operator that continues the current block. Recognises the
    /// full M1-core set: arithmetic (`+ - * /`), comparison (`== /= < > <= >=`),
    /// and boolean (`&& ||`). Precedence + associativity are resolved later, at
    /// canonicalisation, from the flat chain this records.
    fn peek_binop(&self, threshold: u32) -> Option<(&'static str, Span)> {
        let tok = self.peek()?;
        if !layout::continues_block(tok, threshold) {
            return None;
        }
        let op = match tok.kind {
            Tok::Plus => "+",
            Tok::Minus => "-",
            Tok::Star => "*",
            Tok::Slash => "/",
            Tok::SlashEq => "/=",
            Tok::EqEq => "==",
            Tok::Lt => "<",
            Tok::Gt => ">",
            Tok::Le => "<=",
            Tok::Ge => ">=",
            Tok::AmpAmp => "&&",
            Tok::PipePipe => "||",
            _ => return None,
        };
        Some((op, tok.span))
    }

    fn peek_is_simple_atom_in_block(&self, threshold: u32) -> bool {
        self.peek().is_some_and(|t| {
            layout::continues_block(t, threshold) && Self::is_simple_atom_start(&t.kind)
        })
    }

    fn peek_aligned_at(&self, align: u32) -> bool {
        self.peek().is_some_and(|t| layout::aligned_at(t, align))
    }

    fn parse_atom(&mut self, threshold: u32, depth: u32) -> DResult<Expr> {
        if depth > MAX_DEPTH {
            return Err(self.too_deep(Construct::Expression));
        }
        if self.peek_kind() == Some(&Tok::Case) {
            return self.parse_case(threshold, depth + 1);
        }
        if self.peek_kind() == Some(&Tok::Let) {
            return self.parse_let(threshold, depth + 1);
        }
        if self.peek_kind() == Some(&Tok::If) {
            return self.parse_if(threshold, depth + 1);
        }
        let tok = self.bump(Construct::Expression)?;
        match &tok.kind {
            Tok::LParen => {
                let opener = tok.span;
                let inner = self.parse_expr(0, depth + 1)?;
                self.close_paren(opener, Construct::ParenGroup)?;
                Ok(Located::new(tok.span, inner.value))
            }
            Tok::Int(n) => Ok(Located::new(tok.span, Expr_::Int(*n))),
            Tok::Ident(text) => {
                let expr = self.ident_expr(text)?;
                Ok(Located::new(tok.span, expr))
            }
            _ => Err(Self::unexpected_token(&tok, &[Expected::Expression])),
        }
    }

    /// Resolve a (possibly dotted) identifier into a `VarLocal` / `VarQual`.
    fn ident_expr(&mut self, text: &str) -> DResult<Expr_> {
        let mut segs = text.split('.');
        let first = segs.next().unwrap_or("");
        let rest: Vec<&str> = segs.collect();
        if rest.is_empty() {
            return Ok(Expr_::VarLocal(self.interner.intern(first)?));
        }
        // Qualified: everything but the last segment is the qualifier.
        let mut all: Vec<&str> = Vec::with_capacity(rest.len() + 1);
        all.push(first);
        all.extend(rest);
        let Some((last, init)) = all.split_last() else {
            return Ok(Expr_::VarLocal(self.interner.intern(text)?));
        };
        let qualifier = init.join(".");
        let q = self.interner.intern(&qualifier)?;
        let name = self.interner.intern(last)?;
        Ok(Expr_::VarQual(q, name))
    }

    fn parse_case(&mut self, threshold: u32, depth: u32) -> DResult<Expr> {
        if depth > MAX_DEPTH {
            return Err(self.too_deep(Construct::Expression));
        }
        // The caller has already peeked `case`.
        let case_tok = self.bump(Construct::Expression)?;
        let scrutinee = self.parse_expr(threshold, depth + 1)?;

        // `of` after the scrutinee.
        match self.peek() {
            Some(t) if t.kind == Tok::Of => {
                self.bump(Construct::CaseBranch)?;
            }
            Some(t) => {
                return Err(Self::malformed_case(t.span, CaseDefect::MissingOf));
            }
            None => {
                return Err(Self::malformed_case(
                    self.eof_err_span(),
                    CaseDefect::MissingOf,
                ));
            }
        }

        let (arm_col, first_span) = match self.peek() {
            Some(t) => (t.col, t.span),
            None => {
                return Err(Self::malformed_case(
                    self.eof_err_span(),
                    CaseDefect::NoBranches,
                ));
            }
        };
        if arm_col <= threshold {
            return Err(Self::malformed_case(
                first_span,
                CaseDefect::FirstBranchNotIndented,
            ));
        }

        let mut arms = Vec::new();
        let mut end = scrutinee.span;
        while self.peek_aligned_at(arm_col) {
            let pat = self.parse_pattern(depth + 1)?;
            // `->` in this branch.
            match self.peek() {
                Some(t) if t.kind == Tok::Arrow => {
                    self.bump(Construct::CaseBranch)?;
                }
                Some(t) => {
                    return Err(Self::malformed_case(t.span, CaseDefect::MissingArrow));
                }
                None => {
                    return Err(Self::malformed_case(
                        self.eof_err_span(),
                        CaseDefect::MissingArrow,
                    ));
                }
            }
            let body = self.parse_expr(arm_col, depth + 1)?;
            end = body.span;
            arms.push((pat, body));
        }
        if arms.is_empty() {
            return Err(Self::malformed_case(
                self.err_here_span(),
                CaseDefect::NoBranches,
            ));
        }
        let span = Self::span_merge(case_tok.span, end);
        Ok(Located::new(span, Expr_::Case(Box::new(scrutinee), arms)))
    }

    /// Parse `let <bindings> in <body>`. The first binding's column fixes the
    /// alignment for the block; every later binding must start at that column.
    /// Each binding is a simple value (`name = expr`) whose body parses with the
    /// binding column as its layout threshold, so it stops at the next aligned
    /// binding or at `in`. Bindings are scoped sequentially (`let*`): each value
    /// sees the bindings before it, which matches the non-recursive nested-`Let`
    /// the lowerer produces.
    fn parse_let(&mut self, threshold: u32, depth: u32) -> DResult<Expr> {
        if depth > MAX_DEPTH {
            return Err(self.too_deep(Construct::Expression));
        }
        // The caller has already peeked `let`.
        let let_tok = self.bump(Construct::Let)?;

        // The first binding establishes the alignment column for the block. A
        // `let` immediately followed by `in` (or end of input) has no bindings.
        let binding_col = match self.peek() {
            Some(t) if t.kind == Tok::In => {
                return Err(Self::malformed_let(t.span, LetDefect::NoBindings));
            }
            Some(t) => t.col,
            None => {
                return Err(Self::malformed_let(
                    self.eof_err_span(),
                    LetDefect::NoBindings,
                ));
            }
        };

        let mut bindings = Vec::new();
        loop {
            bindings.push(self.parse_let_binding(binding_col, depth + 1)?);
            // Another binding continues only when the next token is an
            // identifier aligned at the binding column. `in` is its own token
            // (never an `Ident`), so it always ends the binding list.
            let more = self.peek().is_some_and(|t| {
                layout::aligned_at(t, binding_col) && matches!(t.kind, Tok::Ident(_))
            });
            if !more {
                break;
            }
        }

        // `in` after the bindings.
        match self.peek() {
            Some(t) if t.kind == Tok::In => {
                self.bump(Construct::Let)?;
            }
            Some(t) => return Err(Self::malformed_let(t.span, LetDefect::MissingIn)),
            None => {
                return Err(Self::malformed_let(
                    self.eof_err_span(),
                    LetDefect::MissingIn,
                ));
            }
        }

        let body = self.parse_expr(threshold, depth + 1)?;
        let span = Self::span_merge(let_tok.span, body.span);
        Ok(Located::new(span, Expr_::Let(bindings, Box::new(body))))
    }

    /// Parse `if <cond> then <a> else <b>`, including any `else if` chain. Each
    /// condition and branch parses as a full expression at `threshold`; the
    /// `then` / `else` / `if` keyword tokens delimit them, since the expression
    /// parser never consumes a keyword as an application argument or operator.
    /// The result is `If [(cond, branch), …] else`, mirroring the Haskell
    /// compiler's `Src.If [(Expr, Expr)] Expr` — the leading `if` plus every
    /// `else if`, then the mandatory final `else`.
    fn parse_if(&mut self, threshold: u32, depth: u32) -> DResult<Expr> {
        if depth > MAX_DEPTH {
            return Err(self.too_deep(Construct::If));
        }
        // The caller has already peeked `if`.
        let if_tok = self.bump(Construct::If)?;

        let mut branches = Vec::new();
        loop {
            // Reject an absent condition cleanly rather than letting the
            // expression parser stumble over the `then` / `else` keyword.
            match self.peek() {
                Some(t) if matches!(t.kind, Tok::Then | Tok::Else) => {
                    return Err(Self::malformed_if(t.span, IfDefect::MissingCondition));
                }
                None => {
                    return Err(Self::malformed_if(
                        self.eof_err_span(),
                        IfDefect::MissingCondition,
                    ));
                }
                _ => {}
            }
            let cond = self.parse_expr(threshold, depth + 1)?;

            // `then` after the condition.
            match self.peek() {
                Some(t) if t.kind == Tok::Then => {
                    self.bump(Construct::If)?;
                }
                Some(t) => return Err(Self::malformed_if(t.span, IfDefect::MissingThen)),
                None => {
                    return Err(Self::malformed_if(
                        self.eof_err_span(),
                        IfDefect::MissingThen,
                    ));
                }
            }

            let branch = self.parse_expr(threshold, depth + 1)?;
            branches.push((cond, branch));

            // `else` after the `then` branch (mandatory — every `if` is an
            // expression with both outcomes).
            match self.peek() {
                Some(t) if t.kind == Tok::Else => {
                    self.bump(Construct::If)?;
                }
                Some(t) => return Err(Self::malformed_if(t.span, IfDefect::MissingElse)),
                None => {
                    return Err(Self::malformed_if(
                        self.eof_err_span(),
                        IfDefect::MissingElse,
                    ));
                }
            }

            // `else if` continues the chain with another `(cond, branch)` pair;
            // a plain `else` ends it with the final expression below.
            if self.peek_kind() == Some(&Tok::If) {
                self.bump(Construct::If)?;
                continue;
            }
            break;
        }

        let else_branch = self.parse_expr(threshold, depth + 1)?;
        let span = Self::span_merge(if_tok.span, else_branch.span);
        Ok(Located::new(
            span,
            Expr_::If(branches, Box::new(else_branch)),
        ))
    }

    /// Parse a single `name = body` let binding. `binding_col` is the block's
    /// alignment column, used as the body's layout threshold so the body stops
    /// at the next aligned binding or at `in`.
    fn parse_let_binding(&mut self, binding_col: u32, depth: u32) -> DResult<LetBinding> {
        if depth > MAX_DEPTH {
            return Err(self.too_deep(Construct::Let));
        }
        let name_tok = self.bump(Construct::Let)?;
        let Tok::Ident(text) = &name_tok.kind else {
            return Err(Self::malformed_let(
                name_tok.span,
                LetDefect::BindingNameNotLower,
            ));
        };
        // A value binding name is a plain lowercase identifier; reject an
        // uppercase (constructor) or dotted (qualified) name.
        if text.contains('.') || text.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
            return Err(Self::malformed_let(
                name_tok.span,
                LetDefect::BindingNameNotLower,
            ));
        }
        let name_sym = self.interner.intern(text)?;
        let name = Located::new(name_tok.span, name_sym);

        // `=` after the name. A parameter here (`let f x = …`) lands as a
        // non-`=` token, producing the clean MissingEquals rejection — M1 has no
        // function bindings in `let`.
        match self.peek() {
            Some(t) if t.kind == Tok::Equals => {
                self.bump(Construct::Let)?;
            }
            Some(t) => return Err(Self::malformed_let(t.span, LetDefect::MissingEquals)),
            None => {
                return Err(Self::malformed_let(
                    self.eof_err_span(),
                    LetDefect::MissingEquals,
                ));
            }
        }

        let body = self.parse_expr(binding_col, depth + 1)?;
        Ok(LetBinding { name, body })
    }

    // ---- patterns ---------------------------------------------------------

    /// A full pattern, gathering constructor sub-patterns (case-arm position).
    fn parse_pattern(&mut self, depth: u32) -> DResult<Pattern> {
        if depth > MAX_DEPTH {
            return Err(self.too_deep(Construct::Pattern));
        }
        let head = self.parse_pattern_atom(depth + 1)?;
        // Only a constructor head may take sub-patterns.
        if let Pattern_::PCtor(name, mods, _) = head.value.clone() {
            let mut sub = Vec::new();
            let mut end = head.span;
            while self.peek_is_pattern_atom_start() {
                let p = self.parse_pattern_atom(depth + 1)?;
                end = p.span;
                sub.push(p);
            }
            if sub.is_empty() {
                return Ok(head);
            }
            let span = Self::span_merge(head.span, end);
            return Ok(Located::new(span, Pattern_::PCtor(name, mods, sub)));
        }
        Ok(head)
    }

    fn parse_pattern_atom(&mut self, depth: u32) -> DResult<Pattern> {
        if depth > MAX_DEPTH {
            return Err(self.too_deep(Construct::Pattern));
        }
        let tok = self.bump(Construct::Pattern)?;
        match &tok.kind {
            Tok::Underscore => Ok(Located::new(tok.span, Pattern_::PAnything)),
            Tok::LParen => {
                let opener = tok.span;
                let inner = self.parse_pattern(depth + 1)?;
                self.close_paren(opener, Construct::Pattern)?;
                Ok(Located::new(tok.span, inner.value))
            }
            Tok::Ident(text) => {
                let first_upper = text.chars().next().is_some_and(|c| c.is_ascii_uppercase());
                if first_upper {
                    // M0 does not model qualified constructors (`Module.Ctor`) in
                    // pattern position. Reject a dotted upper-case name rather than
                    // drop the qualifier into a non-reference AST.
                    if text.contains('.') {
                        return Err(Self::unexpected_token(&tok, &[Expected::Constructor]));
                    }
                    let name = self.interner.intern(text)?;
                    Ok(Located::new(
                        tok.span,
                        Pattern_::PCtor(name, Vec::new(), Vec::new()),
                    ))
                } else {
                    let sym = self.interner.intern(text)?;
                    Ok(Located::new(tok.span, Pattern_::PVar(sym)))
                }
            }
            _ => Err(Self::unexpected_token(&tok, &[Expected::Pattern])),
        }
    }

    fn peek_is_pattern_atom_start(&self) -> bool {
        matches!(
            self.peek_kind(),
            Some(&(Tok::Underscore | Tok::LParen | Tok::Ident(_)))
        )
    }

    // ---- span helper ------------------------------------------------------

    fn span_merge(a: Span, b: Span) -> Span {
        Span::new(a.lo.min(b.lo), a.hi.max(b.hi))
    }
}
