//! Recursive-descent parser for the supported subset of Ipê.
//!
//! Port of `Ipe.Parse.{Module,Declaration,Type,Pattern,Expression}` narrowed to
//! the supported grammar: a module header, imports, `type` unions, top-level value
//! bindings with optional type annotations, `case … of`, function application,
//! and `+`/`-` binary-operator chains.
//!
//! Every raise site emits a **coded, structured** [`ParseError`]: the generic
//! "expected X, found Y" family funnels through [`ParseError::UnexpectedToken`]
//! (IPE-P0001) carrying the found [`TokenKind`] and an [`ExpectedSet`];
//! truncated input becomes [`ParseError::UnexpectedEof`] (IPE-P0002) tagged with
//! the enclosing [`Construct`]; and each construct (module header, exposing list,
//! definition, type declaration, `case`, parenthesised group) has its own
//! defect-precise variant. Recursion is bounded by [`MAX_DEPTH`]; every
//! recursive entry threads a depth counter and fails with
//! [`ParseError::NestingTooDeep`] (IPE-P0003) before the native stack can
//! overflow on adversarial input.
//!
//! Qualified upper-case names in **type** and **pattern** position are rejected
//! with a typed error rather than collapsed into a non-reference AST: the parser
//! does not yet model `Module.Type` annotations or `Module.Ctor` patterns, so the
//! parser fails fast instead of silently dropping the qualifier (which the
//! canonicaliser would then resolve against the wrong name).

use ipe_diagnostics::{
    CaseDefect, Construct, DResult, Diagnostic, Expected, ExpectedSet, ExposingDefect,
    HeaderDefect, IfDefect, LetDefect, Located, ParseError, Span, TokenKind, TypeDeclDefect,
};
use ipe_intern::{Interner, Symbol};
use ipe_syntax::{
    Ctor, Exposed, Exposing, Expr, Expr_, Import, LetBinding, Module, Pattern, Pattern_, Privacy,
    TypeAlias, TypeAnnotation, Union, Value,
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

/// The result of splitting parsed declarations into their three kinds: value
/// bindings (with annotations attached), union types, and type aliases.
type AssembledDecls = (
    Vec<Located<Value>>,
    Vec<Located<Union>>,
    Vec<Located<TypeAlias>>,
);

/// One parsed top-level declaration, before annotations are matched to values.
enum Decl {
    Union(Located<Union>),
    Alias(Located<TypeAlias>),
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
        Tok::Do => TokenKind::Do,
        Tok::ParallelDo => TokenKind::ParallelDo,
        Tok::LParen => TokenKind::LParen,
        Tok::RParen => TokenKind::RParen,
        Tok::LBrace => TokenKind::LBrace,
        Tok::RBrace => TokenKind::RBrace,
        Tok::LBracket => TokenKind::LBracket,
        Tok::RBracket => TokenKind::RBracket,
        Tok::ColonColon => TokenKind::ColonColon,
        Tok::Equals => TokenKind::Equals,
        Tok::Pipe => TokenKind::Pipe,
        Tok::Colon => TokenKind::Colon,
        Tok::Arrow => TokenKind::Arrow,
        Tok::LeftArrow => TokenKind::LeftArrow,
        Tok::Backslash => TokenKind::Backslash,
        Tok::DotDot => TokenKind::DotDot,
        Tok::Dot => TokenKind::Dot,
        Tok::Comma => TokenKind::Comma,
        Tok::Underscore => TokenKind::Underscore,
        Tok::Plus => TokenKind::Plus,
        Tok::PlusPlus => TokenKind::PlusPlus,
        Tok::Minus => TokenKind::Minus,
        Tok::Star => TokenKind::Star,
        Tok::Slash => TokenKind::Slash,
        Tok::SlashEq => TokenKind::SlashEq,
        Tok::SlashSlash => TokenKind::SlashSlash,
        Tok::EqEq => TokenKind::EqEq,
        Tok::Lt => TokenKind::Lt,
        Tok::Gt => TokenKind::Gt,
        Tok::Le => TokenKind::Le,
        Tok::Ge => TokenKind::Ge,
        Tok::AmpAmp => TokenKind::AmpAmp,
        Tok::PipePipe => TokenKind::PipePipe,
        Tok::PipeGt => TokenKind::PipeGt,
        Tok::LtPipe => TokenKind::LtPipe,
        Tok::GtGt => TokenKind::GtGt,
        Tok::LtLt => TokenKind::LtLt,
        Tok::Ident(_) => TokenKind::Ident,
        Tok::Int(_) => TokenKind::Int,
        Tok::Float(_) => TokenKind::Float,
        Tok::Str(_) | Tok::TripleStr(_) => TokenKind::Str,
        Tok::Char(_) => TokenKind::Char,
    }
}

/// One statement inside a `do` block, before desugaring to `Task.andThen`.
///
/// `Bind` and `Let` carry the span of their `<-` / `=` operator token: the
/// desugar stamps its synthetic nodes with that span (never an operand's), so
/// the type checker's `(home, span)` region map cannot collide a synthetic node
/// with a real one — the same discipline the `>>` / `<<` desugar uses.
enum DoStmt {
    /// `p <- e` — run the task `e` and bind its result to `p` for the rest.
    Bind(Pattern, Span, Expr),
    /// `p = e` — a pure `let` binding in scope for the rest of the block.
    Let(Pattern, Span, Expr),
    /// `e` — run the task `e`; a non-final one discards its result, the final
    /// one is the block's value.
    Run(Expr),
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

    /// "found `<tok>`, expected `<set>`" — the IPE-P0001 funnel.
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
    /// expected; `opener` is carried as the secondary span (IPE-P0050).
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
        let (values, unions, aliases) = Self::assemble(decls);
        Ok(Module {
            name,
            exposing: Located::new(header_span, exposing),
            imports,
            values,
            unions,
            aliases,
        })
    }

    /// Split decls into values (with annotations attached), unions, and aliases.
    fn assemble(decls: Vec<Decl>) -> AssembledDecls {
        let mut unions = Vec::new();
        let mut aliases = Vec::new();
        let mut annotations: Vec<(Symbol, Located<TypeAnnotation>)> = Vec::new();
        let mut values = Vec::new();
        for d in decls {
            match d {
                Decl::Union(u) => unions.push(u),
                Decl::Alias(a) => aliases.push(a),
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
        (values, unions, aliases)
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

    /// A dotted import name, e.g. `Ipe.Prelude`.
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
            // `type alias …` and `type …` (a union) share the `type` keyword; the
            // disambiguator is the soft keyword `alias` (a plain identifier) in
            // the next slot.
            if self.peek_is_alias_keyword() {
                return self.parse_type_alias().map(Decl::Alias);
            }
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
        while self.peek_is_binder_atom_start() {
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

    /// True when the token after the peeked `type` is the soft keyword `alias`.
    /// `alias` is a plain identifier (not a reserved token), so a union named by
    /// a user who happens to also write `alias` is distinguished only here, at
    /// the one site where the look-ahead is meaningful.
    fn peek_is_alias_keyword(&self) -> bool {
        matches!(
            self.toks.get(self.pos + 1).map(|t| &t.kind),
            Some(Tok::Ident(text)) if text.as_str() == "alias"
        )
    }

    /// Parse `type alias Name [vars…] = T`. Type parameters are captured in
    /// `vars` so canonicalisation can substitute each use site's type arguments
    /// for them and expand the alias body; both the parametric and the
    /// non-parametric form are supported.
    fn parse_type_alias(&mut self) -> DResult<Located<TypeAlias>> {
        // The caller has already established `type` followed by `alias`.
        let type_tok = self.bump(Construct::TypeDeclaration)?; // `type`
        let alias_col = type_tok.col;
        self.bump(Construct::TypeDeclaration)?; // `alias`

        let name_tok = self.bump(Construct::TypeDeclaration)?;
        let Tok::Ident(name_text) = &name_tok.kind else {
            return Err(Self::malformed_type_decl(
                name_tok.span,
                TypeDeclDefect::MissingName,
            ));
        };
        let name = Located::new(name_tok.span, self.interner.intern(name_text)?);

        // Declared type parameters (lowercase identifiers), mirroring `parse_union`.
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

        // The `=` before the aliased type.
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

        let body = self.parse_type(alias_col, 0)?;
        let span = Self::span_merge(type_tok.span, body.span);
        Ok(Located::new(span, TypeAlias { name, vars, body }))
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
            // A type variable / arrow cannot be applied in the grammar.
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
                // `()` — the unit type.
                if self.peek_kind() == Some(&Tok::RParen) {
                    let close = self.expect_rparen(opener, Construct::Type)?;
                    let span = Self::span_merge(opener, close);
                    return Ok(Located::new(span, TypeAnnotation::TUnit));
                }
                let first = self.parse_type(0, depth + 1)?;
                // No following comma → a parenthesised single type. Unwrap to the
                // inner type, spanning from the `(` to the `)` so explicit
                // grouping (e.g. `(a -> b) -> c`) is honoured.
                if self.peek_kind() != Some(&Tok::Comma) {
                    let close = self.expect_rparen(opener, Construct::Type)?;
                    let span = Self::span_merge(opener, close);
                    return Ok(Located::new(span, first.value));
                }
                // One or more comma-separated members → a tuple type.
                let mut elems = vec![first.value];
                while self.peek_kind() == Some(&Tok::Comma) {
                    self.bump(Construct::Type)?;
                    elems.push(self.parse_type(0, depth + 1)?.value);
                }
                let close = self.expect_rparen(opener, Construct::Type)?;
                let span = Self::span_merge(opener, close);
                Ok(Located::new(span, TypeAnnotation::TTuple(elems)))
            }
            // A closed record type `{ field : T, ... }`.
            Tok::LBrace => self.parse_record_type(tok.span, depth + 1),
            Tok::Ident(text) => {
                let first_upper = text.chars().next().is_some_and(|c| c.is_ascii_uppercase());
                if first_upper {
                    // A dotted uppercase name is a qualified type: split on `.` so
                    // the last segment is the type name and the rest is the module
                    // qualifier. E.g. `JsonDec.Decoder` → qualifier="JsonDec",
                    // name="Decoder". An unqualified name has an empty qualifier.
                    if text.contains('.') {
                        let dot = text.rfind('.').unwrap_or(0);
                        let qualifier = &text[..dot];
                        let name = &text[dot + 1..];
                        let q = self.interner.intern(qualifier)?;
                        let seg = self.interner.intern(name)?;
                        return Ok(Located::new(
                            tok.span,
                            TypeAnnotation::TType(q, vec![seg], Vec::new()),
                        ));
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
            layout::continues_block(t, threshold)
                && matches!(t.kind, Tok::LParen | Tok::LBrace | Tok::Ident(_))
        })
    }

    /// Parse a closed record type `{ field : T, ... }`, the opening `{` already
    /// consumed (its span is `opener`). The empty record `{}` is valid and
    /// produces a `TRecord` with an empty field list (mirrors the Haskell
    /// compiler's behaviour). Each non-empty field is a lowercase name, a `:`,
    /// then its type; fields are comma-separated and the list is closed by `}`.
    /// Duplicate field names are not rejected here (a later stage owns that),
    /// matching how the record *literal* parser stays purely syntactic.
    fn parse_record_type(&mut self, opener: Span, depth: u32) -> DResult<Located<TypeAnnotation>> {
        if depth > MAX_DEPTH {
            return Err(self.too_deep(Construct::Type));
        }
        // `{}` (the empty record type) — valid, produces `TRecord []`.
        // Mirrors the Haskell reference: `Just '}' -> char '}'  >> return (TRecord [] Nothing)`.
        if let Some(t) = self.peek().filter(|t| t.kind == Tok::RBrace) {
            let close = t.span;
            self.bump(Construct::Type)?;
            let span = Self::span_merge(opener, close);
            return Ok(Located::new(span, TypeAnnotation::TRecord(Vec::new())));
        }
        // The row-polymorphic (open) record type `{ r | field : T, ... }`: a
        // lowercase row variable followed by `|`, then the constrained fields.
        // A closed record's first field is a name followed by `:`, so a `|` in
        // the second token position disambiguates without backtracking. (A `|`
        // never appears here in its record-update meaning — that sigil lives in
        // expression land, never type land.)
        if self.peek_is_open_record_intro() {
            let row_var = self.parse_record_field_name()?;
            self.bump(Construct::Type)?; // the `|`
            let fields = self.parse_record_type_fields(depth)?;
            let close = self.expect_record_close()?;
            let span = Self::span_merge(opener, close);
            return Ok(Located::new(
                span,
                TypeAnnotation::TRecordOpen(row_var.value, fields),
            ));
        }
        let fields = self.parse_record_type_fields(depth)?;
        let close = self.expect_record_close()?;
        let span = Self::span_merge(opener, close);
        Ok(Located::new(span, TypeAnnotation::TRecord(fields)))
    }

    /// True when the record-type parser sits at a row-polymorphic opener
    /// `<lowerVar> |` — a lowercase identifier immediately followed by a `|`.
    /// Non-consuming (two-token lookahead), so the closed-record path is
    /// untouched when this returns false. A qualified/uppercase name is not a
    /// row variable, so only a bare lowercase `Ident` qualifies.
    fn peek_is_open_record_intro(&self) -> bool {
        let is_row_var = self.peek_kind().is_some_and(|k| match k {
            Tok::Ident(text) => {
                !text.contains('.') && text.chars().next().is_some_and(|c| !c.is_ascii_uppercase())
            }
            _ => false,
        });
        is_row_var && self.toks.get(self.pos + 1).map(|t| &t.kind) == Some(&Tok::Pipe)
    }

    /// Parse a non-empty, comma-separated `field : Type` list, stopping before
    /// the closing `}`. Shared by the closed and open record-type arms.
    fn parse_record_type_fields(&mut self, depth: u32) -> DResult<Vec<(Symbol, TypeAnnotation)>> {
        let mut fields = Vec::new();
        loop {
            let name = self.parse_record_field_name()?;
            self.expect_field_colon()?;
            let ty = self.parse_type(0, depth + 1)?;
            fields.push((name.value, ty.value));
            match self.peek() {
                Some(t) if t.kind == Tok::Comma => {
                    self.bump(Construct::Type)?;
                }
                _ => return Ok(fields),
            }
        }
    }

    /// Consume the `}` closing a record type, returning its span. Anything else
    /// (mid field-list) is the same "expected `,` or `}`" error the closed
    /// record loop reported before this was factored out.
    fn expect_record_close(&mut self) -> DResult<Span> {
        match self.peek() {
            Some(t) if t.kind == Tok::RBrace => {
                let close = t.span;
                self.bump(Construct::Type)?;
                Ok(close)
            }
            Some(t) => Err(Self::unexpected_token(
                t,
                &[Expected::Comma, Expected::RBrace],
            )),
            None => Err(self.record_eof()),
        }
    }

    /// Consume the `:` separating a record-type field name from its type.
    fn expect_field_colon(&mut self) -> DResult<()> {
        match self.peek() {
            Some(t) if t.kind == Tok::Colon => {
                self.bump(Construct::Type)?;
                Ok(())
            }
            Some(t) => Err(Self::unexpected_token(t, &[Expected::Colon])),
            None => Err(self.record_eof()),
        }
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
        let head = self.parse_atom_postfix(threshold, depth + 1)?;
        let mut args = Vec::new();
        let mut end = head.span;
        while self.peek_is_simple_atom_in_block(threshold) {
            let arg = self.parse_atom_postfix(threshold, depth + 1)?;
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
        matches!(
            kind,
            Tok::LParen
                | Tok::LBrace
                | Tok::LBracket
                | Tok::Int(_)
                | Tok::Float(_)
                | Tok::Str(_)
                | Tok::TripleStr(_)
                | Tok::Char(_)
                | Tok::Ident(_)
                // A leading `.field` begins a first-class accessor atom, so
                // `List.map .name xs` gathers `.name` as an argument.
                | Tok::Dot
        )
    }

    /// Peek a binary operator that continues the current block. Recognises the
    /// full core set: arithmetic (`+ - * /`), string append (`++`),
    /// comparison (`== /= < > <= >=`), and boolean (`&& ||`). Precedence +
    /// associativity are resolved later, at canonicalisation, from the flat
    /// chain this records.
    fn peek_binop(&self, threshold: u32) -> Option<(&'static str, Span)> {
        let tok = self.peek()?;
        if !layout::continues_block(tok, threshold) {
            return None;
        }
        let op = match tok.kind {
            Tok::Plus => "+",
            Tok::PlusPlus => "++",
            Tok::Minus => "-",
            Tok::Star => "*",
            Tok::Slash => "/",
            Tok::SlashEq => "/=",
            Tok::SlashSlash => "//",
            Tok::EqEq => "==",
            Tok::Lt => "<",
            Tok::Gt => ">",
            Tok::Le => "<=",
            Tok::Ge => ">=",
            Tok::AmpAmp => "&&",
            Tok::PipePipe => "||",
            Tok::PipeGt => "|>",
            Tok::LtPipe => "<|",
            Tok::GtGt => ">>",
            Tok::LtLt => "<<",
            Tok::ColonColon => "::",
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

    /// Parse an atom followed by zero or more postfix field accesses.
    ///
    /// A bare `ident.field` run is one [`Tok::Ident`] handled in [`Self::ident_expr`];
    /// this method covers field access on a *non-identifier* atom — most importantly
    /// a parenthesised expression, `(record).field` or `(wrap 1).value`. The lexer
    /// emits a [`Tok::Dot`] for each such `.`, each immediately followed by an
    /// identifier token; a chained access like `(r).a.b` lexes the `a.b` as a single
    /// dotted identifier, so its segments are split back out here. Field access binds
    /// tighter than application, so it is resolved per-atom before [`Self::parse_app`]
    /// gathers arguments.
    ///
    /// The `.` must sit flush against the atom (`(r).value`). A space before it —
    /// `f .value` — is the accessor-function reading (`.value` applied to `f`), a
    /// different program the type system cannot yet express; it is rejected with a
    /// teaching diagnostic rather than silently parsed as field access.
    fn parse_atom_postfix(&mut self, threshold: u32, depth: u32) -> DResult<Expr> {
        let mut expr = self.parse_atom(threshold, depth + 1)?;
        while self.peek_kind() == Some(&Tok::Dot) {
            let dot_span = self.err_here_span();
            if dot_span.lo != expr.span.hi {
                // A space before the `.` is NOT field access on `expr`; it is the
                // first-class accessor reading — `f .x` is `.x` (the getter
                // `\p -> p.x`) applied to `f`. Stop the postfix run and leave the
                // `.x` for `parse_app` to gather as an argument atom.
                break;
            }
            self.bump(Construct::Expression)?;
            let tok = self.bump(Construct::Expression)?;
            let Tok::Ident(text) = &tok.kind else {
                return Err(Self::unexpected_token(&tok, &[Expected::Identifier]));
            };
            // `a.b.c` after a dot lexes as one dotted identifier; each segment is a
            // separate field access on the running expression.
            for (seg_count, seg) in (0_u32..).zip(text.split('.')) {
                if seg_count > MAX_DEPTH {
                    return Err(self.too_deep(Construct::Expression));
                }
                let field = Located::new(tok.span, self.interner.intern(seg)?);
                let span = Self::span_merge(expr.span, tok.span);
                expr = Located::new(span, Expr_::Access(Box::new(expr), field));
            }
        }
        Ok(expr)
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
        if self.peek_kind() == Some(&Tok::Do) {
            return self.parse_do(threshold, depth + 1);
        }
        if self.peek_kind() == Some(&Tok::ParallelDo) {
            return self.parse_parallel_do(threshold, depth + 1);
        }
        if self.peek_kind() == Some(&Tok::Backslash) {
            return self.parse_lambda(threshold, depth + 1);
        }
        let tok = self.bump(Construct::Expression)?;
        match &tok.kind {
            Tok::LParen => self.parse_paren_or_tuple(tok.span, depth + 1),
            Tok::LBrace => self.parse_record(tok.span, depth + 1),
            Tok::LBracket => self.parse_list(tok.span, depth + 1),
            Tok::Int(n) => Ok(Located::new(tok.span, Expr_::Int(*n))),
            Tok::Float(f) => Ok(Located::new(tok.span, Expr_::Float(*f))),
            Tok::Str(s) => Ok(Located::new(tok.span, Expr_::Str(s.clone()))),
            // Triple-quoted strings carry raw content; the canonicaliser desugars
            // `{{expr}}` interpolation at name-resolution time. Mirrors the Haskell
            // parser's `MultiLine str -> return (Src.MultilineStr str)` arm.
            Tok::TripleStr(s) => Ok(Located::new(tok.span, Expr_::MultilineStr(s.clone()))),
            Tok::Char(c) => Ok(Located::new(tok.span, Expr_::Char(c.clone()))),
            Tok::Minus => self.parse_negative_literal(tok.span, threshold, depth),
            Tok::Ident(text) => {
                let expr = self.ident_expr(text, tok.span)?;
                Ok(Located::new(tok.span, expr))
            }
            // A leading `.field` in atom position is the first-class accessor — a
            // value of type `{ r | field : a } -> a`. It desugars here to the
            // getter lambda `\<fresh> -> <fresh>.field`, reusing the ordinary
            // record-access path (deferred field access + monomorphic pinning) so
            // no new type/canon/backend node is needed.
            Tok::Dot => self.parse_field_accessor(tok.span, depth),
            _ => Err(Self::unexpected_token(&tok, &[Expected::Expression])),
        }
    }

    /// Build the desugared getter lambda for a first-class accessor `.field`, the
    /// leading `.` already consumed (its span is `dot_span`). The next token is
    /// the field identifier (`lex_dot` only yields [`Tok::Dot`] when an identifier
    /// start follows). A dotted run `.a.b` lexes the `a.b` as one identifier; its
    /// segments become a nested `Access` chain over the synthesised parameter,
    /// exactly as [`Self::parse_atom_postfix`] does for `(r).a.b`.
    fn parse_field_accessor(&mut self, dot_span: Span, depth: u32) -> DResult<Expr> {
        if depth > MAX_DEPTH {
            return Err(self.too_deep(Construct::Expression));
        }
        let tok = self.bump(Construct::Expression)?;
        let Tok::Ident(text) = &tok.kind else {
            return Err(Self::unexpected_token(&tok, &[Expected::Identifier]));
        };
        // The synthesised parameter. Its name need only be a valid emitted Rust
        // identifier: the parameter is the innermost binder of this lambda, so the
        // body's `VarLocal` resolves to it by lexical scoping even if a user
        // binding of the same name exists further out (which the body never
        // references anyway).
        let param_sym = self.interner.intern("ipe_accessor_arg")?;
        let param = Located::new(dot_span, Pattern_::PVar(param_sym));
        let mut body = Located::new(dot_span, Expr_::VarLocal(param_sym));
        for (seg_count, seg) in (0_u32..).zip(text.split('.')) {
            if seg_count > MAX_DEPTH {
                return Err(self.too_deep(Construct::Expression));
            }
            let field = Located::new(tok.span, self.interner.intern(seg)?);
            let span = Self::span_merge(dot_span, tok.span);
            body = Located::new(span, Expr_::Access(Box::new(body), field));
        }
        let span = Self::span_merge(dot_span, tok.span);
        Ok(Located::new(
            span,
            Expr_::Lambda(vec![param], Box::new(body)),
        ))
    }

    /// Parse a unary minus in atom (prefix) position, the `-` already consumed
    /// at `minus_span`.
    ///
    /// **Faithful port of the Haskell `exprAtom_` `Negate` arm**
    /// (`Ipe.Parse.Expression`, lines 356–367 of the upstream reference):
    ///
    /// ```haskell
    /// do char mkError '-'
    ///    mc <- peek
    ///    case mc of
    ///        Just c | c >= '0' && c <= '9' -> do
    ///            e <- addLocation (exprAtom_ mkError)
    ///            return (Src.Negate e)
    ///        _ -> do
    ///            e <- addLocation (exprAtom_ mkError)
    ///            return (Src.Negate e)
    /// ```
    ///
    /// Both branches of the digit/non-digit dispatch produce `Src.Negate(e)` —
    /// the digit check is vestigial; the disambiguation between unary negation
    /// and binary subtraction is positional: `parse_negative_literal` is only
    /// reached when an atom (fresh operand) was expected, never after a complete
    /// expression (`peek_binop` in [`Self::parse_expr`] would have consumed the
    /// `-` as binary subtraction first).
    ///
    /// ## Operand forms
    ///
    /// * **Adjacent numeric literal** (`-5`, `-2.7`) — the sign is folded
    ///   directly into a signed [`Expr_::Int`] / [`Expr_::Float`] node.  No
    ///   downstream `Negate` AST arm is required, and the value is identical to
    ///   `negate 5`.
    ///
    /// * **Adjacent non-literal atom** (`-x`, `-(e)`, `-f x`) — desugared at
    ///   parse time to `Call(VarLocal("negate"), [e])`, matching the canonical
    ///   Elm / Ipê desugar path.  This closes the IPE-P0001 that 37-composite-
    ///   live-shop hit on `if cents < 0 then -cents else cents` (State.ipe:156).
    ///
    /// * **Non-adjacent** (`- 5`, `- x`) — the Haskell parser's `exprAtom_`
    ///   has no leading `spaces` call after consuming `-`, so a space before the
    ///   operand causes the nested atom parse to fail on the space character
    ///   (consumed error, no backtrack).  We mirror this by checking byte-span
    ///   adjacency and erroring when the gap is non-zero.
    fn parse_negative_literal(
        &mut self,
        minus_span: Span,
        threshold: u32,
        depth: u32,
    ) -> DResult<Expr> {
        // ── Attempt 1: adjacent numeric literal ──────────────────────────────
        // Snapshot the Copy payload of the following token, then drop the borrow
        // before consuming it.
        enum NegLit {
            Int(i64, Span),
            Float(f64, Span),
        }
        let lit = self.peek().and_then(|t| {
            if t.span.lo != minus_span.hi {
                return None; // whitespace between `-` and the token
            }
            match &t.kind {
                Tok::Int(n) => Some(NegLit::Int(*n, t.span)),
                Tok::Float(f) => Some(NegLit::Float(*f, t.span)),
                _ => None,
            }
        });
        match lit {
            Some(NegLit::Int(n, lit_span)) => {
                self.bump(Construct::Expression)?;
                // A positive `Int` token is bounded by `i64::MAX` at lex time, so
                // `checked_neg` never overflows here; the fail-closed branch keeps
                // the parser panic-free regardless.
                let value = n.checked_neg().ok_or_else(|| Diagnostic::Parse {
                    span: Self::span_merge(minus_span, lit_span),
                    msg: ParseError::UnexpectedToken {
                        found: tok_kind(&Tok::Int(n)),
                        expected: ExpectedSet([Expected::Expression].into()),
                    },
                })?;
                return Ok(Located::new(
                    Self::span_merge(minus_span, lit_span),
                    Expr_::Int(value),
                ));
            }
            Some(NegLit::Float(f, lit_span)) => {
                self.bump(Construct::Expression)?;
                return Ok(Located::new(
                    Self::span_merge(minus_span, lit_span),
                    Expr_::Float(-f),
                ));
            }
            None => {}
        }

        // ── Attempt 2: adjacent non-literal atom → `negate(e)` ──────────────
        // Port of the Haskell `_` branch: parse the sub-atom immediately
        // following `-` and desugar to `Call(VarLocal("negate"), [e])`.
        // Adjacency check mirrors the Haskell `exprAtom_` having no leading
        // `spaces` call after the `-`: a space before the operand would cause
        // the recursive atom parse to fail on the space character (consumed
        // error). The check uses byte-span adjacency: `t.span.lo == minus_span.hi`.
        let is_adjacent = self.peek().is_some_and(|t| t.span.lo == minus_span.hi);
        if is_adjacent {
            let negate_sym = self.intern("negate")?;
            let negate_expr = Located::new(minus_span, Expr_::VarLocal(negate_sym));
            let sub_expr = self.parse_atom_postfix(threshold, depth + 1)?;
            let call_span = Self::span_merge(minus_span, sub_expr.span);
            return Ok(Located::new(
                call_span,
                Expr_::Call(Box::new(negate_expr), vec![sub_expr]),
            ));
        }

        // ── No match: non-adjacent `-` in atom position ──────────────────────
        Err(Diagnostic::Parse {
            span: minus_span,
            msg: ParseError::UnexpectedToken {
                found: tok_kind(&Tok::Minus),
                expected: ExpectedSet([Expected::Expression].into()),
            },
        })
    }

    /// Parse what follows a just-consumed `(`: empty parens `()` are the unit
    /// value ([`Expr_::Unit`]); a single expression has its parens unwrapped
    /// (`(e)` is just `e`, preserving the outer span so explicit grouping is
    /// honoured); a comma-separated list is a tuple literal `(e1, e2, ...)` of
    /// arity ≥ 2.
    ///
    /// `opener` is the `(`'s span; `depth` bounds the inner-expression recursion.
    fn parse_paren_or_tuple(&mut self, opener: Span, depth: u32) -> DResult<Expr> {
        if depth > MAX_DEPTH {
            return Err(self.too_deep(Construct::Tuple));
        }
        // `()` — the unit value. Empty parentheses are the sole inhabitant of
        // the unit type; the closing `)` is consumed and the span spans both.
        if let Some(t) = self.peek().filter(|t| t.kind == Tok::RParen) {
            let close = t.span;
            self.bump(Construct::ParenGroup)?;
            let span = Self::span_merge(opener, close);
            return Ok(Located::new(span, Expr_::Unit));
        }
        let first = self.parse_expr(0, depth + 1)?;
        // No following comma → a plain parenthesised group. Unwrap to the inner
        // value, spanning from the `(` to the `)` so the grouping is honoured.
        if self.peek_kind() != Some(&Tok::Comma) {
            let close = self.expect_rparen(opener, Construct::ParenGroup)?;
            let span = Self::span_merge(opener, close);
            return Ok(Located::new(span, first.value));
        }
        // One or more comma-separated elements → a tuple literal.
        let mut elems = vec![first];
        while self.peek_kind() == Some(&Tok::Comma) {
            self.bump(Construct::Tuple)?;
            elems.push(self.parse_expr(0, depth + 1)?);
        }
        let close = self.expect_rparen(opener, Construct::Tuple)?;
        let span = Self::span_merge(opener, close);
        Ok(Located::new(span, Expr_::Tuple(elems)))
    }

    /// Parse what follows a just-consumed `[`: a list literal `[]` / `[a, b, c]`.
    /// Elements are comma-separated full expressions; a trailing comma is not
    /// permitted. `opener` is the `[`'s span; `depth` bounds inner recursion.
    fn parse_list(&mut self, opener: Span, depth: u32) -> DResult<Expr> {
        if depth > MAX_DEPTH {
            return Err(self.too_deep(Construct::Expression));
        }
        // `[]` — the empty list.
        if let Some(t) = self.peek().filter(|t| t.kind == Tok::RBracket) {
            let close = t.span;
            self.bump(Construct::Expression)?;
            let span = Self::span_merge(opener, close);
            return Ok(Located::new(span, Expr_::List(Vec::new())));
        }
        let mut elems = vec![self.parse_expr(0, depth + 1)?];
        while self.peek_kind() == Some(&Tok::Comma) {
            self.bump(Construct::Expression)?;
            elems.push(self.parse_expr(0, depth + 1)?);
        }
        let close = self.expect_rbracket(opener)?;
        let span = Self::span_merge(opener, close);
        Ok(Located::new(span, Expr_::List(elems)))
    }

    /// Require a closing `]`, returning its span.
    fn expect_rbracket(&mut self, opener: Span) -> DResult<Span> {
        match self.peek() {
            Some(t) if t.kind == Tok::RBracket => {
                let span = t.span;
                self.bump(Construct::Expression)?;
                Ok(span)
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

    /// Require a closing `)`, returning its span. Like [`Self::close_paren`] but
    /// hands back the `)`'s span so the caller can build the full bracketed span.
    fn expect_rparen(&mut self, opener: Span, construct: Construct) -> DResult<Span> {
        match self.peek() {
            Some(t) if t.kind == Tok::RParen => {
                let span = t.span;
                self.bump(construct)?;
                Ok(span)
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

    /// Resolve a (possibly dotted) identifier token into an expression.
    ///
    /// The first segment's case decides the shape, mirroring Elm:
    /// * an **upper**-case head (`String.fromInt`, `Json.Decode.field`) is a
    ///   module-qualified name — everything but the last segment is the
    ///   qualifier, the last is the value (`VarQual`);
    /// * a **lower**-case head (`p`, `p.x`, `record.a.b`) is a local reference
    ///   followed by zero or more record-field accesses — `p.x.y` becomes
    ///   `Access (Access p x) y`. A bare `p` (no dots) is just `VarLocal p`.
    ///
    /// `span` is the whole identifier token's span; every node built here reuses
    /// it (the lexer produces one token for the dotted run, so there is no
    /// finer-grained span to attribute the pieces to).
    fn ident_expr(&mut self, text: &str, span: Span) -> DResult<Expr_> {
        let mut segs = text.split('.');
        let first = segs.next().unwrap_or("");
        let rest: Vec<&str> = segs.collect();
        if rest.is_empty() {
            return Ok(Expr_::VarLocal(self.interner.intern(first)?));
        }
        let head_upper = first.chars().next().is_some_and(|c| c.is_ascii_uppercase());
        if head_upper {
            // Qualified: everything but the last segment is the qualifier.
            // Slice at the last '.' instead of re-assembling the init
            // segments through Vec + join (efficiency-audit §5 low) —
            // `split('.')` → join of the init segments ≡ `text[..last_dot]`,
            // and rfind at an ASCII '.' is always a char boundary (safe
            // slice). `rest` is non-empty here, so the rfind always hits.
            let Some(idx) = text.rfind('.') else {
                return Ok(Expr_::VarLocal(self.interner.intern(text)?));
            };
            let qualifier = text.get(..idx).unwrap_or_default();
            let last = text.get(idx + 1..).unwrap_or_default();
            let q = self.interner.intern(qualifier)?;
            let name = self.interner.intern(last)?;
            return Ok(Expr_::VarQual(q, name));
        }
        // Lower-case head: a local var with a chain of field accesses.
        let mut expr = Located::new(span, Expr_::VarLocal(self.interner.intern(first)?));
        for (seg_count, seg) in (0_u32..).zip(rest) {
            if seg_count > MAX_DEPTH {
                return Err(self.too_deep(Construct::Expression));
            }
            let field = Located::new(span, self.interner.intern(seg)?);
            expr = Located::new(span, Expr_::Access(Box::new(expr), field));
        }
        Ok(expr.value)
    }

    /// Parse a record literal `{ field = expr, ... }`, the `{` already consumed.
    ///
    /// After the opening `{` three forms are accepted:
    ///
    /// * `{}` — the **empty record literal**: zero fields. Mirrors the Haskell
    ///   compiler's `Src.Record []` (line 309-311 of Expression.hs).
    /// * `{ name = value, ... }` — a record **literal**: a non-empty, comma-
    ///   separated list of `name = value` fields.
    /// * `{ base | field = value, ... }` — a record **update** (a `|` after the
    ///   first name): a copy of the record variable `base` with the listed
    ///   fields replaced. The base is a bare lowercase variable, matching Ipê's
    ///   (and Elm's) grammar.
    ///
    /// `opener` is the `{`'s span; `depth` bounds the field-value recursion.
    fn parse_record(&mut self, opener: Span, depth: u32) -> DResult<Expr> {
        if depth > MAX_DEPTH {
            return Err(self.too_deep(Construct::Record));
        }
        // `{}` (the empty record literal) — valid, produces `Record []`.
        // Mirrors the Haskell reference: `Just '}' -> char '}' >> return (Record [])`.
        if let Some(t) = self.peek().filter(|t| t.kind == Tok::RBrace) {
            let close = t.span;
            self.bump(Construct::Record)?;
            let span = Self::span_merge(opener, close);
            return Ok(Located::new(span, Expr_::Record(Vec::new())));
        }
        // The first lowercase identifier is either the first field's name (a
        // literal) or the base record variable (an update); a following `|`
        // decides.
        let first = self.parse_record_field_name()?;
        if self.peek_kind() == Some(&Tok::Pipe) {
            self.bump(Construct::Record)?;
            return self.parse_record_update(opener, first, depth);
        }
        // Literal: `first` is the first field's name; `=` then its value.
        self.expect_field_equals()?;
        let value = self.parse_expr(0, depth + 1)?;
        let mut fields = vec![(first, value)];
        loop {
            // A `,` continues the field list; a `}` closes it.
            match self.peek() {
                Some(t) if t.kind == Tok::Comma => {
                    self.bump(Construct::Record)?;
                }
                Some(t) if t.kind == Tok::RBrace => {
                    let close = t.span;
                    self.bump(Construct::Record)?;
                    let span = Self::span_merge(opener, close);
                    return Ok(Located::new(span, Expr_::Record(fields)));
                }
                Some(t) => {
                    return Err(Self::unexpected_token(
                        t,
                        &[Expected::Comma, Expected::RBrace],
                    ));
                }
                None => return Err(self.record_eof()),
            }
            let name = self.parse_record_field_name()?;
            self.expect_field_equals()?;
            let value = self.parse_expr(0, depth + 1)?;
            fields.push((name, value));
        }
    }

    /// Parse the field list of a record update `{ base | field = value, ... }`,
    /// the `base |` prefix already consumed. At least one updated field is
    /// required; the list is comma-separated and closed by `}`. `opener` is the
    /// `{`'s span; `base` is the located base variable name.
    fn parse_record_update(
        &mut self,
        opener: Span,
        base: Located<Symbol>,
        depth: u32,
    ) -> DResult<Expr> {
        let mut fields = Vec::new();
        loop {
            let name = self.parse_record_field_name()?;
            self.expect_field_equals()?;
            let value = self.parse_expr(0, depth + 1)?;
            fields.push((name, value));
            match self.peek() {
                Some(t) if t.kind == Tok::Comma => {
                    self.bump(Construct::Record)?;
                }
                Some(t) if t.kind == Tok::RBrace => {
                    let close = t.span;
                    self.bump(Construct::Record)?;
                    let span = Self::span_merge(opener, close);
                    return Ok(Located::new(span, Expr_::Update(base, fields)));
                }
                Some(t) => {
                    return Err(Self::unexpected_token(
                        t,
                        &[Expected::Comma, Expected::RBrace],
                    ));
                }
                None => return Err(self.record_eof()),
            }
        }
    }

    /// Consume the `=` that separates a record field name from its value.
    fn expect_field_equals(&mut self) -> DResult<()> {
        match self.peek() {
            Some(t) if t.kind == Tok::Equals => {
                self.bump(Construct::Record)?;
                Ok(())
            }
            Some(t) => Err(Self::unexpected_token(t, &[Expected::Equals])),
            None => Err(self.record_eof()),
        }
    }

    /// The unexpected-EOF diagnostic for an unterminated record (literal or
    /// update).
    fn record_eof(&self) -> Diagnostic {
        Diagnostic::Parse {
            span: self.eof_err_span(),
            msg: ParseError::UnexpectedEof {
                construct: Construct::Record,
            },
        }
    }

    /// Parse a record field name: a plain lowercase identifier (no dots, no
    /// uppercase head). A qualified or upper-case token here is rejected rather
    /// than silently accepted as a label.
    fn parse_record_field_name(&mut self) -> DResult<Located<Symbol>> {
        let tok = self.bump(Construct::Record)?;
        let Tok::Ident(text) = &tok.kind else {
            return Err(Self::unexpected_token(&tok, &[Expected::Identifier]));
        };
        if text.contains('.') || text.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
            return Err(Self::unexpected_token(&tok, &[Expected::Identifier]));
        }
        let sym = self.interner.intern(text)?;
        Ok(Located::new(tok.span, sym))
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
                layout::aligned_at(t, binding_col)
                    && matches!(
                        t.kind,
                        Tok::Ident(_) | Tok::LParen | Tok::LBrace | Tok::Underscore
                    )
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

    /// Parse a lambda `\p0 p1 ... -> body`, the `\` already peeked. One or more
    /// parameter patterns precede the `->`; the body parses as a full expression
    /// at `threshold`, so it extends as far right as the surrounding layout
    /// allows (`\x -> x + 1` captures the whole `x + 1`). A zero-parameter
    /// `\ -> e` and a missing `->` are clean parse errors, never a silently
    /// reshaped AST. Mirrors the Haskell compiler's `Ipe.Parse.Expression.lambda`.
    fn parse_lambda(&mut self, threshold: u32, depth: u32) -> DResult<Expr> {
        if depth > MAX_DEPTH {
            return Err(self.too_deep(Construct::Lambda));
        }
        // The caller has already peeked `\`.
        let lambda_tok = self.bump(Construct::Lambda)?;

        // At least one parameter pattern before the `->`.
        let mut params = Vec::new();
        while self.peek_is_binder_atom_start() {
            params.push(self.parse_pattern_atom(depth + 1)?);
        }
        if params.is_empty() {
            return Err(self.peek().map_or_else(
                || Diagnostic::Parse {
                    span: self.eof_err_span(),
                    msg: ParseError::UnexpectedEof {
                        construct: Construct::Lambda,
                    },
                },
                |t| Self::unexpected_token(t, &[Expected::Pattern]),
            ));
        }

        // `->` between the parameters and the body.
        match self.peek() {
            Some(t) if t.kind == Tok::Arrow => {
                self.bump(Construct::Lambda)?;
            }
            Some(t) => return Err(Self::unexpected_token(t, &[Expected::Arrow])),
            None => {
                return Err(Diagnostic::Parse {
                    span: self.eof_err_span(),
                    msg: ParseError::UnexpectedEof {
                        construct: Construct::Lambda,
                    },
                });
            }
        }

        let body = self.parse_expr(threshold, depth + 1)?;
        let span = Self::span_merge(lambda_tok.span, body.span);
        Ok(Located::new(span, Expr_::Lambda(params, Box::new(body))))
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
        // The binder is either a destructure pattern — a tuple `(a, b)`, a record
        // `{ x }`, or a wildcard `_` — or the common `name = body` value
        // binding. The destructure forms start with a token that cannot begin a
        // plain value name, so peeking selects the path without lookahead beyond
        // one token. The simple path keeps its precise BindingNameNotLower
        // diagnostics for an uppercase / dotted name.
        let pat = if matches!(
            self.peek_kind(),
            Some(&(Tok::LParen | Tok::LBrace | Tok::Underscore))
        ) {
            self.parse_pattern(depth + 1)?
        } else {
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
            Located::new(name_tok.span, Pattern_::PVar(name_sym))
        };

        // Function-parameter sugar: `let f x y = body` desugars to
        // `let f = \x y -> body`. Parameters are collected only when the binder
        // is a simple `PVar` name (destructure binders like `{ x }` already
        // consumed their `=` in `parse_pattern`).
        //
        // We collect binder atoms as long as the next token can begin one AND
        // it is not `=`. A literal start (Int, Str, …) is not a binder atom, so
        // it falls through to the `MissingEquals` error below — exactly the
        // right behaviour for malformed input like `let x 2 in x`.
        let mut params: Vec<Pattern> = Vec::new();
        if matches!(pat.value, Pattern_::PVar(_)) {
            while self.peek_is_binder_atom_start()
                && !matches!(self.peek_kind(), Some(&Tok::Equals))
            {
                params.push(self.parse_pattern_atom(depth + 1)?);
            }
        }

        // `=` after the binder (and any collected parameters).
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

        let rhs = self.parse_expr(binding_col, depth + 1)?;
        // If we collected parameters, wrap the body in a Lambda so the let
        // binding holds the desugared function value. The lambda span runs from
        // the first parameter to the end of the body.
        let body = if params.is_empty() {
            rhs
        } else {
            let first_param_span = params.first().map_or(rhs.span, |p| p.span);
            let span = Self::span_merge(first_param_span, rhs.span);
            Located::new(span, Expr_::Lambda(params, Box::new(rhs)))
        };
        Ok(LetBinding { pat, body })
    }

    /// Parse a `do` block — an aligned sequence of statements desugared to a
    /// `Task.andThen` chain. Statement forms: `p <- e` (run `e`, bind its result
    /// to `p`), `p = e` (a pure `let`), and bare `e` (run for effect). The last
    /// statement is the block's result expression.
    ///
    /// Desugaring, bottom-up over the tail `rest`:
    /// - `⟦(p <- e); rest⟧ = Task.andThen (\p -> ⟦rest⟧) e`
    /// - `⟦(p =  e); rest⟧ = let p = e in ⟦rest⟧`
    /// - `⟦e ; rest⟧       = Task.andThen (\_ -> ⟦rest⟧) e`
    /// - `⟦e⟧              = e`
    ///
    /// Because the desugar names `Task.andThen`, the enclosing module must have
    /// `Ipe.Task` in scope as `Task` (the usual `import Ipe.Task as Task`).
    fn parse_do(&mut self, _threshold: u32, depth: u32) -> DResult<Expr> {
        if depth > MAX_DEPTH {
            return Err(self.too_deep(Construct::Expression));
        }
        let do_tok = self.bump(Construct::Expression)?;
        let Some(first) = self.peek() else {
            return Err(Diagnostic::Parse {
                span: self.eof_err_span(),
                msg: ParseError::UnexpectedEof {
                    construct: Construct::Expression,
                },
            });
        };
        let block_col = first.col;
        let mut stmts = Vec::new();
        loop {
            stmts.push(self.parse_do_statement(block_col, depth + 1)?);
            if !self
                .peek()
                .is_some_and(|t| layout::aligned_at(t, block_col))
            {
                break;
            }
        }
        self.desugar_do(do_tok.span, stmts)
    }

    /// Parse one `do`-block statement. A leading lowercase name (or `_`) directly
    /// followed by `<-` or `=` is a bind / let; anything else is a bare run.
    fn parse_do_statement(&mut self, block_col: u32, depth: u32) -> DResult<DoStmt> {
        let head_is_binder = matches!(self.peek_kind(), Some(Tok::Ident(_) | Tok::Underscore));
        let next = self.toks.get(self.pos + 1).map(|t| &t.kind);
        let is_bind = next == Some(&Tok::LeftArrow);
        let is_let = next == Some(&Tok::Equals);
        if head_is_binder && (is_bind || is_let) {
            let head = self.bump(Construct::Expression)?;
            let pat = match &head.kind {
                Tok::Underscore => Located::new(head.span, Pattern_::PAnything),
                Tok::Ident(text) => {
                    if text.contains('.')
                        || text.chars().next().is_some_and(|c| c.is_ascii_uppercase())
                    {
                        return Err(Self::malformed_let(
                            head.span,
                            LetDefect::BindingNameNotLower,
                        ));
                    }
                    let sym = self.interner.intern(text)?;
                    Located::new(head.span, Pattern_::PVar(sym))
                }
                _ => {
                    return Err(Diagnostic::Parse {
                        span: head.span,
                        msg: ParseError::Unexpected,
                    });
                }
            };
            let op = self.bump(Construct::Expression)?; // consume `<-` or `=`
            let rhs = self.parse_expr(block_col, depth + 1)?;
            Ok(if is_bind {
                DoStmt::Bind(pat, op.span, rhs)
            } else {
                DoStmt::Let(pat, op.span, rhs)
            })
        } else {
            let e = self.parse_expr(block_col, depth + 1)?;
            Ok(DoStmt::Run(e))
        }
    }

    /// Fold the parsed statements into the `Task.andThen` chain (see
    /// [`Self::parse_do`]). The block must end in a result expression.
    fn desugar_do(&mut self, do_span: Span, stmts: Vec<DoStmt>) -> DResult<Expr> {
        let mut rev = stmts.into_iter().rev();
        let Some(last) = rev.next() else {
            return Err(Diagnostic::Parse {
                span: do_span,
                msg: ParseError::Unexpected,
            });
        };
        let mut acc = match last {
            DoStmt::Run(e) => e,
            DoStmt::Bind(pat, ..) | DoStmt::Let(pat, ..) => {
                return Err(Diagnostic::Parse {
                    span: pat.span,
                    msg: ParseError::Unexpected,
                });
            }
        };
        for stmt in rev {
            acc = match stmt {
                DoStmt::Bind(pat, op_span, task) => {
                    // Lambda gets a zero-width span at the operator's start; the
                    // call/callee get the full operator span. Distinct, so the
                    // post-order parent `Call` cannot overwrite the lambda arrow.
                    let lam_span = Span::new(op_span.lo, op_span.lo);
                    self.task_and_then(pat, lam_span, op_span, acc, task)?
                }
                DoStmt::Run(task) => {
                    // A bare run has no operator token: the lambda takes a
                    // zero-width span at the task's end, the call at its start —
                    // distinct, and sharing no operand.
                    let lam_span = Span::new(task.span.hi, task.span.hi);
                    let call_span = Span::new(task.span.lo, task.span.lo);
                    let wild = Located::new(lam_span, Pattern_::PAnything);
                    self.task_and_then(wild, lam_span, call_span, acc, task)?
                }
                DoStmt::Let(pat, op_span, body) => Located::new(
                    op_span,
                    Expr_::Let(vec![LetBinding { pat, body }], Box::new(acc)),
                ),
            };
        }
        // Re-stamp the whole block with the `do` keyword's (unique) span.
        Ok(Located::new(do_span, acc.value))
    }

    /// Build `Task.andThen (\pat -> cont) task`. The lambda carries `lam_span`
    /// and the call/callee carry `call_span` — distinct, and neither an
    /// operand's — so the type checker's post-order `(home, span)` region map
    /// records the lambda's arrow without the parent call overwriting it.
    fn task_and_then(
        &mut self,
        pat: Pattern,
        lam_span: Span,
        call_span: Span,
        cont: Expr,
        task: Expr,
    ) -> DResult<Expr> {
        let task_mod = self.interner.intern("Task")?;
        let and_then = self.interner.intern("andThen")?;
        let callee = Located::new(call_span, Expr_::VarQual(task_mod, and_then));
        let lam = Located::new(lam_span, Expr_::Lambda(vec![pat], Box::new(cont)));
        Ok(Located::new(
            call_span,
            Expr_::Call(Box::new(callee), vec![lam, task]),
        ))
    }

    /// Parse a `parallelDo` block — aligned same-typed task expressions run
    /// concurrently, their results collected in order. Desugars to
    /// `Task.parallel [e1, ..., eN] : Task Error (List a)`. Nest it in a `do`
    /// (`results <- parallelDo …`) to consume the collected list. `Ipe.Task`
    /// must be in scope as `Task`.
    fn parse_parallel_do(&mut self, _threshold: u32, depth: u32) -> DResult<Expr> {
        if depth > MAX_DEPTH {
            return Err(self.too_deep(Construct::Expression));
        }
        let pd_tok = self.bump(Construct::Expression)?;
        let Some(first) = self.peek() else {
            return Err(Diagnostic::Parse {
                span: self.eof_err_span(),
                msg: ParseError::UnexpectedEof {
                    construct: Construct::Expression,
                },
            });
        };
        let block_col = first.col;
        let mut elems = Vec::new();
        loop {
            elems.push(self.parse_expr(block_col, depth + 1)?);
            if !self
                .peek()
                .is_some_and(|t| layout::aligned_at(t, block_col))
            {
                break;
            }
        }
        // The synthetic list argument takes a zero-width span at the keyword's
        // start; the `Task.parallel` reference and the call take the full keyword
        // span. Distinct (and never an element's), so the post-order parent
        // `Call` cannot overwrite the list's `List` region type.
        let list_span = Span::new(pd_tok.span.lo, pd_tok.span.lo);
        let list = Located::new(list_span, Expr_::List(elems));
        let task_mod = self.interner.intern("Task")?;
        let parallel = self.interner.intern("parallel")?;
        let callee = Located::new(pd_tok.span, Expr_::VarQual(task_mod, parallel));
        Ok(Located::new(
            pd_tok.span,
            Expr_::Call(Box::new(callee), vec![list]),
        ))
    }

    // ---- patterns ---------------------------------------------------------

    /// A full pattern, gathering constructor sub-patterns (case-arm position).
    fn parse_pattern(&mut self, depth: u32) -> DResult<Pattern> {
        if depth > MAX_DEPTH {
            return Err(self.too_deep(Construct::Pattern));
        }
        let head = self.parse_pattern_atom(depth + 1)?;
        // Only a constructor head may take sub-patterns.
        let pat = if let Pattern_::PCtor(name, mods, _) = head.value.clone() {
            let mut sub = Vec::new();
            let mut end = head.span;
            while self.peek_is_pattern_atom_start() {
                let p = self.parse_pattern_atom(depth + 1)?;
                end = p.span;
                sub.push(p);
            }
            if sub.is_empty() {
                head
            } else {
                let span = Self::span_merge(head.span, end);
                Located::new(span, Pattern_::PCtor(name, mods, sub))
            }
        } else {
            head
        };
        // A cons pattern `head :: tail`. `::` is right-associative and
        // binds looser than constructor application, so the head parsed so far is
        // the first element and the tail is parsed recursively (which nests
        // rightward and consumes its own `as` alias). `a :: b :: rest` becomes
        // `PCons(a, PCons(b, rest))`.
        let pat = if self.peek_kind() == Some(&Tok::ColonColon) {
            self.bump(Construct::Pattern)?;
            let tail = self.parse_pattern(depth + 1)?;
            let span = Self::span_merge(pat.span, tail.span);
            Located::new(span, Pattern_::PCons(Box::new(pat), Box::new(tail)))
        } else {
            pat
        };
        // An `as` alias binds the whole pattern parsed so far to a name
        // (`inner as name`). Mirrors the Haskell compiler's `pattern_` postfix
        // check; the inner sub-pattern keeps its shape and the alias wraps it.
        if self.peek_kind() == Some(&Tok::As) {
            self.bump(Construct::Pattern)?;
            let name = self.parse_lower_name()?;
            let span = Self::span_merge(pat.span, name.span);
            return Ok(Located::new(span, Pattern_::PAlias(Box::new(pat), name)));
        }
        Ok(pat)
    }

    /// Parse a lowercase identifier as a located binding name (the alias target
    /// of an `as` pattern). An upper-case identifier or any other token is a
    /// parse error — only a lowercase name can bind a value.
    fn parse_lower_name(&mut self) -> DResult<Located<Symbol>> {
        let tok = self.bump(Construct::Pattern)?;
        if let Tok::Ident(text) = &tok.kind
            && text.chars().next().is_some_and(|c| c.is_ascii_lowercase())
            && !text.contains('.')
        {
            let sym = self.interner.intern(text)?;
            return Ok(Located::new(tok.span, sym));
        }
        Err(Self::unexpected_token(&tok, &[Expected::Identifier]))
    }

    fn parse_pattern_atom(&mut self, depth: u32) -> DResult<Pattern> {
        if depth > MAX_DEPTH {
            return Err(self.too_deep(Construct::Pattern));
        }
        let tok = self.bump(Construct::Pattern)?;
        match &tok.kind {
            Tok::Underscore => Ok(Located::new(tok.span, Pattern_::PAnything)),
            // Literal leaves: int / string / char. Bool literals
            // (`True` / `False`) come through the `Ident` arm below. A float
            // literal is intentionally NOT a pattern leaf — equality on `f64`
            // is unsound to match on (Rust forbids float patterns), so a
            // `Tok::Float` falls through to the fail-closed catch-all below.
            Tok::Int(n) => Ok(Located::new(tok.span, Pattern_::PInt(*n))),
            Tok::Str(s) | Tok::TripleStr(s) => {
                Ok(Located::new(tok.span, Pattern_::PStr(s.clone())))
            }
            Tok::Char(c) => Ok(Located::new(tok.span, Pattern_::PChar(c.clone()))),
            // A negative integer literal pattern `-3`. The `-` lexes as
            // [`Tok::Minus`]; the digit must follow immediately. Anything else
            // after `-` is not a pattern.
            Tok::Minus => {
                let neg = self.bump(Construct::Pattern)?;
                if let Tok::Int(n) = &neg.kind {
                    let span = Self::span_merge(tok.span, neg.span);
                    Ok(Located::new(span, Pattern_::PInt(n.wrapping_neg())))
                } else {
                    Err(Self::unexpected_token(&neg, &[Expected::Pattern]))
                }
            }
            Tok::LParen => {
                let opener = tok.span;
                let inner = self.parse_pattern(depth + 1)?;
                // A following `,` makes this a tuple pattern `(p0, p1, ...)`;
                // otherwise the parens just group a single pattern and unwrap.
                if self.peek_kind() == Some(&Tok::Comma) {
                    let mut elems = vec![inner];
                    while self.peek_kind() == Some(&Tok::Comma) {
                        self.bump(Construct::Pattern)?;
                        elems.push(self.parse_pattern(depth + 1)?);
                    }
                    let close = self.expect_rparen(opener, Construct::Pattern)?;
                    let span = Self::span_merge(opener, close);
                    return Ok(Located::new(span, Pattern_::PTuple(elems)));
                }
                self.close_paren(opener, Construct::Pattern)?;
                Ok(Located::new(tok.span, inner.value))
            }
            Tok::LBrace => {
                // A record pattern `{ x, y }`. Field-pun only: each entry
                // is a bare lowercase field name that also binds a local of the
                // same name. There is no `{ field = sub-pattern }` form (the Go
                // reference rejects it at parse), and the empty record `{}` is
                // outside the grammar, so a record pattern carries ≥ 1 field.
                let opener = tok.span;
                if let Some(t) = self.peek().filter(|t| t.kind == Tok::RBrace) {
                    return Err(Self::unexpected_token(t, &[Expected::Identifier]));
                }
                let mut fields = vec![self.parse_record_field_name()?];
                loop {
                    match self.peek() {
                        Some(t) if t.kind == Tok::Comma => {
                            self.bump(Construct::Pattern)?;
                        }
                        Some(t) if t.kind == Tok::RBrace => {
                            let close = t.span;
                            self.bump(Construct::Pattern)?;
                            let span = Self::span_merge(opener, close);
                            return Ok(Located::new(span, Pattern_::PRecord(fields)));
                        }
                        Some(t) => {
                            return Err(Self::unexpected_token(
                                t,
                                &[Expected::Comma, Expected::RBrace],
                            ));
                        }
                        None => return Err(self.record_eof()),
                    }
                    fields.push(self.parse_record_field_name()?);
                }
            }
            Tok::LBracket => self.parse_list_pattern(tok.span, depth + 1),
            Tok::Ident(text) => {
                // `True` / `False` are the two Bool constructors; in pattern
                // position they are boolean literal patterns (a closed
                // two-constructor cover), matching the Haskell compiler's
                // `Src.PBool`. They are checked before the general ctor branch.
                if text == "True" {
                    return Ok(Located::new(tok.span, Pattern_::PBool(true)));
                }
                if text == "False" {
                    return Ok(Located::new(tok.span, Pattern_::PBool(false)));
                }
                let first_upper = text.chars().next().is_some_and(|c| c.is_ascii_uppercase());
                if first_upper {
                    // Qualified constructors (`Module.Ctor`) are not modelled in
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

    /// Whether the next token can begin a pattern ATOM in a position that admits
    /// refutable literal sub-patterns — namely a constructor's argument list
    /// (`Just 0`, `MkWrap 'a'`). Includes the literal starts.
    /// Parse a list pattern `[]` / `[a, b, c]` after the `[` is consumed.
    /// Comma-separated sub-patterns; the empty brackets are the nil cover.
    /// `opener` is the `[`'s span.
    fn parse_list_pattern(&mut self, opener: Span, depth: u32) -> DResult<Pattern> {
        if depth > MAX_DEPTH {
            return Err(self.too_deep(Construct::Pattern));
        }
        if let Some(t) = self.peek().filter(|t| t.kind == Tok::RBracket) {
            let close = t.span;
            self.bump(Construct::Pattern)?;
            let span = Self::span_merge(opener, close);
            return Ok(Located::new(span, Pattern_::PList(Vec::new())));
        }
        let mut elems = vec![self.parse_pattern(depth + 1)?];
        while self.peek_kind() == Some(&Tok::Comma) {
            self.bump(Construct::Pattern)?;
            elems.push(self.parse_pattern(depth + 1)?);
        }
        let close = self.expect_rbracket(opener)?;
        let span = Self::span_merge(opener, close);
        Ok(Located::new(span, Pattern_::PList(elems)))
    }

    fn peek_is_pattern_atom_start(&self) -> bool {
        matches!(
            self.peek_kind(),
            Some(
                &(Tok::Underscore
                    | Tok::LParen
                    | Tok::LBrace
                    | Tok::LBracket
                    | Tok::Ident(_)
                    | Tok::Int(_)
                    | Tok::Str(_)
                    | Tok::TripleStr(_)
                    | Tok::Char(_)
                    | Tok::Minus)
            )
        )
    }

    /// Whether the next token can begin a BINDER pattern atom — a function or
    /// lambda parameter. Literals are refutable and never bind a parameter, so a
    /// literal start STOPS the parameter list (`\x 1` reports a missing `->` at
    /// `1`, not a swallowed literal parameter).
    fn peek_is_binder_atom_start(&self) -> bool {
        matches!(
            self.peek_kind(),
            Some(&(Tok::Underscore | Tok::LParen | Tok::LBrace | Tok::Ident(_)))
        )
    }

    // ---- span helper ------------------------------------------------------

    fn span_merge(a: Span, b: Span) -> Span {
        Span::new(a.lo.min(b.lo), a.hi.max(b.hi))
    }
}
