//! The Win32 name rules every output-path check shares.
//!
//! Win32 opens some names as another entry than the one they spell: it strips
//! a trailing `.` or space, reads a `:` as a stream separator
//! (`out::$INDEX_ALLOCATION` opens `out` itself), and maps a DOS device stem
//! to the device whatever its extension. It rejects `<>"|?*`, separators, and
//! control characters outright. These rules are stated here once, for the
//! path proof and the handle-relative opens alike.

/// Device names Windows reserves in every directory, whatever the extension.
const RESERVED_DEVICE_NAMES: [&str; 33] = [
    "CON",
    "PRN",
    "AUX",
    "NUL",
    "CONIN$",
    "CONOUT$",
    "CLOCK$",
    "COM0",
    "COM1",
    "COM2",
    "COM3",
    "COM4",
    "COM5",
    "COM6",
    "COM7",
    "COM8",
    "COM9",
    "COM\u{b9}",
    "COM\u{b2}",
    "COM\u{b3}",
    "LPT0",
    "LPT1",
    "LPT2",
    "LPT3",
    "LPT4",
    "LPT5",
    "LPT6",
    "LPT7",
    "LPT8",
    "LPT9",
    "LPT\u{b9}",
    "LPT\u{b2}",
    "LPT\u{b3}",
];

/// Whether `text` holds a character Win32 refuses in a name or reads as syntax.
#[must_use]
pub fn has_forbidden_char(text: &str) -> bool {
    text.chars().any(|c| {
        c.is_control() || matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
    })
}

/// Whether `text` names a reserved device: its stem before the first inner `.`, trailing spaces trimmed.
#[must_use]
pub fn is_reserved_device_name(text: &str) -> bool {
    let stem = text
        .char_indices()
        .skip(1)
        .find(|&(_, c)| c == '.')
        .and_then(|(at, _)| text.get(..at))
        .unwrap_or(text);
    let stem = stem.trim_end().to_uppercase();
    RESERVED_DEVICE_NAMES.contains(&stem.as_str())
}

/// Whether Win32 opens `text` as exactly the entry it spells.
///
/// Refused: the empty name, a trailing `.` or space, a forbidden character
/// ([`has_forbidden_char`]), and a device name
/// ([`is_reserved_device_name`]). The rule holds under a verbatim (`\\?\`)
/// path too, so a name never reads back as another whichever way it is
/// later opened.
#[must_use]
pub fn opens_as_spelled(text: &str) -> bool {
    !text.is_empty()
        && !text.ends_with(['.', ' '])
        && !has_forbidden_char(text)
        && !is_reserved_device_name(text)
}

#[cfg(test)]
mod tests {
    use super::opens_as_spelled;

    /// Every name Win32 rewrites, maps to a device, or rejects is refused.
    #[test]
    fn a_name_win32_rewrites_is_refused() {
        for name in [
            "",
            ".",
            "..",
            "...",
            "out.",
            "out ",
            "a:b",
            "out::$INDEX_ALLOCATION",
            "nul",
            "NUL",
            "con.txt",
            "Con.tar.gz",
            "aux .txt",
            "prn",
            "COM1",
            "com0",
            "lpt9.log",
            "COM\u{b9}",
            "lpt\u{b3}.x",
            "CONIN$",
            "conout$.txt",
            "a<b",
            "a>b",
            "a|b",
            "a?b",
            "a*b",
            "a\"b",
            "a\\b",
            "a/b",
            "a\u{1}b",
            "a\u{7f}b",
        ] {
            assert!(!opens_as_spelled(name), "{name:?} must be refused");
        }
    }

    /// A name that only resembles a rewritten one opens as spelled.
    #[test]
    fn a_plain_name_is_kept() {
        for name in [
            "out",
            ".hidden",
            ".nul",
            "out.d",
            "nullable",
            "console",
            "console.log",
            "COM10",
            "LPT",
            "comx",
            "a b",
            " lead",
            "x.nul",
            "caf\u{e9}",
        ] {
            assert!(opens_as_spelled(name), "{name:?} must be kept");
        }
    }
}
