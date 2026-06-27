//! Recursive-descent parser for the Milestone-0 subset of Sky.
//!
//! Port of `Sky.Parse.{Module,Declaration,Type,Pattern,Expression}` narrowed to
//! the M0 grammar: a module header, imports, `type` unions, top-level value
//! bindings with optional type annotations, `case … of`, function application,
//! and `+`/`-` binary-operator chains.
//!
//! Recursion is bounded by [`MAX_DEPTH`]; every recursive entry threads a depth
//! counter and fails with [`ParseError::TooDeep`] before the native stack can
//! overflow on adversarial input.

use sky_diagnostics::{DResult, Diagnostic, Located, ParseError, Span};
use sky_intern::{Interner, Symbol};
use sky_syntax::{
    Ctor, Exposed, Exposing, Expr, Expr_, Import, Module, Pattern, Pattern_, Privacy,
    TypeAnnotation, Union, Value,
};

use crate::layout;
use crate::lexer::{Tok, Token};

/// Maximum recursion depth before the parser bails with [`ParseError::TooDeep`].
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

    fn bump(&mut self) -> DResult<Token> {
        let tok = self
            .toks
            .get(self.pos)
            .cloned()
            .ok_or_else(|| self.eof_err())?;
        self.pos += 1;
        Ok(tok)
    }

    fn eof_err(&self) -> Diagnostic {
        let span = self.toks.last().map_or(Span::DUMMY, |t| t.span);
        Diagnostic::Parse {
            span,
            msg: ParseError::Unexpected,
        }
    }

    fn err_here(&self) -> Diagnostic {
        let span = self.peek().map_or_else(|| self.eof_err_span(), |t| t.span);
        Diagnostic::Parse {
            span,
            msg: ParseError::Unexpected,
        }
    }

    fn eof_err_span(&self) -> Span {
        self.toks.last().map_or(Span::DUMMY, |t| t.span)
    }

    fn too_deep(&self) -> Diagnostic {
        Diagnostic::Parse {
            span: self.err_here_span(),
            msg: ParseError::TooDeep,
        }
    }

    fn err_here_span(&self) -> Span {
        self.peek().map_or_else(|| self.eof_err_span(), |t| t.span)
    }

    /// Consume the next token, requiring it to equal `want`.
    fn expect(&mut self, want: &Tok) -> DResult<Token> {
        let tok = self.bump()?;
        if &tok.kind == want {
            Ok(tok)
        } else {
            Err(Diagnostic::Parse {
                span: tok.span,
                msg: ParseError::Unexpected,
            })
        }
    }

    fn intern(&mut self, s: &str) -> Symbol {
        self.interner.intern(s)
    }

    // ---- module -----------------------------------------------------------

    pub fn parse_module(&mut self) -> DResult<Module> {
        let module_tok = self.expect(&Tok::Module)?;
        let name = self.parse_dotted_name()?;
        self.expect(&Tok::Exposing)?;
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

    fn parse_dotted_name(&mut self) -> DResult<Located<Vec<Symbol>>> {
        let tok = self.bump()?;
        match &tok.kind {
            Tok::Ident(text) => {
                let segs: Vec<Symbol> = text.split('.').map(|s| self.interner.intern(s)).collect();
                Ok(Located::new(tok.span, segs))
            }
            _ => Err(Diagnostic::Parse {
                span: tok.span,
                msg: ParseError::Unexpected,
            }),
        }
    }

    fn parse_exposing(&mut self) -> DResult<Exposing> {
        self.expect(&Tok::LParen)?;
        if self.peek_kind() == Some(&Tok::DotDot) {
            self.bump()?;
            self.expect(&Tok::RParen)?;
            return Ok(Exposing::All);
        }
        let mut items = Vec::new();
        loop {
            items.push(self.parse_exposed()?);
            match self.peek_kind() {
                Some(&Tok::Comma) => {
                    self.bump()?;
                }
                Some(&Tok::RParen) => {
                    self.bump()?;
                    break;
                }
                _ => return Err(self.err_here()),
            }
        }
        Ok(Exposing::List(items))
    }

    fn parse_exposed(&mut self) -> DResult<Located<Exposed>> {
        let tok = self.bump()?;
        let Tok::Ident(text) = &tok.kind else {
            return Err(Diagnostic::Parse {
                span: tok.span,
                msg: ParseError::Unexpected,
            });
        };
        let sym = self.interner.intern(text);
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
        self.bump()?; // (
        if self.peek_kind() == Some(&Tok::DotDot) {
            self.bump()?;
            self.expect(&Tok::RParen)?;
            return Ok(Privacy::Public);
        }
        let mut ctors = Vec::new();
        loop {
            let tok = self.bump()?;
            match &tok.kind {
                Tok::Ident(text) => ctors.push(self.interner.intern(text)),
                _ => {
                    return Err(Diagnostic::Parse {
                        span: tok.span,
                        msg: ParseError::Unexpected,
                    });
                }
            }
            match self.peek_kind() {
                Some(&Tok::Comma) => {
                    self.bump()?;
                }
                Some(&Tok::RParen) => {
                    self.bump()?;
                    break;
                }
                _ => return Err(self.err_here()),
            }
        }
        Ok(Privacy::PublicCtors(ctors))
    }

    fn parse_import(&mut self) -> DResult<Import> {
        self.expect(&Tok::Import)?;
        let name = self.parse_dotted_name()?;
        let alias = if self.peek_kind() == Some(&Tok::As) {
            self.bump()?;
            let tok = self.bump()?;
            match &tok.kind {
                Tok::Ident(text) => Some(self.interner.intern(text)),
                _ => {
                    return Err(Diagnostic::Parse {
                        span: tok.span,
                        msg: ParseError::Unexpected,
                    });
                }
            }
        } else {
            None
        };
        let exposing = if self.peek_kind() == Some(&Tok::Exposing) {
            self.bump()?;
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
        let tok = self.bump()?;
        let Tok::Ident(text) = &tok.kind else {
            return Err(Diagnostic::Parse {
                span: tok.span,
                msg: ParseError::Unexpected,
            });
        };
        let name_sym = self.interner.intern(text);
        let name = Located::new(tok.span, name_sym);
        let name_col = tok.col;

        if self.peek_kind() == Some(&Tok::Colon) {
            self.bump()?; // :
            let ty = self.parse_type(name_col, 0)?;
            return Ok(Decl::Annotation(name_sym, ty));
        }

        let mut patterns = Vec::new();
        while self.peek_is_pattern_atom_start() {
            patterns.push(self.parse_pattern_atom(0)?);
        }
        self.expect(&Tok::Equals)?;
        let body = self.parse_expr(name_col, 0)?;
        Ok(Decl::Value {
            name,
            patterns,
            body,
        })
    }

    fn parse_union(&mut self) -> DResult<Located<Union>> {
        let type_tok = self.expect(&Tok::Type)?;
        let union_col = type_tok.col;
        let name_tok = self.bump()?;
        let Tok::Ident(name_text) = &name_tok.kind else {
            return Err(Diagnostic::Parse {
                span: name_tok.span,
                msg: ParseError::Unexpected,
            });
        };
        let name = Located::new(name_tok.span, self.interner.intern(name_text));

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
            let sym = self.intern(&text);
            vars.push(Located::new(span, sym));
            self.bump()?;
        }

        self.expect(&Tok::Equals)?;

        let mut ctors = Vec::new();
        loop {
            ctors.push(self.parse_ctor(union_col)?);
            if self.peek_kind() == Some(&Tok::Pipe) {
                self.bump()?;
            } else {
                break;
            }
        }
        let last_span = ctors.last().map_or(name.span, |c| c.span);
        let span = Self::span_merge(type_tok.span, last_span);
        Ok(Located::new(span, Union { name, vars, ctors }))
    }

    fn parse_ctor(&mut self, threshold: u32) -> DResult<Located<Ctor>> {
        let tok = self.bump()?;
        let Tok::Ident(text) = &tok.kind else {
            return Err(Diagnostic::Parse {
                span: tok.span,
                msg: ParseError::Unexpected,
            });
        };
        let name = self.interner.intern(text);
        let mut args = Vec::new();
        while self.peek_is_type_atom_in_block(threshold) {
            args.push(self.parse_type_atom(0)?.value);
        }
        Ok(Located::new(tok.span, Ctor { name, args }))
    }

    // ---- types ------------------------------------------------------------

    fn parse_type(&mut self, threshold: u32, depth: u32) -> DResult<Located<TypeAnnotation>> {
        if depth > MAX_DEPTH {
            return Err(self.too_deep());
        }
        let left = self.parse_type_app(threshold, depth + 1)?;
        let arrow_in_block = self
            .peek()
            .is_some_and(|t| t.kind == Tok::Arrow && layout::continues_block(t, threshold));
        if arrow_in_block {
            self.bump()?;
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
            return Err(self.too_deep());
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
                msg: ParseError::Unexpected,
            }),
        }
    }

    fn parse_type_atom(&mut self, depth: u32) -> DResult<Located<TypeAnnotation>> {
        if depth > MAX_DEPTH {
            return Err(self.too_deep());
        }
        let tok = self.bump()?;
        match &tok.kind {
            Tok::LParen => {
                let inner = self.parse_type(0, depth + 1)?;
                self.expect(&Tok::RParen)?;
                Ok(Located::new(tok.span, inner.value))
            }
            Tok::Ident(text) => {
                let first_upper = text.chars().next().is_some_and(|c| c.is_ascii_uppercase());
                if first_upper {
                    let empty = self.interner.intern("");
                    let segs: Vec<Symbol> =
                        text.split('.').map(|s| self.interner.intern(s)).collect();
                    Ok(Located::new(
                        tok.span,
                        TypeAnnotation::TType(empty, segs, Vec::new()),
                    ))
                } else {
                    let sym = self.interner.intern(text);
                    Ok(Located::new(tok.span, TypeAnnotation::TVar(sym)))
                }
            }
            _ => Err(Diagnostic::Parse {
                span: tok.span,
                msg: ParseError::Unexpected,
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
            return Err(self.too_deep());
        }
        let first = self.parse_app(threshold, depth + 1)?;
        let mut ops: Vec<(Expr, Located<Symbol>)> = Vec::new();
        let mut operand = first;
        while let Some((op, op_span)) = self.peek_binop(threshold) {
            let op_sym = self.intern(op);
            self.bump()?;
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
            return Err(self.too_deep());
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

    /// Peek a `+`/`-` binary operator that continues the current block.
    fn peek_binop(&self, threshold: u32) -> Option<(&'static str, Span)> {
        let tok = self.peek()?;
        if !layout::continues_block(tok, threshold) {
            return None;
        }
        match tok.kind {
            Tok::Plus => Some(("+", tok.span)),
            Tok::Minus => Some(("-", tok.span)),
            _ => None,
        }
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
            return Err(self.too_deep());
        }
        if self.peek_kind() == Some(&Tok::Case) {
            return self.parse_case(threshold, depth + 1);
        }
        let tok = self.bump()?;
        match &tok.kind {
            Tok::LParen => {
                let inner = self.parse_expr(0, depth + 1)?;
                self.expect(&Tok::RParen)?;
                Ok(Located::new(tok.span, inner.value))
            }
            Tok::Int(n) => Ok(Located::new(tok.span, Expr_::Int(*n))),
            Tok::Ident(text) => Ok(Located::new(tok.span, self.ident_expr(text))),
            _ => Err(Diagnostic::Parse {
                span: tok.span,
                msg: ParseError::Unexpected,
            }),
        }
    }

    /// Resolve a (possibly dotted) identifier into a `VarLocal` / `VarQual`.
    fn ident_expr(&mut self, text: &str) -> Expr_ {
        let mut segs = text.split('.');
        let first = segs.next().unwrap_or("");
        let rest: Vec<&str> = segs.collect();
        if rest.is_empty() {
            return Expr_::VarLocal(self.interner.intern(first));
        }
        // Qualified: everything but the last segment is the qualifier.
        let mut all: Vec<&str> = Vec::with_capacity(rest.len() + 1);
        all.push(first);
        all.extend(rest);
        let Some((last, init)) = all.split_last() else {
            return Expr_::VarLocal(self.interner.intern(text));
        };
        let qualifier = init.join(".");
        let q = self.interner.intern(&qualifier);
        let name = self.interner.intern(last);
        Expr_::VarQual(q, name)
    }

    fn parse_case(&mut self, threshold: u32, depth: u32) -> DResult<Expr> {
        if depth > MAX_DEPTH {
            return Err(self.too_deep());
        }
        let case_tok = self.expect(&Tok::Case)?;
        let scrutinee = self.parse_expr(threshold, depth + 1)?;
        self.expect(&Tok::Of)?;

        let Some(first) = self.peek() else {
            return Err(self.eof_err());
        };
        let arm_col = first.col;
        if arm_col <= threshold {
            return Err(self.err_here());
        }

        let mut arms = Vec::new();
        let mut end = scrutinee.span;
        while self.peek_aligned_at(arm_col) {
            let pat = self.parse_pattern(depth + 1)?;
            self.expect(&Tok::Arrow)?;
            let body = self.parse_expr(arm_col, depth + 1)?;
            end = body.span;
            arms.push((pat, body));
        }
        if arms.is_empty() {
            return Err(self.err_here());
        }
        let span = Self::span_merge(case_tok.span, end);
        Ok(Located::new(span, Expr_::Case(Box::new(scrutinee), arms)))
    }

    // ---- patterns ---------------------------------------------------------

    /// A full pattern, gathering constructor sub-patterns (case-arm position).
    fn parse_pattern(&mut self, depth: u32) -> DResult<Pattern> {
        if depth > MAX_DEPTH {
            return Err(self.too_deep());
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
            return Err(self.too_deep());
        }
        let tok = self.bump()?;
        match &tok.kind {
            Tok::Underscore => Ok(Located::new(tok.span, Pattern_::PAnything)),
            Tok::LParen => {
                let inner = self.parse_pattern(depth + 1)?;
                self.expect(&Tok::RParen)?;
                Ok(Located::new(tok.span, inner.value))
            }
            Tok::Ident(text) => {
                let first_upper = text.chars().next().is_some_and(|c| c.is_ascii_uppercase());
                if first_upper {
                    let mut segs: Vec<Symbol> =
                        text.split('.').map(|s| self.interner.intern(s)).collect();
                    let name = segs.pop().unwrap_or_else(|| self.interner.intern(text));
                    Ok(Located::new(
                        tok.span,
                        Pattern_::PCtor(name, segs, Vec::new()),
                    ))
                } else {
                    let sym = self.interner.intern(text);
                    Ok(Located::new(tok.span, Pattern_::PVar(sym)))
                }
            }
            _ => Err(Diagnostic::Parse {
                span: tok.span,
                msg: ParseError::Unexpected,
            }),
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
