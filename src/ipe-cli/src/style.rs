//! The single source of truth for the `ipe` command-line look: the colour
//! palette, the status glyphs, the download spinner and progress bar, and the
//! two fixed strings (the repository URL and the "report bugs" footer) that
//! appear in both the CLI and the shell installer.
//!
//! Every visual fact lives here exactly once. The Rust help renderer, audit,
//! and diff import these constants directly; the shell installer
//! (`scripts/install.sh`) cannot import Rust, so it hand-mirrors the same
//! values and a drift test (`tests/install_style_drift.rs`) asserts the two
//! agree — an unmirrored change to either side fails CI.
//!
//! Colour is opt-in per output stream: ANSI escapes are emitted only when the
//! destination is a terminal and `NO_COLOR` is unset (per
//! <https://no-color.org>). Piped, redirected, or `NO_COLOR` output resolves to
//! the plain palette, whose every escape is the empty string, so one format
//! string yields either coloured or clean plain text.

use std::io::IsTerminal;

/// The repository home, shown in the CLI header and the "report bugs" footer,
/// and mirrored by the installer.
pub const REPO_URL: &str = "https://github.com/arthurmaciel/ipe-lang";

/// The "report bugs" footer phrase, ending in `{REPO_URL}/issues.`. One
/// call site formats it; the installer mirrors the rendered text.
#[must_use]
pub fn report_bugs_footer() -> String {
    format!("Found any bugs? Please report them at {REPO_URL}/issues.")
}

/// The product header line, `Ipê language - v{version} - {REPO_URL}` with no
/// colour. The help renderer inserts the palette escapes around the segments;
/// this is the plain skeleton the header text agrees on.
#[must_use]
pub fn header_line(version: &str) -> String {
    format!("Ipê language - v{version} - {REPO_URL}")
}

/// The status glyphs that lead a line of progress chatter: a step bullet, a
/// success check, and a failure cross. Shared by the CLI and the installer.
pub mod glyph {
    /// A step in progress (leads an in-flight status line).
    pub const STEP: &str = "•";
    /// A completed step (leads a success line).
    pub const OK: &str = "✓";
    /// A failed step (leads a failure line).
    pub const FAIL: &str = "✗";
}

/// The braille download spinner, one frame per animation tick. The installer
/// mirrors these ten frames in its `spin_glyph` case.
pub const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// The progress-bar geometry: the total cell count and the filled/empty cell
/// characters. The installer mirrors these when it renders the download bar.
pub mod progress_bar {
    /// The bar's width in cells.
    pub const WIDTH: usize = 24;
    /// The character for a filled cell.
    pub const FILLED: char = '#';
    /// The character for an empty cell.
    pub const EMPTY: char = '-';
}

/// The ANSI palette, resolved once against a destination stream. When colour is
/// off every field is the empty string, so the same format code produces clean
/// plain text.
pub struct Palette {
    /// A soft Ipê-amarelo (256-colour 222), for the product name and command
    /// names.
    pub yellow: &'static str,
    /// A mid grey (256-colour 244), for descriptions and the footer.
    pub dim: &'static str,
    /// A soft green (256-colour 114), for success emphasis.
    pub green: &'static str,
    /// A plain red, for failure emphasis.
    pub red: &'static str,
    /// A bold weight, for section titles and headers.
    pub bold: &'static str,
    /// Resets all attributes.
    pub reset: &'static str,
}

impl Palette {
    /// The coloured palette: a soft Ipê-amarelo (222) for names, a mid grey
    /// (244) for dim text, a soft green (114) for success, plain red for
    /// failure.
    pub const COLOR: Self = Self {
        yellow: "\x1b[38;5;222m",
        dim: "\x1b[38;5;244m",
        green: "\x1b[38;5;114m",
        red: "\x1b[31m",
        bold: "\x1b[1m",
        reset: "\x1b[0m",
    };

    /// The plain palette: every escape is empty, yielding aligned plain text.
    pub const PLAIN: Self = Self {
        yellow: "",
        dim: "",
        green: "",
        red: "",
        bold: "",
        reset: "",
    };

    /// Select the coloured palette when `color` is on, else the plain one.
    #[must_use]
    pub const fn select(color: bool) -> &'static Self {
        if color { &Self::COLOR } else { &Self::PLAIN }
    }

    /// Select the palette for `stream`: coloured only when it is a terminal and
    /// `NO_COLOR` is unset (per <https://no-color.org>).
    #[must_use]
    pub fn for_stream(stream: &impl IsTerminal) -> &'static Self {
        Self::select(use_color(stream))
    }
}

/// Whether to emit ANSI to `stream`: only when it is a terminal and `NO_COLOR`
/// is unset (per <https://no-color.org>).
#[must_use]
pub fn use_color(stream: &impl IsTerminal) -> bool {
    stream.is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_palette_carries_ansi_plain_does_not() {
        assert!(Palette::COLOR.yellow.contains('\x1b'));
        assert!(Palette::PLAIN.yellow.is_empty());
        // `select` returns the coloured palette on true, the plain one on false.
        assert!(Palette::select(true).red.contains('\x1b'));
        assert!(Palette::select(false).red.is_empty());
    }

    #[test]
    fn footer_and_header_carry_the_repo_url() {
        assert!(report_bugs_footer().contains(REPO_URL));
        assert!(report_bugs_footer().ends_with("/issues."));
        assert!(header_line("9.9.9").contains(REPO_URL));
        assert!(header_line("9.9.9").contains("v9.9.9"));
    }

    #[test]
    fn spinner_has_ten_frames_and_bar_is_nonempty() {
        assert_eq!(SPINNER_FRAMES.len(), 10);
        assert!(progress_bar::WIDTH > 0);
    }
}
