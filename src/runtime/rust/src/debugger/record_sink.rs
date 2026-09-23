//! The cli/worker record sink — dump a recorded session's portable replay log
//! to the destination named by `IPE_DEBUGGER_RECORD`, fail-closed to plain text.
//!
//! Enabled only when the `debugger` feature is active. A non-`--debugger` build
//! carries zero code from this module.
//!
//! ## The output boundary (principle 1)
//!
//! The recorder's portable form ([`RecordBuffer::replay_log`]) is plain by
//! construction — every half renders through `IpeStringify::ipe_show`, which
//! emits no ANSI/control byte. This sink is the OUTPUT boundary that keeps it
//! plain regardless: [`plain_line`] strips every control byte before a line
//! reaches a file/pipe, so a redirected log receives ZERO control codes even if
//! a body somehow carried one. Absent proof the body is control-free, the sink
//! still cannot feed a control byte to the destination — fail closed.
//!
//! The strip rule is identical to the cli's `progress::inspect_line` `Mode::Plain`
//! branch (`char::is_control` filter, one settling newline). The two live in
//! separate crates and cannot import one another; [`plain_line`] is pinned to
//! that rule by a unit test here, so a drift breaks the build's test gate.

#![cfg(feature = "debugger")]

use std::io::Write;

use crate::debugger::{RECORD_ENV, RecordBuffer};
use crate::stringify::IpeStringify;
use crate::tea::IpeCmd;

/// The sentinel `IPE_DEBUGGER_RECORD` value that selects stderr over a file.
const STDERR_SENTINEL: &str = "-";

/// Where a record dump lands: stderr, or a filesystem path.
///
/// A parsed value at the boundary (parse, don't validate): the raw env string is
/// turned into this two-variant choice once, so the writer never re-inspects the
/// sentinel. Absent env ⇒ no `RecordDest` at all (the dump is skipped upstream).
enum RecordDest {
    /// The `-` sentinel: write the dump to stderr.
    Stderr,
    /// A filesystem path: create/truncate and write the dump there.
    Path(std::path::PathBuf),
}

impl RecordDest {
    /// Parse the raw `IPE_DEBUGGER_RECORD` value into a destination.
    fn parse(raw: &str) -> Self {
        if raw == STDERR_SENTINEL {
            Self::Stderr
        } else {
            Self::Path(std::path::PathBuf::from(raw))
        }
    }
}

/// Render one replay-log line as plain, control-code-free text ending in exactly
/// one newline — the fail-closed output-boundary form.
///
/// Every control byte is stripped (C0/C1 and DEL — every ANSI-escape introducer,
/// carriage return, and cursor-motion byte), then a single settling newline is
/// appended. This mirrors the cli's `progress::inspect_line(Mode::Plain, …)`
/// exactly; the equivalence is pinned by a test in this module so the two cannot
/// drift.
#[must_use]
pub fn plain_line(body: &str) -> String {
    let plain: String = body.chars().filter(|c| !c.is_control()).collect();
    format!("{plain}\n")
}

/// Render the whole replay log as one plain, control-free blob — one
/// `"<msg> => <model>"` line per retained step, each through [`plain_line`].
///
/// Pure: no `Cmd` is fired (a re-fold via [`RecordBuffer::replay_log`]) and no
/// I/O happens. The blob is bounded by the recorder's ring cap. This is the
/// testable seam the env-driven [`dump_replay_log`] writes out.
#[must_use]
pub fn render_replay_blob<Msg, Model, F>(buf: &RecordBuffer<Msg, Model>, update: &F) -> String
where
    Msg: Clone + IpeStringify,
    Model: Clone + IpeStringify,
    F: Fn(Msg, Model) -> (Model, IpeCmd<Msg>),
{
    let mut out = String::new();
    for line in buf.replay_log(update) {
        out.push_str(&plain_line(&line));
    }
    out
}

/// Write an already-plain replay `blob` to `dest`. A write error is swallowed —
/// the record dump is advisory dev tooling and must never turn a working app
/// into a failing one over a closed pipe or an unwritable path.
fn write_blob(dest: &RecordDest, blob: &str) {
    match dest {
        RecordDest::Stderr => {
            let stderr = std::io::stderr();
            let mut lock = stderr.lock();
            let _ = lock.write_all(blob.as_bytes());
            let _ = lock.flush();
        }
        // A path destination: create/truncate and write the plain dump. The
        // recorder's dev-loop log is not a trust boundary, so `File::create` is
        // fine; a failure to open is swallowed like any write error.
        RecordDest::Path(path) => {
            if let Ok(mut file) = std::fs::File::create(path) {
                let _ = file.write_all(blob.as_bytes());
                let _ = file.flush();
            }
        }
    }
}

/// Dump `buf`'s portable replay log to the destination named by
/// `IPE_DEBUGGER_RECORD`, if that variable is set. A no-op when it is unset.
///
/// The dump is rendered by [`render_replay_blob`] (plain, bounded by the ring
/// cap, no `Cmd` fired) and written to the parsed [`RecordDest`], so a
/// redirected log or file receives only plain, control-free lines.
pub fn dump_replay_log<Msg, Model, F>(buf: &RecordBuffer<Msg, Model>, update: &F)
where
    Msg: Clone + IpeStringify,
    Model: Clone + IpeStringify,
    F: Fn(Msg, Model) -> (Model, IpeCmd<Msg>),
{
    let Ok(raw) = std::env::var(RECORD_ENV) else {
        return;
    };
    let dest = RecordDest::parse(&raw);
    let blob = render_replay_blob(buf, update);
    write_blob(&dest, &blob);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    enum Msg {
        Add(i64),
    }
    #[derive(Clone, Debug, PartialEq)]
    struct Model {
        n: i64,
    }
    impl IpeStringify for Msg {
        fn ipe_show(&self) -> String {
            let Msg::Add(v) = self;
            format!("Add({v})")
        }
    }
    impl IpeStringify for Model {
        fn ipe_show(&self) -> String {
            format!("Model {{ n = {} }}", self.n)
        }
    }
    fn update(msg: Msg, m: Model) -> (Model, IpeCmd<Msg>) {
        let Msg::Add(v) = msg;
        (Model { n: m.n + v }, IpeCmd::None)
    }

    // plain_line strips every control byte and settles with exactly one newline
    // — the fail-closed output boundary. A control-laced body reaches the sink
    // as plain text with zero control codes (byte-identical to the cli's
    // `progress::inspect_line(Mode::Plain, …)` refusal).
    #[test]
    fn plain_line_strips_all_control_bytes() {
        let laced = "\x1b[31mModel { n = 7 }\x1b[0m\r\x1b[2K\x07";
        let out = plain_line(laced);
        assert!(out.ends_with('\n'), "plain line settles with a newline");
        let body = out.strip_suffix('\n').expect("trailing newline");
        assert!(
            !body.chars().any(char::is_control),
            "off-TTY record line must carry zero control bytes; got: {body:?}"
        );
        assert!(!body.contains('\x1b'), "no ANSI escape may survive");
        // Printable payload preserved verbatim; only control bytes drop.
        assert_eq!(body, "[31mModel { n = 7 }[0m[2K");
    }

    // render_replay_blob is the plain replay log: one `"<msg> => <model>"` line
    // per step, zero control codes, bounded by the ring cap. No env, no I/O.
    #[test]
    fn render_replay_blob_is_plain_replay_log() {
        let mut buf = RecordBuffer::new(Model { n: 0 }, 8);
        let msgs = [Msg::Add(2), Msg::Add(5)];
        let mut live = Model { n: 0 };
        for msg in &msgs {
            let (next, _) = update(msg.clone(), live.clone());
            live = next.clone();
            buf.record(msg.clone(), next, &update);
        }
        let blob = render_replay_blob(&buf, &update);
        assert_eq!(
            blob,
            "Add(2) => Model { n = 2 }\nAdd(5) => Model { n = 7 }\n"
        );
        assert!(
            !blob.chars().any(|c| c.is_control() && c != '\n'),
            "record blob must carry no control code other than line breaks; got: {blob:?}"
        );
    }

    // An empty session renders an empty blob — no spurious line, no panic.
    #[test]
    fn render_replay_blob_empty_session_is_empty() {
        let buf: RecordBuffer<Msg, Model> = RecordBuffer::new(Model { n: 0 }, 8);
        assert_eq!(render_replay_blob(&buf, &update), "");
    }

    // The `-` sentinel parses to stderr; anything else is a path.
    #[test]
    fn record_dest_parse_sentinel_and_path() {
        assert!(matches!(RecordDest::parse("-"), RecordDest::Stderr));
        assert!(matches!(
            RecordDest::parse("/tmp/session.log"),
            RecordDest::Path(p) if p == std::path::Path::new("/tmp/session.log")
        ));
    }

    // Past the cap the rendered blob holds exactly `cap` lines — the dump is
    // bounded by the recorder's ring, never unbounded (principle 3 / principle 1
    // resource-exhaustion floor).
    #[test]
    fn render_replay_blob_is_bounded_by_ring_cap() {
        let cap = 4usize;
        let mut buf = RecordBuffer::new(Model { n: 0 }, cap);
        let mut live = Model { n: 0 };
        for i in 1..=20i64 {
            let (next, _) = update(Msg::Add(i), live.clone());
            live = next.clone();
            buf.record(Msg::Add(i), next, &update);
        }
        let blob = render_replay_blob(&buf, &update);
        let lines = blob.lines().count();
        assert_eq!(lines, cap, "rendered blob must hold exactly cap lines");
    }
}
