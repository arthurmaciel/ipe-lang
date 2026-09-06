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
//! The token stream is produced by [`ipe_annotate::annotate`] (the shared SSOT
//! for both highlighting and term-to-definition linking) and then projected to
//! this legend.  Semantic classification therefore cannot drift from the shared
//! API.

use ipe_annotate::TokenClass;
use ipe_db::{Db as _, IpeDatabase, SourceFile};
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
// Token collection — thin projection over ipe_annotate::annotate
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

    // Produce annotated tokens via the shared API.  We use `annotate_syntax_only`
    // here because the LSP path does not yet run the full canonicaliser on every
    // keypress; the syntax-only path keeps the same token set the previous
    // hand-written walk produced (class-only, no def keys).
    let annotated = ipe_annotate::annotate_syntax_only(&module, &interner);

    drop(interner);

    // Project each AnnotatedToken to an LSP RawToken (class → legend index).
    let raw: Vec<RawToken> = annotated
        .into_iter()
        .filter_map(|tok| {
            let token_type = class_to_lsp(tok.class)?;
            Some(RawToken {
                byte: tok.byte_start,
                len: tok.byte_len,
                token_type,
            })
        })
        .collect();

    encode(raw, text, encoding)
}

/// Project a [`TokenClass`] to an LSP token type index, or `None` for classes
/// the LSP legend does not expose (e.g. `Comment`, `Punctuation`).
const fn class_to_lsp(class: TokenClass) -> Option<u32> {
    match class {
        TokenClass::Module => Some(TT_NAMESPACE),
        TokenClass::Type => Some(TT_TYPE),
        TokenClass::TypeVar => Some(TT_TYPE_PARAMETER),
        TokenClass::Function | TokenClass::Kernel => Some(TT_FUNCTION),
        TokenClass::Variable => Some(TT_VARIABLE),
        TokenClass::Constructor => Some(TT_ENUM_MEMBER),
        TokenClass::Keyword => Some(TT_KEYWORD),
        TokenClass::StringLit => Some(TT_STRING),
        TokenClass::Number => Some(TT_NUMBER),
        TokenClass::Operator => Some(TT_OPERATOR),
        TokenClass::Comment | TokenClass::Punctuation => None,
    }
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
        let slice = text
            .get(tok.byte as usize..(tok.byte + tok.len) as usize)
            .unwrap_or("");
        let length = match encoding {
            crate::offset::PositionEncoding::Utf8 => tok.len,
            crate::offset::PositionEncoding::Utf16 => slice
                .chars()
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
// The helper functions below are DELETED — they were part of the previous
// hand-written walk and are now fully superseded by ipe_annotate::annotate.
// The removal proves there is no dual maintenance: any classification logic
// lives in ipe_annotate and projects here via class_to_lsp.
// ---------------------------------------------------------------------------

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
        let _ = line;
    }
}
