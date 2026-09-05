//! The single source of truth for the `ipe` command-line look.
//!
//! It owns the colour palette, the status glyphs, the download spinner and
//! progress bar, and the two fixed strings (the repository URL and the "report
//! bugs" footer) that appear in both the CLI and the shell installer.
//!
//! Every visual fact lives here exactly once. The Rust help renderer, audit,
//! and diff import these constants directly; the shell installer
//! (`install.sh`) cannot import Rust, so it hand-mirrors the same
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
    format!("If you find any bugs, please report them at {REPO_URL}/issues.")
}

/// The product header line, `Ipê language - v{version} - {REPO_URL}`.
///
/// Rendered with no colour; the help renderer inserts the palette escapes around
/// the segments. This is the plain skeleton the header text agrees on.
#[must_use]
pub fn header_line(version: &str) -> String {
    format!("Ipê language - v{version} - {REPO_URL}")
}

/// The left gutter that indents every human-facing line.
///
/// It leads help pages, banners, status chatter, and diagnostics. Two spaces, so
/// prose sits off the terminal edge while machine output (`--plain` / `--json` /
/// a `run` child's stdout) stays flush-left for `grep` / `awk` / `jq`. The single
/// width lives here so every command and the installer indent identically.
pub const GUTTER: &str = "  ";

/// Prefix every non-empty line of `text` with the [`GUTTER`].
///
/// Blank lines stay empty (an indented blank line is trailing whitespace, not
/// structure). This is how a command renders human output: build the plain body,
/// then gutter it once at the edge.
#[must_use]
pub fn gutter(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        // Split off the trailing newline so an empty line stays empty rather
        // than becoming two gutter spaces.
        let (body, newline) = line.strip_suffix('\n').map_or((line, ""), |b| (b, "\n"));
        if !body.is_empty() {
            out.push_str(GUTTER);
        }
        out.push_str(body);
        out.push_str(newline);
    }
    out
}

/// Frame a human block with exactly one leading and one trailing newline.
///
/// A command's output opens and closes with a blank edge — a consistent
/// breathing frame around the guttered prose. Any surrounding newlines in `body`
/// are normalised to one each.
///
/// Machine output (`--plain` / `--json`, a `run` child's stdout) is NEVER framed
/// — it stays flush and unwrapped for `grep` / `jq`, like the [`gutter`] it also
/// skips.
#[must_use]
pub fn frame(body: &str) -> String {
    format!("\n{}\n", body.trim_matches('\n'))
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
    /// A bright/light yellow (ANSI bright yellow), for the `ipe health`
    /// suggested-fix bullets — a stronger amber than [`yellow`](Self::yellow) so
    /// each actionable item stands out from the report above it.
    pub bright_yellow: &'static str,
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
        bright_yellow: "\x1b[93m",
        dim: "\x1b[38;5;244m",
        green: "\x1b[38;5;114m",
        red: "\x1b[31m",
        bold: "\x1b[1m",
        reset: "\x1b[0m",
    };

    /// The plain palette: every escape is empty, yielding aligned plain text.
    pub const PLAIN: Self = Self {
        yellow: "",
        bright_yellow: "",
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

/// The version banner printed at the start of `ipe build`, `ipe run`, and
/// `ipe watch` in human (default) output mode — NOT under `--plain`, `--json`,
/// or `--quiet`.
///
/// Includes a leading blank line, the 2-space gutter, and the URL so it is
/// immediately identifiable in a session. Coloured when `use_color` is true for
/// stderr; plain otherwise.
#[must_use]
pub fn command_header(use_color: bool) -> String {
    let version = env!("CARGO_PKG_VERSION");
    let p = Palette::select(use_color);
    format!(
        "\n{GUTTER}{}{}{} - v{} - {}{}{}\n",
        p.yellow, "Ipê language", p.reset, version, p.dim, REPO_URL, p.reset,
    )
}

/// Print the version banner to stderr, respecting the terminal / `NO_COLOR`
/// state of stderr. Called at the start of human-mode commands.
pub fn print_command_header() {
    let colored = use_color(&std::io::stderr());
    eprint!("{}", command_header(colored));
}

/// Text that has been proven safe to write to a terminal.
///
/// No ANSI escape sequences, no C0/C1 control bytes, no `DEL`, only printable
/// characters plus the two layout whitespaces (`\n`, `\t`) the gutter and
/// terminal handle safely.
///
/// The error sink writes an arbitrary error message — a filename, a compiler
/// excerpt, any embedded text — to stderr. On a terminal, a crafted message
/// laced with ANSI escapes or control bytes could move the cursor, recolour or
/// erase lines, or hide text, turning a diagnostic into a spoofing/injection
/// surface. `TerminalSafe` is the typed boundary: construct it ONCE from the
/// untrusted string, and every downstream renderer takes a `TerminalSafe`
/// rather than a bare `&str`, so the unsanitised form is unrepresentable past
/// the sink. Parse, don't validate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalSafe(String);

impl TerminalSafe {
    /// Sanitise `raw` into terminal-safe text: drop every ANSI escape sequence
    /// (a lone `ESC`, or a CSI `ESC [ … final`) whole, and drop every remaining
    /// control byte below `0x20` and the `DEL` (`0x7f`), keeping only `\n` and
    /// `\t` — the whitespace the gutter and line layout rely on. Printable text
    /// passes through untouched.
    #[must_use]
    pub fn sanitize(raw: &str) -> Self {
        let mut out = String::with_capacity(raw.len());
        let mut chars = raw.chars();
        while let Some(c) = chars.next() {
            if c == '\u{1b}' {
                // An escape introduces a control sequence. A CSI (`ESC [`) runs
                // until a final byte in 0x40..=0x7e; any other escape consumes
                // just its single following byte. Either way the escape and its
                // sequence are dropped whole.
                if chars.clone().next() == Some('[') {
                    chars.next();
                    for seq in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&seq) {
                            break;
                        }
                    }
                } else {
                    chars.next();
                }
                continue;
            }
            // Keep the two layout whitespaces and any printable character; drop
            // every other control byte (C0 below 0x20, and DEL 0x7f).
            if c == '\n' || c == '\t' || !c.is_control() {
                out.push(c);
            }
        }
        Self(out)
    }

    /// The sanitised text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for TerminalSafe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The short-diagnostic error banner: a soft-yellow block that leads with the
/// product name and version, then the error message, both guttered and framed.
///
/// A failed command is not an alarm — the last-good state (a prior build, the
/// running app) usually survives — so the banner is a calm soft yellow, never
/// red. No `ipe: ` prefix: the product-name header identifies the source, so the
/// prefix only added noise that scrolled away with the message. Coloured when
/// `color` is on, plain (empty escapes) otherwise, so one shape serves both.
///
/// The message is already-sanitised [`TerminalSafe`]: the banner's own escapes
/// (yellow/reset) are the only control bytes the output may carry.
#[must_use]
pub fn error_banner(message: &TerminalSafe, color: bool) -> String {
    let version = env!("CARGO_PKG_VERSION");
    let p = Palette::select(color);
    let body = format!(
        "{y}Ipê lang - {version}{r}\n\n{y}{message}{r}",
        y = p.yellow,
        r = p.reset,
    );
    frame(&gutter(&body))
}

/// Print [`error_banner`] to stderr, respecting the terminal / `NO_COLOR` state
/// of stderr. The one place short CLI diagnostics render their final screen.
///
/// The untrusted message is parsed into [`TerminalSafe`] HERE, once, at the
/// boundary — every deeper renderer sees only the sanitised form.
pub fn print_error_banner(message: &str) {
    let colored = use_color(&std::io::stderr());
    let safe = TerminalSafe::sanitize(message);
    eprint!("{}", error_banner(&safe, colored));
}

/// A framed, guttered status line: a leading success/failure glyph, then the
/// message. `ok` picks the green check or the red cross; `color` toggles ANSI.
///
/// The one way a human-mode command reports a completed step — replacing a bare
/// left-flush spinner line with `<glyph> <message>` plus the 2-space gutter.
#[must_use]
pub fn status_line(ok: bool, message: &str, color: bool) -> String {
    let p = Palette::select(color);
    let (glyph, tint) = if ok {
        (glyph::OK, p.green)
    } else {
        (glyph::FAIL, p.red)
    };
    frame(&gutter(&format!("{tint}{glyph}{} {message}", p.reset)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_palette_carries_ansi_plain_does_not() {
        assert!(Palette::COLOR.yellow.contains('\x1b'));
        assert!(Palette::PLAIN.yellow.is_empty());
        // The bright-yellow field the health fix bullets use follows the same
        // rule: an escape under colour, empty under plain.
        assert!(Palette::COLOR.bright_yellow.contains('\x1b'));
        assert!(Palette::PLAIN.bright_yellow.is_empty());
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
    fn command_header_has_leading_newline_gutter_version_and_url() {
        let h = command_header(false);
        // Starts with a blank line so the banner breathes at the top of output.
        assert!(h.starts_with('\n'), "banner has leading newline");
        // The 2-space gutter is present.
        assert!(h.contains(GUTTER), "banner has gutter");
        // The runtime version is embedded dynamically.
        assert!(
            h.contains(env!("CARGO_PKG_VERSION")),
            "banner carries crate version"
        );
        // The canonical project URL is present.
        assert!(h.contains(REPO_URL), "banner carries repo URL");
        // The plain variant has no ANSI escapes.
        assert!(!h.contains('\x1b'), "plain banner has no ANSI");
    }

    #[test]
    fn command_header_colour_mode_has_ansi() {
        let h = command_header(true);
        assert!(h.contains('\x1b'), "colour banner carries ANSI");
    }

    #[test]
    fn error_banner_leads_with_versioned_product_name_then_message() {
        let b = error_banner(&TerminalSafe::sanitize("no such file `Main.ipe`"), false);
        // Framed: one blank edge each side.
        assert!(b.starts_with('\n') && b.ends_with('\n'), "framed banner");
        // No `ipe: ` prefix survives.
        assert!(!b.contains("ipe: "), "the ipe: prefix is gone");
        // The product name and crate version lead.
        assert!(b.contains("Ipê lang - "), "product name present");
        assert!(
            b.contains(env!("CARGO_PKG_VERSION")),
            "crate version present"
        );
        // The message follows, guttered.
        assert!(b.contains("  no such file `Main.ipe`"), "message present");
        // Header sits above the message.
        let header_at = b.find("Ipê lang").expect("header present");
        let msg_at = b.find("no such file").expect("message present");
        assert!(header_at < msg_at, "header above message");
        // Plain variant carries no ANSI.
        assert!(!b.contains('\x1b'), "plain banner has no ANSI");
    }

    #[test]
    fn error_banner_colour_mode_is_soft_yellow_not_red() {
        let b = error_banner(&TerminalSafe::sanitize("boom"), true);
        assert!(b.contains(Palette::COLOR.yellow), "soft yellow applied");
        assert!(!b.contains(Palette::COLOR.red), "never the alarming red");
    }

    /// A hostile error message laced with ANSI escapes and control bytes is
    /// stripped to printable text plus layout whitespace: the banner it renders
    /// carries no escape the message injected, so a crafted filename or compiler
    /// excerpt cannot rewrite the terminal.
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
        // The CSI colour codes are removed whole, leaving only the visible text;
        // layout whitespace (tab, newline) is preserved.
        assert_eq!(s, "redmoveddone\ttab\nline");
    }

    /// The escaped banner is safe end to end: sanitising the message before it
    /// reaches [`error_banner`] means the only ANSI in the rendered output is the
    /// banner's own soft-yellow framing, never a byte the message smuggled in.
    #[test]
    fn error_banner_message_cannot_inject_ansi_in_plain_mode() {
        let hostile = "boom\u{1b}[2J\u{1b}[1;1H";
        let b = error_banner(&TerminalSafe::sanitize(hostile), false);
        assert!(!b.contains('\x1b'), "plain banner carries no ANSI: {b:?}");
        assert!(b.contains("boom"), "the visible text survives");
    }

    #[test]
    fn gutter_indents_prose_lines_and_leaves_blanks_empty() {
        let g = gutter("one\n\ntwo\n");
        assert_eq!(g, "  one\n\n  two\n");
        // A body with no trailing newline is still guttered.
        assert_eq!(gutter("tail"), "  tail");
        // The gutter is exactly the two-space SSOT width.
        assert_eq!(GUTTER, "  ");
    }

    #[test]
    fn frame_wraps_a_block_in_one_leading_and_trailing_newline() {
        // A bare body gains exactly one newline on each edge.
        assert_eq!(frame("  body"), "\n  body\n");
        // Surrounding newlines are normalised to one each (never doubled).
        assert_eq!(frame("\n\n  page\n\n"), "\n  page\n");
        // Interior blank lines are preserved.
        assert_eq!(frame("a\n\nb"), "\na\n\nb\n");
    }

    #[test]
    fn spinner_has_ten_frames_and_bar_chars_differ() {
        assert_eq!(SPINNER_FRAMES.len(), 10);
        // Every frame is a distinct non-empty glyph, and the filled/empty bar
        // cells are different characters so the bar reads.
        assert!(SPINNER_FRAMES.iter().all(|f| !f.is_empty()));
        assert_ne!(progress_bar::FILLED, progress_bar::EMPTY);
    }
}
