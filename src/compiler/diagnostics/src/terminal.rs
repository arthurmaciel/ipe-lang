//! Terminal-safe text: the one sanitiser every crate that prints untrusted text uses.
//!
//! It lives in this lowest shared crate so a compiler stage that carries a
//! foreign string into a user-facing refusal (a dependency name, a file label,
//! a child process's stderr) parses it into [`TerminalSafe`] where the value is
//! built, with the same rules the CLI applies to every message it prints.

use std::fmt;
use std::ops::RangeInclusive;

/// Format characters that reorder, hide, or break the visible text without
/// being control bytes.
///
/// The bidirectional marks, embeddings, overrides, and isolates; the Unicode
/// line and paragraph separators; and the invisible format characters (soft
/// hyphen, zero-width space and joiners, word joiner and invisible operators,
/// deprecated format controls, byte-order mark, interlinear annotation
/// controls, and the tag block).
///
/// A value carrying one could make the terminal show text in an order other
/// than the bytes', open a line the CLI never wrote, or hide characters that
/// change what a name means, so [`TerminalSafe`] drops each of them. No
/// CLI message relies on a zero-width joiner, so it is dropped too.
pub const DENIED_FORMAT_CHARS: &[RangeInclusive<char>] = &[
    '\u{00AD}'..='\u{00AD}',   // SOFT HYPHEN
    '\u{061C}'..='\u{061C}',   // ARABIC LETTER MARK
    '\u{200B}'..='\u{200D}',   // ZERO WIDTH SPACE, NON-JOINER, JOINER
    '\u{200E}'..='\u{200F}',   // LEFT-TO-RIGHT / RIGHT-TO-LEFT MARK
    '\u{2028}'..='\u{2029}',   // LINE / PARAGRAPH SEPARATOR
    '\u{202A}'..='\u{202E}',   // bidi EMBEDDINGs, POP, OVERRIDEs
    '\u{2060}'..='\u{2064}',   // WORD JOINER, invisible operators
    '\u{2066}'..='\u{2069}',   // bidi ISOLATEs, POP DIRECTIONAL ISOLATE
    '\u{206A}'..='\u{206F}',   // deprecated format controls
    '\u{FEFF}'..='\u{FEFF}',   // ZERO WIDTH NO-BREAK SPACE (BOM)
    '\u{FFF9}'..='\u{FFFB}',   // INTERLINEAR ANNOTATION controls
    '\u{E0000}'..='\u{E007F}', // TAG block
];

/// Whether `c` is one of the [`DENIED_FORMAT_CHARS`].
#[must_use]
pub fn is_denied_format_char(c: char) -> bool {
    DENIED_FORMAT_CHARS.iter().any(|range| range.contains(&c))
}

/// Text that has been proven safe to write to a terminal.
///
/// No ANSI escape sequences, no C0/C1 control bytes, no `DEL`, no
/// [`DENIED_FORMAT_CHARS`], only printable characters plus the two layout
/// whitespaces (`\n`, `\t`) the gutter and terminal handle safely.
///
/// A crafted string laced with ANSI escapes or control bytes could move the
/// cursor, recolour or erase lines, or hide text, turning a diagnostic into a
/// spoofing/injection surface. `TerminalSafe` is the typed boundary: construct
/// it ONCE from the untrusted string, and every downstream renderer takes a
/// `TerminalSafe` rather than a bare `&str`, so the unsanitised form is
/// unrepresentable past it. Parse, don't validate.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TerminalSafe(String);

impl TerminalSafe {
    /// Sanitise `raw` into terminal-safe text.
    ///
    /// Drops every ANSI escape sequence (a lone `ESC`, a CSI `ESC [ … final`, or
    /// an OSC `ESC ] … BEL/ST`) whole, and every remaining control byte (C0,
    /// `DEL`, C1) and [`DENIED_FORMAT_CHARS`] entry, keeping only `\n` and `\t`.
    /// A sequence never spans a line: a `\n` inside an unterminated CSI or OSC
    /// ends it and is kept, so one hostile value cannot swallow the lines that
    /// follow it. Other printable text passes through untouched.
    #[must_use]
    pub fn sanitize(raw: &str) -> Self {
        let mut out = String::with_capacity(raw.len());
        let mut chars = raw.chars();
        while let Some(c) = chars.next() {
            if c == '\u{1b}' {
                match chars.clone().next() {
                    Some('[') => {
                        // CSI (`ESC [`) runs until a final byte in 0x40..=0x7e.
                        chars.next();
                        for seq in chars.by_ref() {
                            if seq == '\n' {
                                out.push('\n');
                                break;
                            }
                            if ('\u{40}'..='\u{7e}').contains(&seq) {
                                break;
                            }
                        }
                    }
                    Some(']') => {
                        // OSC (`ESC ]`) runs until BEL (0x07) or ST (`ESC \`).
                        chars.next();
                        while let Some(seq) = chars.next() {
                            if seq == '\n' {
                                out.push('\n');
                                break;
                            }
                            if seq == '\u{7}' {
                                break;
                            }
                            if seq == '\u{1b}' {
                                if chars.clone().next() == Some('\\') {
                                    chars.next();
                                }
                                break;
                            }
                        }
                    }
                    // A lone `ESC` before a line break drops only itself.
                    Some('\n') | None => {}
                    Some(_) => {
                        chars.next();
                    }
                }
                continue;
            }
            if c == '\n' || c == '\t' || (!c.is_control() && !is_denied_format_char(c)) {
                out.push(c);
            }
        }
        Self(out)
    }

    /// The sanitised block text, newlines kept for a renderer that gutters each line.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Indent that opens every continuation line of a [`TerminalSafe`] rendered inline.
///
/// Wider than the output gutter, so a continuation line never starts where a
/// real output line does.
pub const CONTINUATION_INDENT: &str = "    ";

/// The inline form: every line after the first is indented by [`CONTINUATION_INDENT`].
///
/// An inline placeholder renders untrusted text inside a line the caller owns.
/// A newline in that text cannot open a fresh, forged output line: it only
/// continues the owning line, visibly indented. Block renderers that gutter
/// each line themselves take [`TerminalSafe::as_str`] instead.
impl fmt::Display for TerminalSafe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut lines = self.0.split('\n');
        if let Some(first) = lines.next() {
            f.write_str(first)?;
        }
        for line in lines {
            f.write_str("\n")?;
            f.write_str(CONTINUATION_INDENT)?;
            f.write_str(line)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hostile message laced with ANSI escapes and control bytes is stripped
    /// to printable text plus layout whitespace.
    #[test]
    fn terminal_safe_strips_ansi_and_control_bytes() {
        let hostile = "\u{1b}[31mred\u{1b}[0m\u{1b}]0;title\u{7}\rmoved\u{8}\u{7f}done\ttab\nline";
        let safe = TerminalSafe::sanitize(hostile);
        let s = safe.as_str();
        assert!(!s.contains('\u{1b}'), "no ESC survives: {s:?}");
        assert!(!s.contains('\r'), "carriage return dropped: {s:?}");
        assert!(!s.contains('\u{7}'), "bell dropped: {s:?}");
        assert!(!s.contains('\u{8}'), "backspace dropped: {s:?}");
        assert!(!s.contains('\u{7f}'), "DEL dropped: {s:?}");
        assert_eq!(s, "redmoveddone\ttab\nline");
    }

    /// Rendered inline, untrusted text cannot open a forged output line.
    #[test]
    fn terminal_safe_inline_form_indents_continuation_lines() {
        let forged = TerminalSafe::sanitize("oops\n\u{2713} published\n\u{1b}[2Kdone");
        assert_eq!(
            forged.to_string(),
            format!("oops\n{CONTINUATION_INDENT}\u{2713} published\n{CONTINUATION_INDENT}done")
        );
        assert_eq!(TerminalSafe::sanitize("one line").to_string(), "one line");
    }

    /// An OSC closed by ST (`ESC \\`) is dropped whole, terminator included,
    /// and the text after it survives.
    #[test]
    fn terminal_safe_strips_st_terminated_osc() {
        let safe = TerminalSafe::sanitize("a\u{1b}]8;;https://evil\u{1b}\\b");
        assert_eq!(safe.as_str(), "ab");
    }

    /// An unterminated OSC or CSI ends at the next line break: the break and
    /// every later line survive, so one value cannot swallow what follows it.
    #[test]
    fn an_unterminated_sequence_stops_at_the_line_break() {
        let osc = TerminalSafe::sanitize("dep\u{1b}]8;;evil\nnext line\nlast");
        assert_eq!(osc.as_str(), "dep\nnext line\nlast");
        // `;` and digits are CSI parameter bytes, never a final byte, so only
        // the line break can end this sequence.
        let csi = TerminalSafe::sanitize("dep\u{1b}[1;2;3\n42 kept");
        assert_eq!(csi.as_str(), "dep\n42 kept");
        let lone = TerminalSafe::sanitize("a\u{1b}\nb");
        assert_eq!(lone.as_str(), "a\nb");
    }

    /// Every denied format character — both ends of each range — is dropped,
    /// so a value cannot reorder, hide, or break the visible text.
    #[test]
    fn terminal_safe_strips_every_denied_format_char() {
        for range in DENIED_FORMAT_CHARS {
            for denied in [*range.start(), *range.end()] {
                let safe = TerminalSafe::sanitize(&format!("a{denied}b"));
                assert_eq!(safe.as_str(), "ab", "{denied:?} survived");
            }
        }
        let spoof = TerminalSafe::sanitize("invoice\u{202E}fdp.exe");
        assert_eq!(spoof.as_str(), "invoicefdp.exe");
        let hidden = TerminalSafe::sanitize("req\u{200B}west\u{00AD}\u{FEFF}\u{E0041}\u{2060}");
        assert_eq!(hidden.as_str(), "reqwest");
    }

    /// Printable neighbours of the denied ranges pass through untouched.
    #[test]
    fn terminal_safe_keeps_printable_neighbours() {
        let kept = "\u{00AC}\u{00AE}\u{200A}\u{2010}\u{FEFC}\u{FFFC}\u{E0100}";
        assert_eq!(TerminalSafe::sanitize(kept).as_str(), kept);
    }
}
