//! The one renderer for human-facing CLI output.
//!
//! Every human screen has the same frame:
//!
//! ```text
//!
//!   Ipê language - vN.N.N - https://github.com/arthurmaciel/ipe-lang
//!   <content, indented by the two-space gutter>
//!
//! ```
//!
//! The header ([`crate::style::command_header`]) opens a process's human output
//! once; a later screen in the same run carries only its content. An error
//! screen always closes with the "report bugs" footer.
//!
//! A line's colour is its semantic [`Tone`], never a raw palette field picked at
//! the call site: light green for success, light red for an ipe-internal error,
//! light orange for a user error, soft white for common text, dim gray for
//! auxiliary text. Colour follows the destination stream (a terminal with
//! `NO_COLOR` unset); piped or `NO_COLOR` output is the same frame in plain
//! text.
//!
//! Machine output (`--json`, `--plain`, a `run` child's stdout) is never framed:
//! it goes through [`emit_machine`] byte for byte.

use std::fmt::Write as _;
use std::io::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::CliError;
use crate::style::{self, GUTTER, Palette, REPORT_BUGS_PHRASE, TerminalSafe};

/// The semantic role of a piece of human output, which fixes its colour.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tone {
    /// A successful outcome — light green.
    Success,
    /// A failure inside ipe itself (a bug to report) — light red.
    InternalError,
    /// A failure the user can fix (misuse, a source error, the environment) —
    /// light orange.
    UserError,
    /// Common prose — soft white.
    Text,
    /// Auxiliary detail (hints, locations, URLs) — dim gray.
    Aux,
}

impl Tone {
    /// The escape that paints this tone under `p` (empty under the plain
    /// palette).
    #[must_use]
    pub const fn ink(self, p: &Palette) -> &'static str {
        match self {
            Self::Success => p.green,
            Self::InternalError => p.light_red,
            Self::UserError => p.orange,
            Self::Text => p.white,
            Self::Aux => p.dim,
        }
    }
}

/// Who a failed command's error belongs to — it picks the error [`Tone`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fault {
    /// The user can act on it: misuse, a source error, a missing prerequisite.
    User,
    /// ipe itself broke an invariant it promises (a bug to report).
    Internal,
}

impl Fault {
    /// The tone an error of this fault is painted in.
    #[must_use]
    pub const fn tone(self) -> Tone {
        match self {
            Self::User => Tone::UserError,
            Self::Internal => Tone::InternalError,
        }
    }
}

/// The destination stream of a screen: it decides the colour and receives the
/// bytes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stream {
    /// Standard output — requested output (help, reports, documentation).
    Stdout,
    /// Standard error — progress chatter and errors.
    Stderr,
}

impl Stream {
    /// Whether ANSI colour reaches this stream (a terminal, `NO_COLOR` unset).
    fn color(self) -> bool {
        match self {
            Self::Stdout => style::use_color(&std::io::stdout()),
            Self::Stderr => style::use_color(&std::io::stderr()),
        }
    }

    /// Write `text` whole and flush. Best effort: a closed stream (a reader
    /// that went away) drops the bytes rather than aborting the process.
    fn write(self, text: &str) {
        let _ = match self {
            Self::Stdout => {
                let mut out = std::io::stdout().lock();
                out.write_all(text.as_bytes()).and_then(|()| out.flush())
            }
            Self::Stderr => {
                let mut err = std::io::stderr().lock();
                err.write_all(text.as_bytes()).and_then(|()| err.flush())
            }
        };
    }
}

/// Whether a rendered screen opens with the product header.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Header {
    /// The first human output of the process: the header leads.
    Shown,
    /// The header already opened this process's output.
    Omitted,
}

/// Set once the header has been written, so a process shows it exactly once.
static HEADER_WRITTEN: AtomicBool = AtomicBool::new(false);

/// Claim the header for the next screen: [`Header::Shown`] the first time,
/// [`Header::Omitted`] after.
fn claim_header() -> Header {
    if HEADER_WRITTEN.swap(true, Ordering::Relaxed) {
        Header::Omitted
    } else {
        Header::Shown
    }
}

/// Write the product header alone to `stream`, unless it was already written.
/// Leads a command's progress chatter before any screen exists.
pub fn emit_header(stream: Stream) {
    if claim_header() == Header::Shown {
        stream.write(&style::command_header(stream.color()));
    }
}

/// Write machine output (`--json`, `--plain`) to `stream` byte for byte —
/// never framed, never coloured.
pub fn emit_machine(stream: Stream, text: &str) {
    stream.write(text);
}

/// A human screen under construction: guttered, toned lines bound to one
/// stream's palette.
pub struct Screen {
    /// Where the screen is written.
    stream: Stream,
    /// The palette resolved for [`Self::stream`].
    palette: &'static Palette,
    /// The body so far: every non-empty line already guttered, each ended by a
    /// newline.
    body: String,
    /// Whether the screen closes with the bug footer (always, for an error).
    bug_footer: bool,
}

impl Screen {
    /// A screen for `stream`, coloured when that stream takes colour.
    #[must_use]
    pub fn new(stream: Stream) -> Self {
        Self::with_color(stream, stream.color())
    }

    /// A screen for `stream` with colour forced on or off (deterministic
    /// rendering for tests).
    #[must_use]
    pub const fn with_color(stream: Stream, color: bool) -> Self {
        Self {
            stream,
            palette: Palette::select(color),
            body: String::new(),
            bug_footer: false,
        }
    }

    /// The palette this screen paints with, for trusted renderers that style a
    /// block before handing it to [`Self::styled`].
    #[must_use]
    pub const fn palette(&self) -> &'static Palette {
        self.palette
    }

    /// Append `text` painted in `tone`, one guttered line per text line. The
    /// text may be untrusted: it is sanitised to [`TerminalSafe`] first, so the
    /// tone's own escapes are the only control bytes it can carry.
    pub fn line(&mut self, tone: Tone, text: &str) -> &mut Self {
        let safe = TerminalSafe::sanitize(text);
        let ink = tone.ink(self.palette);
        let reset = if ink.is_empty() {
            ""
        } else {
            self.palette.reset
        };
        for line in safe.as_str().trim_matches('\n').split('\n') {
            if !line.is_empty() {
                self.body.push_str(GUTTER);
                self.body.push_str(ink);
                self.body.push_str(line);
                self.body.push_str(reset);
            }
            self.body.push('\n');
        }
        self
    }

    /// Append an empty separator line.
    pub fn blank(&mut self) -> &mut Self {
        self.body.push('\n');
        self
    }

    /// Append a block a trusted renderer already styled with [`Self::palette`]
    /// (a help page, a finding). Every line gains the gutter; edge newlines are
    /// dropped.
    pub fn styled(&mut self, block: &str) -> &mut Self {
        let block = block.trim_matches('\n');
        if !block.is_empty() {
            self.body.push_str(&style::gutter(block));
            self.body.push('\n');
        }
        self
    }

    /// Append a block whose renderer guttered some or all of its own lines
    /// (a self-rendering error). A line that already starts with the gutter is
    /// kept as is; any other non-empty line gains it, so no line reaches the
    /// terminal edge and none is indented twice.
    pub fn guttered(&mut self, block: &str) -> &mut Self {
        for line in block.trim_matches('\n').split('\n') {
            if !line.is_empty() && !line.starts_with(GUTTER) {
                self.body.push_str(GUTTER);
            }
            self.body.push_str(line);
            self.body.push('\n');
        }
        self
    }

    /// Mark the screen as an error report: it closes with the bug footer.
    pub const fn as_error(&mut self) -> &mut Self {
        self.bug_footer = true;
        self
    }

    /// Close a non-error screen with the bug footer too (the top-level help
    /// overview, where a newcomer looks for where to report problems).
    pub const fn with_bug_footer(&mut self) -> &mut Self {
        self.bug_footer = true;
        self
    }

    /// Render the screen as text, with or without the header.
    #[must_use]
    pub fn render(&self, header: Header) -> String {
        let p = self.palette;
        let mut out = match header {
            Header::Shown => style::command_header(!p.reset.is_empty()),
            Header::Omitted => String::from("\n"),
        };
        out.push_str(self.body.trim_end_matches('\n'));
        out.push('\n');
        if self.bug_footer {
            let text = Tone::Text.ink(p);
            let aux = Tone::Aux.ink(p);
            let r = p.reset;
            let _ = write!(
                out,
                "\n{GUTTER}{text}{REPORT_BUGS_PHRASE}{r}{aux}{}{r}{text}.{r}\n",
                style::issues_url()
            );
        }
        out
    }

    /// Write the screen to its stream, with the header when it is the process's
    /// first human output.
    pub fn emit(&self) {
        self.stream.write(&self.render(claim_header()));
    }
}

/// Report a failed command on stderr in the one error frame: the header (when
/// not yet shown), the error in its [`Fault`]'s tone — or, for an error that
/// renders its own complete screen, that screen — then the bug footer.
///
/// An error that already wrote its final output (a machine-mode envelope, an
/// upgrade verdict) renders nothing here.
pub fn report_error(err: &CliError) {
    if let Some(screen) = error_screen(err, Stream::Stderr.color()) {
        screen.emit();
    }
}

/// Build the error screen for `err`, or `None` when the error already wrote its
/// final output.
#[must_use]
pub fn error_screen(err: &CliError, color: bool) -> Option<Screen> {
    let text = err.to_string();
    if text.trim().is_empty() {
        return None;
    }
    let mut screen = Screen::with_color(Stream::Stderr, color);
    if err.renders_own_screen() {
        // A self-rendering error (a help page, a gate report) is styled by
        // ipe's own renderers with the stderr palette, so its escapes pass
        // through; only the frame is added.
        screen.guttered(&text);
    } else {
        screen.line(err.fault().tone(), &text);
    }
    screen.as_error();
    Some(screen)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(stream: Stream) -> Screen {
        Screen::with_color(stream, false)
    }

    #[test]
    fn frame_is_header_then_guttered_content() {
        let mut s = plain(Stream::Stdout);
        s.line(Tone::Text, "hello\nworld");
        let version = env!("CARGO_PKG_VERSION");
        assert_eq!(
            s.render(Header::Shown),
            format!(
                "\n  Ipê language - v{version} - {}\n  hello\n  world\n",
                style::REPO_URL
            )
        );
        assert_eq!(s.render(Header::Omitted), "\n  hello\n  world\n");
    }

    #[test]
    fn error_screen_closes_with_the_bug_footer() {
        let mut s = plain(Stream::Stderr);
        s.line(Tone::UserError, "boom").as_error();
        let out = s.render(Header::Omitted);
        assert!(
            out.ends_with(&format!(
                "  boom\n\n  {REPORT_BUGS_PHRASE}{}.\n",
                style::issues_url()
            )),
            "{out:?}"
        );
    }

    #[test]
    fn plain_screen_carries_no_ansi() {
        let mut s = plain(Stream::Stderr);
        s.line(Tone::InternalError, "x")
            .line(Tone::Success, "y")
            .line(Tone::Aux, "z")
            .as_error();
        let out = s.render(Header::Shown);
        assert!(!out.contains('\x1b'), "{out:?}");
    }

    #[test]
    fn each_tone_paints_its_palette_role() {
        let c = &Palette::COLOR;
        assert_eq!(Tone::Success.ink(c), c.green);
        assert_eq!(Tone::InternalError.ink(c), c.light_red);
        assert_eq!(Tone::UserError.ink(c), c.orange);
        assert_eq!(Tone::Text.ink(c), c.white);
        assert_eq!(Tone::Aux.ink(c), c.dim);
        assert_eq!(Fault::User.tone(), Tone::UserError);
        assert_eq!(Fault::Internal.tone(), Tone::InternalError);
        let mut s = Screen::with_color(Stream::Stderr, true);
        s.line(Tone::UserError, "bad");
        let out = s.render(Header::Omitted);
        assert!(
            out.contains(&format!("  {}bad{}", c.orange, c.reset)),
            "{out:?}"
        );
    }

    #[test]
    fn untrusted_line_text_cannot_inject_escapes() {
        let mut s = plain(Stream::Stderr);
        s.line(Tone::Text, "boom\u{1b}[2J\u{1b}[1;1H");
        let out = s.render(Header::Omitted);
        assert!(!out.contains('\x1b'), "{out:?}");
        assert!(out.contains("  boom"), "{out:?}");
    }

    #[test]
    fn guttered_block_is_indented_exactly_once() {
        let mut s = plain(Stream::Stderr);
        s.guttered("  already\nbare\n    nested");
        assert_eq!(
            s.render(Header::Omitted),
            "\n  already\n  bare\n    nested\n"
        );
    }

    #[test]
    fn styled_block_gains_the_gutter_and_keeps_blank_lines_empty() {
        let mut s = plain(Stream::Stdout);
        s.styled("\ntitle\n\nbody\n");
        assert_eq!(s.render(Header::Omitted), "\n  title\n\n  body\n");
    }

    #[test]
    fn a_user_error_is_orange_and_an_internal_error_light_red() {
        let usage = CliError::Usage("nothing to build here");
        let out = error_screen(&usage, true)
            .map(|s| s.render(Header::Omitted))
            .unwrap_or_default();
        assert!(out.contains(Palette::COLOR.orange), "{out:?}");
        assert!(!out.contains(Palette::COLOR.light_red), "{out:?}");
        assert!(out.contains(REPORT_BUGS_PHRASE), "{out:?}");
    }

    #[test]
    fn an_error_that_already_wrote_its_output_renders_nothing() {
        assert!(error_screen(&CliError::DiagnosticJsonEmitted, false).is_none());
    }
}
