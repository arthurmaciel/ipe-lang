//! Column-based layout helpers.
//!
//! Ipe (like Elm and the compiler) uses indentation-significant layout. Rather than
//! splice synthetic `{`/`;`/`}` tokens into the stream (the classic the compiler
//! layout algorithm in `Ipe.Parse.Space`), the parser keeps the raw token
//! stream and decides block membership from each token's column relative to a
//! *threshold* column established by the enclosing construct.
//!
//! The single rule: a token continues the current block iff its column is
//! strictly greater than the block's threshold. A token at exactly the block's
//! starting column begins a new sibling (e.g. the next `case` arm or the next
//! top-level declaration); a token at a smaller column closes the block.

use crate::lexer::Token;

/// Does `tok` continue a block whose threshold column is `threshold`?
#[must_use]
pub const fn continues_block(tok: &Token, threshold: u32) -> bool {
    tok.col > threshold
}

/// Does `tok` start a new sibling aligned at `align` (same column)?
#[must_use]
pub const fn aligned_at(tok: &Token, align: u32) -> bool {
    tok.col == align
}
