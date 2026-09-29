//! The cli/worker record sink — dump a recorded session's portable replay log
//! to the destination named by `IPE_DEBUGGER_RECORD`, fail-closed to plain text.
//!
//! Beside a file destination it also writes the session's typed log (same
//! stem, extension [`crate::TYPED_LOG_EXTENSION`]) through the program's
//! [`SessionCodec`] — the replayable form `ipe run --replay` reads. A program
//! with no typed log (see [`crate::debugger::session_log`]) gets the trace
//! only, and any stale typed log from an earlier build is removed.
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

use crate::debugger::RecordBuffer;
use crate::debugger::session_log::SessionCodec;
use crate::stringify::IpeStringify;
use crate::tea::IpeCmd;
use crate::{RECORD_ENV, TYPED_LOG_EXTENSION};

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
/// testable seam the env-driven [`dump_session`] writes out.
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
        // A path destination: replace the file with the plain dump. A failure is
        // swallowed like any write error.
        RecordDest::Path(path) => {
            let _ = replace_file(path, blob.as_bytes());
        }
    }
}

/// Replace `path` with `bytes` through an exclusively created sibling temp file.
///
/// The temp file comes from the shared scratch primitive (unguessable name,
/// exclusive, never through a symlink, mode 0600) and is renamed over `path`,
/// so the dump never writes through a symlink planted at the destination; a
/// symlinked destination is refused outright.
fn replace_file(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    if std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput));
    }
    let (tmp, mut file) = crate::scratch_core::exclusive_sibling(path)?;
    let written = file.write_all(bytes).and_then(|()| file.flush());
    drop(file);
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Write the typed log for `buf` beside the trace at `trace`.
///
/// A program with no typed log removes any stale one instead, so a replay can
/// never pick up a log an earlier build of the program wrote. Failures are
/// swallowed like every record write.
fn write_typed_log<Msg, Model, C>(
    trace: &std::path::Path,
    buf: &RecordBuffer<Msg, Model>,
    codec: &C,
) where
    C: SessionCodec<Msg, Model>,
{
    let typed = trace.with_extension(TYPED_LOG_EXTENSION);
    if typed == trace {
        return;
    }
    match codec.encode(buf) {
        Ok(bytes) => {
            let _ = replace_file(&typed, &bytes);
        }
        Err(_) => {
            // `remove_file` on a symlink removes the link, never its target.
            if std::fs::symlink_metadata(&typed).is_ok() {
                let _ = std::fs::remove_file(&typed);
            }
        }
    }
}

/// Dump `buf`'s session to the destination named by `IPE_DEBUGGER_RECORD`, if
/// that variable is set; a no-op when it is unset.
///
/// The plain trace is rendered by [`render_replay_blob`] (plain, bounded by the
/// ring cap, no `Cmd` fired) and written to the parsed [`RecordDest`], so a
/// redirected log or file receives only plain, control-free lines. A file
/// destination also gets the typed log beside it (see [`write_typed_log`]).
pub fn dump_session<Msg, Model, F, C>(buf: &RecordBuffer<Msg, Model>, update: &F, codec: &C)
where
    Msg: Clone + IpeStringify,
    Model: Clone + IpeStringify,
    F: Fn(Msg, Model) -> (Model, IpeCmd<Msg>),
    C: SessionCodec<Msg, Model>,
{
    let Ok(raw) = std::env::var(RECORD_ENV) else {
        return;
    };
    let dest = RecordDest::parse(&raw);
    let blob = render_replay_blob(buf, update);
    write_blob(&dest, &blob);
    if let RecordDest::Path(path) = &dest {
        write_typed_log(path, buf, codec);
    }
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

    // The dump replaces a regular file, and refuses a symlinked destination
    // without touching the link's target.
    #[cfg(unix)]
    #[test]
    fn replace_file_never_writes_through_a_symlink() {
        let dir = std::env::temp_dir().join(format!("ipe_record_sink_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(std::fs::create_dir_all(&dir).is_ok(), "make scratch dir");

        let log = dir.join("session.ipelog");
        assert!(replace_file(&log, b"first\n").is_ok(), "fresh write");
        assert!(replace_file(&log, b"second\n").is_ok(), "replacing write");
        assert_eq!(
            std::fs::read_to_string(&log).ok().as_deref(),
            Some("second\n")
        );

        let victim = dir.join("victim.txt");
        assert!(std::fs::write(&victim, "keep").is_ok(), "write victim");
        let link = dir.join("linked.ipelog");
        assert!(
            std::os::unix::fs::symlink(&victim, &link).is_ok(),
            "plant link"
        );
        assert!(
            replace_file(&link, b"evil").is_err(),
            "a symlinked log is refused"
        );
        assert_eq!(
            std::fs::read_to_string(&victim).ok().as_deref(),
            Some("keep")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // A program with no typed log removes a stale one beside the trace, so a
    // replay never reads a log an earlier build wrote.
    #[test]
    fn trace_only_program_removes_stale_typed_log() {
        use crate::debugger::session_log::{TraceOnly, Unreplayable};
        let dir = std::env::temp_dir().join(format!("ipe_record_typed_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(std::fs::create_dir_all(&dir).is_ok(), "make scratch dir");
        let trace = dir.join("session.ipelog");
        let typed = dir.join("session.ipemsgs");
        assert!(std::fs::write(&typed, "stale").is_ok(), "plant stale log");
        let buf: RecordBuffer<Msg, Model> = RecordBuffer::new(Model { n: 0 }, 8);
        write_typed_log(&trace, &buf, &TraceOnly(Unreplayable::MsgNotEncodable));
        assert!(
            std::fs::symlink_metadata(&typed).is_err(),
            "the stale typed log must be removed"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
