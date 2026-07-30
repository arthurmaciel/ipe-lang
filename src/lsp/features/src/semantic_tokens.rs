//! Semantic tokens: `textDocument/semanticTokens/full`.
//!
//! Classifies every token in the parse tree into one of the types in
//! [`LEGEND`] and returns the LSP delta-encoded token array.
//!
//! **Token types (indexes into the legend):**
//!
//! | Index | `tokenTypes` entry  | What it covers                       |
//! |-------|---------------------|--------------------------------------|
//! | 0     | `namespace`         | Module name in `module` / `import`   |
//! | 1     | `type`              | Type constructor in annotations       |
//! | 2     | `typeParameter`     | Type variable in annotations          |
//! | 3     | `function`          | Top-level value / function name       |
//! | 4     | `variable`          | Local variable in patterns            |
//! | 5     | `enumMember`        | Constructor in patterns / expressions |
//! | 6     | `keyword`           | Keywords (`module`, `import`, …)      |
//! | 7     | `string`            | String / char / multiline-str literal |
//! | 8     | `number`            | Integer / float literal               |
//! | 9     | `operator`          | Binary operator                       |
//!
//! Only `tokenTypes` is used; `tokenModifiers` is empty.
//!
//! The walk is over the parse AST — no type info needed, so tokens are
//! available even when the program doesn't type-check.

use ipe_db::{Db as _, IpeDatabase, SourceFile};
use ipe_diagnostics::Span;
use lsp_types::{SemanticToken, SemanticTokens, SemanticTokensLegend, SemanticTokensResult};

use crate::offset::{PositionEncoding, offset_to_position};

// ---------------------------------------------------------------------------
// Legend
// ---------------------------------------------------------------------------

/// Token type indexes — must stay in sync with [`LEGEND`].
const TT_NAMESPACE: u32 = 0;
const TT_TYPE: u32 = 1;
const TT_TYPE_PARAMETER: u32 = 2;
const TT_FUNCTION: u32 = 3;
const TT_VARIABLE: u32 = 4;
const TT_ENUM_MEMBER: u32 = 5;
const TT_KEYWORD: u32 = 6;
const TT_STRING: u32 = 7;
const TT_NUMBER: u32 = 8;
const TT_OPERATOR: u32 = 9;

/// The legend this server advertises and uses.
#[must_use]
pub fn legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: vec![
            lsp_types::SemanticTokenType::NAMESPACE,
            lsp_types::SemanticTokenType::TYPE,
            lsp_types::SemanticTokenType::TYPE_PARAMETER,
            lsp_types::SemanticTokenType::FUNCTION,
            lsp_types::SemanticTokenType::VARIABLE,
            lsp_types::SemanticTokenType::ENUM_MEMBER,
            lsp_types::SemanticTokenType::KEYWORD,
            lsp_types::SemanticTokenType::STRING,
            lsp_types::SemanticTokenType::NUMBER,
            lsp_types::SemanticTokenType::OPERATOR,
        ],
        token_modifiers: vec![],
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Full semantic-token encoding for one document.
///
/// Returns an empty result for an unparseable document — the client falls back
/// to syntax highlighting.
#[must_use]
pub fn semantic_tokens_full(
    db: &IpeDatabase,
    file: SourceFile,
    encoding: PositionEncoding,
) -> SemanticTokensResult {
    let tokens = collect_tokens(db, file, encoding);
    SemanticTokensResult::Tokens(SemanticTokens {
        result_id: None,
        data: tokens,
    })
}

// ---------------------------------------------------------------------------
// Token collection
// ---------------------------------------------------------------------------

/// A raw token before delta-encoding.
#[derive(Debug, Clone, Copy)]
struct RawToken {
    /// Byte offset of the token start.
    byte: u32,
    /// Byte length of the token.
    len: u32,
    /// LSP token type index.
    token_type: u32,
}

fn collect_tokens(
    db: &IpeDatabase,
    file: SourceFile,
    encoding: PositionEncoding,
) -> Vec<SemanticToken> {
    let Ok(module) = ipe_db::parse(db, file) else {
        return Vec::new();
    };
    let text = file.text(db);
    let interner = db.interner().lock();

    let mut raw: Vec<RawToken> = Vec::new();

    // Module keyword + name.
    push_keyword(&mut raw, keyword_span(text, 0, "module"));
    push_span(&mut raw, module.name.span, TT_NAMESPACE);

    // Imports.
    for imp in &module.imports {
        if let Some(kw) = find_keyword_before(text, imp.name.span.lo, "import") {
            push_keyword(&mut raw, kw);
        }
        push_span(&mut raw, imp.name.span, TT_NAMESPACE);
        // `as Alias` — the alias is also a namespace token.
        if let Some(alias_sym) = imp.alias {
            // Find the `as` keyword.
            if let Some(kw) = find_keyword_before(text, imp.name.span.hi, "as") {
                let _ = kw; // keyword is syntactically before the alias token
            }
            let _ = alias_sym; // alias is a simple symbol; we tag the import name
        }
        // Exposed names in `exposing (...)` are tagged in push_exposing_tokens.
        push_exposing_tokens(&mut raw, &imp.exposing.value, text, &interner);
    }

    // Type aliases.
    for alias in &module.aliases {
        if let Some(kw) = find_keyword_before(text, alias.value.name.span.lo, "type") {
            push_keyword(&mut raw, kw);
        }
        push_span(&mut raw, alias.value.name.span, TT_TYPE);
        for var in &alias.value.vars {
            push_span(&mut raw, var.span, TT_TYPE_PARAMETER);
        }
        push_type_annotation_tokens(&mut raw, &alias.value.body.value);
    }

    // Union types.
    for union in &module.unions {
        if let Some(kw) = find_keyword_before(text, union.value.name.span.lo, "type") {
            push_keyword(&mut raw, kw);
        }
        push_span(&mut raw, union.value.name.span, TT_TYPE);
        for var in &union.value.vars {
            push_span(&mut raw, var.span, TT_TYPE_PARAMETER);
        }
        for ctor in &union.value.ctors {
            let name_len = interner.resolve(ctor.value.name).map_or(0, byte_len_u32);
            push_span(
                &mut raw,
                Span::new(ctor.span.lo, ctor.span.lo + name_len),
                TT_ENUM_MEMBER,
            );
        }
    }

    // Values.
    for value in &module.values {
        push_span(&mut raw, value.value.name.span, TT_FUNCTION);
        if let Some(ann) = &value.value.type_annotation {
            push_type_annotation_tokens(&mut raw, &ann.value);
        }
        for pat in &value.value.patterns {
            push_pattern_tokens(&mut raw, pat, &interner);
        }
        push_expr_tokens(&mut raw, &value.value.body, &interner);
    }

    drop(interner);

    // Sort by byte offset, remove duplicates/overlaps.
    raw.sort_by_key(|t| t.byte);
    raw.dedup_by_key(|t| t.byte);

    // Delta-encode into LSP SemanticToken array.
    encode(raw, text, encoding)
}

// ---------------------------------------------------------------------------
// Delta encoding
// ---------------------------------------------------------------------------

fn encode(raw: Vec<RawToken>, text: &str, encoding: PositionEncoding) -> Vec<SemanticToken> {
    let mut out: Vec<SemanticToken> = Vec::with_capacity(raw.len());
    let mut prev_line: u32 = 0;
    let mut prev_char: u32 = 0;

    for tok in raw {
        let pos = offset_to_position(text, tok.byte as usize, encoding);
        let delta_line = pos.line - prev_line;
        let delta_start = if delta_line == 0 {
            pos.character - prev_char
        } else {
            pos.character
        };
        // Length in encoding units (UTF-16 or UTF-8 columns).
        let slice = text
            .get(tok.byte as usize..(tok.byte + tok.len) as usize)
            .unwrap_or("");
        let length = match encoding {
            crate::offset::PositionEncoding::Utf8 => tok.len,
            crate::offset::PositionEncoding::Utf16 => slice
                .chars()
                // `len_utf16` is 1 or 2 — always representable as u32.
                .map(|c| u32::try_from(c.len_utf16()).unwrap_or(2))
                .sum(),
        };
        out.push(SemanticToken {
            delta_line,
            delta_start,
            length,
            token_type: tok.token_type,
            token_modifiers_bitset: 0,
        });
        prev_line = pos.line;
        prev_char = pos.character;
    }
    out
}

// ---------------------------------------------------------------------------
// AST walkers
// ---------------------------------------------------------------------------

fn push_span(raw: &mut Vec<RawToken>, span: Span, token_type: u32) {
    if span.lo >= span.hi {
        return;
    }
    raw.push(RawToken {
        byte: span.lo,
        len: span.hi - span.lo,
        token_type,
    });
}

fn push_keyword(raw: &mut Vec<RawToken>, span: Span) {
    push_span(raw, span, TT_KEYWORD);
}

/// Scan backwards from `before_byte` to find a keyword token in `text`.
fn find_keyword_before(text: &str, before_byte: u32, keyword: &str) -> Option<Span> {
    // Simple scan: look for the keyword as a substring ending before
    // `before_byte`. We only look within 32 bytes to avoid false matches.
    let window_start = (before_byte as usize).saturating_sub(64);
    let window = text.get(window_start..before_byte as usize)?;
    let kw_pos = window.rfind(keyword)?;
    let abs_start = offset_u32(window_start + kw_pos);
    // Verify word boundaries — must be preceded by whitespace/start and
    // followed by whitespace.
    let before_ok = abs_start == 0
        || text
            .as_bytes()
            .get(abs_start as usize - 1)
            .is_none_or(u8::is_ascii_whitespace);
    let after_ok = text
        .as_bytes()
        .get(abs_start as usize + keyword.len())
        .is_none_or(|b| b.is_ascii_whitespace() || *b == b'(');
    if before_ok && after_ok {
        Some(Span::new(abs_start, abs_start + byte_len_u32(keyword)))
    } else {
        None
    }
}

/// A byte length narrowed to the `u32` source spans use. Identifiers and
/// keywords are far shorter than `u32::MAX`, so the saturating fallback is
/// unreachable in practice; it keeps the conversion total.
fn byte_len_u32(s: &str) -> u32 {
    u32::try_from(s.len()).unwrap_or(u32::MAX)
}

/// A byte offset narrowed to the `u32` source spans use. Source files are far
/// smaller than `u32::MAX` bytes, so the saturating fallback is unreachable in
/// practice; it keeps the conversion total.
fn offset_u32(byte: usize) -> u32 {
    u32::try_from(byte).unwrap_or(u32::MAX)
}

/// Locate `"module"` at (or just before) `hint_byte`.
fn keyword_span(text: &str, hint_byte: usize, keyword: &str) -> Span {
    let start = text.find(keyword).unwrap_or(hint_byte);
    Span::new(offset_u32(start), offset_u32(start + keyword.len()))
}

fn push_exposing_tokens(
    raw: &mut Vec<RawToken>,
    exposing: &ipe_syntax::Exposing,
    _text: &str,
    _interner: &ipe_intern::Interner,
) {
    match exposing {
        ipe_syntax::Exposing::All => {}
        ipe_syntax::Exposing::List(items) => {
            for item in items {
                match &item.value {
                    ipe_syntax::Exposed::Value(_sym) => {
                        push_span(raw, item.span, TT_FUNCTION);
                    }
                    ipe_syntax::Exposed::Type(_sym, _) => {
                        push_span(raw, item.span, TT_TYPE);
                    }
                }
            }
        }
    }
}

/// Highlight the tokens of a type annotation.
///
/// The `TypeAnnotation` AST carries no per-node spans, so there is currently
/// nothing to emit — annotations stay un-highlighted until the parser records
/// their spans. Kept as a named seam so the call sites read intentionally and
/// gain highlighting the moment spans are available.
const fn push_type_annotation_tokens(_raw: &mut [RawToken], _ty: &ipe_syntax::TypeAnnotation) {}

fn push_pattern_tokens(
    raw: &mut Vec<RawToken>,
    pat: &ipe_syntax::Pattern,
    interner: &ipe_intern::Interner,
) {
    match &pat.value {
        ipe_syntax::Pattern_::PAnything => {}
        ipe_syntax::Pattern_::PVar(_) => {
            push_span(raw, pat.span, TT_VARIABLE);
        }
        ipe_syntax::Pattern_::PCtor(name, _module_segs, args) => {
            let name_len = interner.resolve(*name).map_or(0, byte_len_u32);
            push_span(
                raw,
                Span::new(pat.span.lo, pat.span.lo + name_len),
                TT_ENUM_MEMBER,
            );
            for arg in args {
                push_pattern_tokens(raw, arg, interner);
            }
        }
        ipe_syntax::Pattern_::PTuple(elems) | ipe_syntax::Pattern_::PList(elems) => {
            for e in elems {
                push_pattern_tokens(raw, e, interner);
            }
        }
        ipe_syntax::Pattern_::PRecord(fields) => {
            for f in fields {
                push_span(raw, f.span, TT_VARIABLE);
            }
        }
        ipe_syntax::Pattern_::PInt(_) => {
            push_span(raw, pat.span, TT_NUMBER);
        }
        ipe_syntax::Pattern_::PBool(_) => {
            push_span(raw, pat.span, TT_ENUM_MEMBER);
        }
        ipe_syntax::Pattern_::PChar(_) | ipe_syntax::Pattern_::PStr(_) => {
            push_span(raw, pat.span, TT_STRING);
        }
        ipe_syntax::Pattern_::PAlias(inner, name) => {
            push_pattern_tokens(raw, inner, interner);
            push_span(raw, name.span, TT_VARIABLE);
        }
        ipe_syntax::Pattern_::PCons(h, t) => {
            push_pattern_tokens(raw, h, interner);
            push_pattern_tokens(raw, t, interner);
        }
        ipe_syntax::Pattern_::POr(alts) => {
            for alt in alts {
                push_pattern_tokens(raw, alt, interner);
            }
        }
    }
}

fn push_expr_tokens(
    raw: &mut Vec<RawToken>,
    expr: &ipe_syntax::Expr,
    interner: &ipe_intern::Interner,
) {
    match &expr.value {
        ipe_syntax::Expr_::VarLocal(_) => {
            push_span(raw, expr.span, TT_VARIABLE);
        }
        ipe_syntax::Expr_::VarQual(_qualifier, _name) => {
            push_span(raw, expr.span, TT_FUNCTION);
        }
        ipe_syntax::Expr_::Int(_) | ipe_syntax::Expr_::Float(_) => {
            push_span(raw, expr.span, TT_NUMBER);
        }
        ipe_syntax::Expr_::Str(_)
        | ipe_syntax::Expr_::MultilineStr { .. }
        | ipe_syntax::Expr_::Char(_) => {
            push_span(raw, expr.span, TT_STRING);
        }
        ipe_syntax::Expr_::Unit => {}
        ipe_syntax::Expr_::Call(f, args) => {
            push_expr_tokens(raw, f, interner);
            for arg in args {
                push_expr_tokens(raw, arg, interner);
            }
        }
        ipe_syntax::Expr_::Binops(pairs, last) => {
            for (lhs, op) in pairs {
                push_expr_tokens(raw, lhs, interner);
                push_span(raw, op.span, TT_OPERATOR);
            }
            push_expr_tokens(raw, last, interner);
        }
        ipe_syntax::Expr_::Lambda(params, body) => {
            for p in params {
                push_pattern_tokens(raw, p, interner);
            }
            push_expr_tokens(raw, body, interner);
        }
        ipe_syntax::Expr_::Let(bindings, body) => {
            for b in bindings {
                push_pattern_tokens(raw, &b.pat, interner);
                push_expr_tokens(raw, &b.body, interner);
            }
            push_expr_tokens(raw, body, interner);
        }
        ipe_syntax::Expr_::If(branches, else_expr) => {
            for (cond, then_) in branches {
                push_expr_tokens(raw, cond, interner);
                push_expr_tokens(raw, then_, interner);
            }
            push_expr_tokens(raw, else_expr, interner);
        }
        ipe_syntax::Expr_::Case(scrutinee, branches) => {
            push_expr_tokens(raw, scrutinee, interner);
            for (pat, body) in branches {
                push_pattern_tokens(raw, pat, interner);
                push_expr_tokens(raw, body, interner);
            }
        }
        ipe_syntax::Expr_::Tuple(elems) | ipe_syntax::Expr_::List(elems) => {
            for e in elems {
                push_expr_tokens(raw, e, interner);
            }
        }
        ipe_syntax::Expr_::Record(fields) => {
            for (name, val) in fields {
                push_span(raw, name.span, TT_VARIABLE);
                push_expr_tokens(raw, val, interner);
            }
        }
        ipe_syntax::Expr_::Access(rec, field) => {
            push_expr_tokens(raw, rec, interner);
            push_span(raw, field.span, TT_VARIABLE);
        }
        ipe_syntax::Expr_::Update(base, fields) => {
            push_span(raw, base.span, TT_VARIABLE);
            for (name, val) in fields {
                push_span(raw, name.span, TT_VARIABLE);
                push_expr_tokens(raw, val, interner);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use ipe_db::{IpeDatabase, ModuleOrigin, SourceFile};

    use super::{legend, semantic_tokens_full};
    use crate::offset::PositionEncoding;

    fn file(db: &IpeDatabase, path: &[&str], text: &str) -> SourceFile {
        ipe_db::SourceFile::new(
            db,
            path.iter().map(|s| (*s).to_owned()).collect(),
            text.to_owned(),
            ModuleOrigin::User,
        )
    }

    /// Unwrap the full-encoding result to its token list. `semantic_tokens_full`
    /// only ever returns the `Tokens` variant; the `Partial` case maps to `None`
    /// so the caller's `.expect` fails the test rather than panicking inline.
    fn tokens_of(result: lsp_types::SemanticTokensResult) -> lsp_types::SemanticTokens {
        let tokens = match result {
            lsp_types::SemanticTokensResult::Tokens(tokens) => Some(tokens),
            lsp_types::SemanticTokensResult::Partial(_) => None,
        };
        tokens.expect("semantic_tokens_full returns the Tokens variant")
    }

    #[test]
    fn legend_has_ten_token_types() {
        assert_eq!(legend().token_types.len(), 10);
    }

    #[test]
    fn tokens_non_empty_for_valid_module() {
        let db = IpeDatabase::new();
        let src = "module Main exposing (main)\n\nmain : Int\nmain =\n    42\n";
        let f = file(&db, &["Main"], src);
        let result = semantic_tokens_full(&db, f, PositionEncoding::Utf16);
        let tokens = tokens_of(result);
        assert!(!tokens.data.is_empty(), "tokens produced for valid module");
    }

    #[test]
    fn no_tokens_for_unparseable_module() {
        let db = IpeDatabase::new();
        let f = file(&db, &["Main"], "@@@ not parseable @@@");
        let result = semantic_tokens_full(&db, f, PositionEncoding::Utf16);
        let tokens = tokens_of(result);
        assert!(tokens.data.is_empty(), "no tokens for unparseable source");
    }

    #[test]
    fn tokens_are_delta_sorted() {
        let db = IpeDatabase::new();
        let src = "module Main exposing (main)\n\nmain : Int\nmain =\n    42\n";
        let f = file(&db, &["Main"], src);
        let result = semantic_tokens_full(&db, f, PositionEncoding::Utf16);
        let tokens = tokens_of(result);
        // In a valid delta encoding, line deltas are non-negative and a
        // zero-delta line implies a non-negative character delta.
        let mut line: u32 = 0;
        let mut col: u32 = 0;
        for tok in &tokens.data {
            if tok.delta_line == 0 {
                assert!(
                    tok.delta_start >= col || col == 0,
                    "tokens on the same line must advance right"
                );
                col += tok.delta_start;
            } else {
                line += tok.delta_line;
                col = tok.delta_start;
            }
        }
        let _ = line; // used only for ordering check
    }
}
