//! The one position mapper: compiler byte offsets ↔ LSP line/character
//! positions.
//!
//! Compiler [`Span`]s are byte offsets into a module's UTF-8 source; LSP
//! positions count lines plus characters in a client-negotiated encoding
//! (UTF-16 code units by default, UTF-8 bytes when negotiated). Every
//! span↔range crossing in the LSP goes through this module — there is no
//! second conversion path to drift.
//!
//! Every function is total: out-of-range offsets, positions past the end of a
//! line or file, and mid-code-point byte offsets all clamp to the nearest
//! valid boundary instead of panicking.

use ipe_diagnostics::Span;
use lsp_types::{Position, Range};

/// The negotiated character-counting unit for LSP positions.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PositionEncoding {
    /// Characters are UTF-8 bytes (the identity for compiler spans).
    Utf8,
    /// Characters are UTF-16 code units (the LSP default, mandatory).
    Utf16,
}

impl PositionEncoding {
    /// The width of `ch` in this encoding's units.
    const fn width(self, ch: char) -> usize {
        match self {
            Self::Utf8 => ch.len_utf8(),
            Self::Utf16 => ch.len_utf16(),
        }
    }
}

/// The largest char boundary `<= byte` (and `<= text.len()`).
fn floor_char_boundary(text: &str, byte: usize) -> usize {
    let mut b = byte.min(text.len());
    while b > 0 && !text.is_char_boundary(b) {
        b -= 1;
    }
    b
}

/// A one-pass index of a document's line-start byte offsets.
///
/// A feature that converts many spans in one request resolves each in
/// `O(log lines)` plus a within-line scan, rather than rescanning the whole
/// prefix from byte 0 per conversion (which is `O(items × file_len)` across a
/// diagnostics / symbols / inlay-hints loop). Build it once per file text and
/// reuse it for every conversion in the request.
pub struct LineIndex<'a> {
    text: &'a str,
    /// Byte offset of each line's first character. Always starts with `0`.
    line_starts: Vec<usize>,
}

impl<'a> LineIndex<'a> {
    /// Build the index over `text` in one linear pass.
    #[must_use]
    pub fn new(text: &'a str) -> Self {
        let mut line_starts = vec![0usize];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        Self { text, line_starts }
    }

    /// Convert a byte offset to an LSP position, clamping like
    /// [`offset_to_position`] but using the prebuilt line table.
    #[must_use]
    pub fn position_of(&self, byte: usize, encoding: PositionEncoding) -> Position {
        let byte = floor_char_boundary(self.text, byte);
        // The containing line is the last line start `<= byte`.
        let line = match self.line_starts.binary_search(&byte) {
            Ok(l) => l,
            Err(i) => i.saturating_sub(1),
        };
        let line_start = self.line_starts.get(line).copied().unwrap_or(0);
        let column: usize = self
            .text
            .get(line_start..byte)
            .unwrap_or("")
            .chars()
            .map(|ch| encoding.width(ch))
            .sum();
        Position {
            line: u32::try_from(line).unwrap_or(u32::MAX),
            character: u32::try_from(column).unwrap_or(u32::MAX),
        }
    }

    /// Convert a compiler [`Span`] to an LSP range using the prebuilt table.
    #[must_use]
    pub fn range_of(&self, span: Span, encoding: PositionEncoding) -> Range {
        let start = self.position_of(span.lo as usize, encoding);
        let end_byte = (span.hi as usize).max(span.lo as usize);
        let end = self.position_of(end_byte, encoding);
        Range { start, end }
    }
}

/// Convert a byte offset into `text` to an LSP position. Clamps out-of-range
/// and mid-code-point offsets down to the nearest char boundary.
#[must_use]
pub fn offset_to_position(text: &str, byte: usize, encoding: PositionEncoding) -> Position {
    let byte = floor_char_boundary(text, byte);
    let before = text.get(..byte).unwrap_or("");
    let line = before.bytes().filter(|&b| b == b'\n').count();
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let column: usize = before
        .get(line_start..)
        .unwrap_or("")
        .chars()
        .map(|ch| encoding.width(ch))
        .sum();
    Position {
        line: u32::try_from(line).unwrap_or(u32::MAX),
        character: u32::try_from(column).unwrap_or(u32::MAX),
    }
}

/// Convert an LSP position to a byte offset into `text`. A line past the end
/// of the file clamps to `text.len()`; a character past the end of its line
/// clamps to the end of that line (before the newline).
#[must_use]
pub fn position_to_offset(text: &str, position: Position, encoding: PositionEncoding) -> usize {
    let mut line_start = 0usize;
    for _ in 0..position.line {
        match text.get(line_start..).and_then(|rest| rest.find('\n')) {
            Some(nl) => line_start += nl + 1,
            None => return text.len(),
        }
    }
    let rest = text.get(line_start..).unwrap_or("");
    let line_end = rest.find('\n').unwrap_or(rest.len());
    let line_text = rest.get(..line_end).unwrap_or("");

    let target = position.character as usize;
    let mut units = 0usize;
    let mut byte = 0usize;
    for ch in line_text.chars() {
        if units >= target {
            break;
        }
        units += encoding.width(ch);
        byte += ch.len_utf8();
    }
    line_start + byte
}

/// Convert a compiler [`Span`] (byte offsets into `text`) to an LSP range.
/// An inverted span clamps to an empty range at its start.
#[must_use]
pub fn span_to_range(text: &str, span: Span, encoding: PositionEncoding) -> Range {
    let start = offset_to_position(text, span.lo as usize, encoding);
    let end_byte = (span.hi as usize).max(span.lo as usize);
    let end = offset_to_position(text, end_byte, encoding);
    Range { start, end }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NASTY: &[&str] = &[
        "",
        "\n",
        "plain ascii\nsecond line\n",
        "no trailing newline",
        "crlf line one\r\nsecond\r\n",
        "emoji 😀 astral\nsecond 😀😀 line\n",
        "cjk 漢字テスト\nмежду строк\n",
        "combining e\u{301} mark\ne\u{301}\u{301}\n",
        "mixed 😀漢e\u{301}x\r\ntail",
        "\u{10FFFF}\n\u{10FFFF}ascii",
    ];

    #[test]
    fn line_index_matches_the_scalar_mapper_everywhere() {
        // The bulk `LineIndex` path must agree byte-for-byte with the scalar
        // `offset_to_position` on every char boundary of the nasty corpus, so a
        // feature can switch to it with no observable change.
        for text in NASTY {
            let index = LineIndex::new(text);
            for encoding in [PositionEncoding::Utf8, PositionEncoding::Utf16] {
                for byte in 0..=text.len() {
                    assert_eq!(
                        index.position_of(byte, encoding),
                        offset_to_position(text, byte, encoding),
                        "LineIndex disagreed at byte {byte} of {text:?} ({encoding:?})"
                    );
                }
            }
        }
    }

    #[test]
    fn round_trip_identity_on_every_char_boundary() {
        for text in NASTY {
            for encoding in [PositionEncoding::Utf8, PositionEncoding::Utf16] {
                for byte in 0..=text.len() {
                    if !text.is_char_boundary(byte) {
                        continue;
                    }
                    let pos = offset_to_position(text, byte, encoding);
                    let back = position_to_offset(text, pos, encoding);
                    assert_eq!(
                        back, byte,
                        "round trip failed at byte {byte} of {text:?} ({encoding:?})"
                    );
                }
            }
        }
    }

    #[test]
    fn mid_code_point_offsets_clamp_down() {
        let text = "a😀b";
        // Bytes 2, 3, 4 are inside the 4-byte emoji starting at byte 1.
        for encoding in [PositionEncoding::Utf8, PositionEncoding::Utf16] {
            for mid in 2..=4 {
                let pos = offset_to_position(text, mid, encoding);
                assert_eq!(pos, offset_to_position(text, 1, encoding));
            }
        }
    }

    #[test]
    fn out_of_range_positions_clamp() {
        let text = "one\ntwo\n";
        for encoding in [PositionEncoding::Utf8, PositionEncoding::Utf16] {
            // Line past EOF → end of text.
            let past_line = Position {
                line: 99,
                character: 0,
            };
            assert_eq!(position_to_offset(text, past_line, encoding), text.len());
            // Character past end of line → end of that line, before `\n`.
            let past_col = Position {
                line: 0,
                character: 99,
            };
            assert_eq!(position_to_offset(text, past_col, encoding), 3);
            // Byte offset past EOF → last position.
            let end = offset_to_position(text, 9999, encoding);
            assert_eq!(position_to_offset(text, end, encoding), text.len());
        }
    }

    #[test]
    fn utf16_counts_astral_pairs() {
        let text = "😀x";
        let pos = offset_to_position(text, 4, PositionEncoding::Utf16);
        assert_eq!(
            pos,
            Position {
                line: 0,
                character: 2
            }
        );
        let pos8 = offset_to_position(text, 4, PositionEncoding::Utf8);
        assert_eq!(
            pos8,
            Position {
                line: 0,
                character: 4
            }
        );
    }

    #[test]
    fn span_to_range_spans_lines() {
        let text = "ab\ncd\n";
        let range = span_to_range(text, Span::new(1, 4), PositionEncoding::Utf16);
        assert_eq!(
            range.start,
            Position {
                line: 0,
                character: 1
            }
        );
        assert_eq!(
            range.end,
            Position {
                line: 1,
                character: 1
            }
        );
    }

    #[test]
    fn inverted_span_clamps_to_empty() {
        let text = "abcdef";
        let range = span_to_range(text, Span { lo: 4, hi: 2 }, PositionEncoding::Utf16);
        assert_eq!(range.start, range.end);
    }
}
