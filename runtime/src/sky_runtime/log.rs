// Log helpers — Go-format parity for `Std.Log.*`.
//
// The plain + JSON line shapes mirror `runtime-go/rt/rt.go`'s `logEmit`
// byte-for-byte (modulo the genuinely-nondeterministic timestamp):
//
//   plain:  <RFC3339Nano-UTC> <LEVEL> <message>[ key=value …]   (level UPPER)
//   json:   {"level":"<level>","msg":"<message>","time":"<ts>"}  (level lower,
//           keys alphabetically sorted to match Go's json.Marshal of a map)
//
// Stream routing matches Go: warn + error → stderr, debug + info → stdout.
// `SKY_LOG_LEVEL` gates output (debug < info < warn < error; default info);
// `SKY_LOG_FORMAT=json` switches to the JSON shape. Each line is also mirrored
// into the telemetry ring (the Sky Console reads it).
//
// `Log.println` is a SEPARATE bare line with NO prefix (Go's `Log_println` is a
// raw `fmt.Println`); codegen routes it to `log_println`, never to `log_info`.
use super::*;

const LOG_LEVEL_DEBUG: i32 = 0;
const LOG_LEVEL_INFO: i32 = 1;
const LOG_LEVEL_WARN: i32 = 2;
const LOG_LEVEL_ERROR: i32 = 3;

/// `SKY_LOG_LEVEL` → numeric threshold. Mirrors Go's `logLevelFromEnv`
/// (`debug` / `warn`|`warning` / `error`; everything else → info).
fn log_threshold() -> i32 {
    match std::env::var("SKY_LOG_LEVEL")
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "debug" => LOG_LEVEL_DEBUG,
        "warn" | "warning" => LOG_LEVEL_WARN,
        "error" => LOG_LEVEL_ERROR,
        _ => LOG_LEVEL_INFO,
    }
}

fn log_json() -> bool {
    std::env::var("SKY_LOG_FORMAT").unwrap_or_default() == "json"
}

/// Current UTC instant in Go's `time.RFC3339Nano` layout
/// (`2006-01-02T15:04:05.999999999Z07:00`): nanosecond precision with trailing
/// zeros trimmed, UTC rendered as `Z`. Matches Go's
/// `now.UTC().Format(time.RFC3339Nano)`.
fn rfc3339_nano_now() -> String {
    let now = chrono::Utc::now();
    let nanos = now.format("%9f").to_string();
    let trimmed = nanos.trim_end_matches('0');
    let date = now.format("%Y-%m-%dT%H:%M:%S").to_string();
    if trimmed.is_empty() {
        format!("{date}Z")
    } else {
        format!("{date}.{trimmed}Z")
    }
}

/// Minimal JSON string escaping for the hand-built plain/JSON records, matching
/// Go's `json.Marshal` for the characters that occur in log text. Reuses the
/// telemetry escaper so the two sinks never diverge.
fn json_str(s: &str) -> String {
    format!("\"{}\"", super::telemetry::json_escape(s))
}

/// Total line-write helpers: Rust's `println!`/`eprintln!` call `std::io::_print`,
/// which PANICS ("failed printing to stdout") when the underlying write errors.
/// Because Rust ignores SIGPIPE by default, a closed downstream pipe surfaces as
/// an `EPIPE` write error rather than process termination — so a piped consumer
/// hanging up (`sky-app | head`) would panic from a well-typed `Log.*` call.
/// These helpers perform the write fallibly and intentionally drop the `Result`,
/// turning a broken pipe into a silently-skipped line instead of an abort.
fn write_stdout_line(line: &str) {
    use std::io::Write;
    let _ = writeln!(std::io::stdout().lock(), "{line}");
}

fn write_stderr_line(line: &str) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr().lock(), "{line}");
}

/// Strip ASCII control characters from a log message for the plain-text path,
/// matching the safety guarantee the JSON path already gets via `json_escape`.
/// Keeps all printable ASCII, spaces (0x20), and multi-byte UTF-8 sequences
/// intact — only bytes 0x00–0x1F and 0x7F are affected:
///   - `\n` (0x0A) and `\r` (0x0D) are replaced by a visible `\n`/`\r` literal
///     so an attacker cannot inject newlines that forge additional log lines.
///   - All other ASCII controls are replaced by `·` (U+00B7, MIDDLE DOT) so the
///     presence of unusual bytes is visible rather than silently dropped.
///   - `\t` (0x09) is preserved as-is (benign, readable in plain log viewers).
fn sanitise_log_msg(msg: &str) -> std::borrow::Cow<'_, str> {
    // Fast path: most messages are clean — scan without allocating.
    let needs_escape = msg
        .bytes()
        .any(|b| matches!(b, 0x00..=0x08 | 0x0A..=0x1F | 0x7F) && b != b'\t');
    if !needs_escape {
        return std::borrow::Cow::Borrowed(msg);
    }
    let mut out = String::with_capacity(msg.len() + 8);
    for ch in msg.chars() {
        match ch {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push('\t'),
            c if (c as u32) < 0x20 || c == '\x7F' => out.push('·'),
            c => out.push(c),
        }
    }
    std::borrow::Cow::Owned(out)
}

/// The Go `logEmit` core: gate on the threshold, mirror into the telemetry ring,
/// then write the plain or JSON line to the correct stream. `level` is the
/// numeric severity; `level_name` is the lowercase token (`info` / `warn` / …).
fn log_emit(level: i32, level_name: &str, msg: &str) {
    if level < log_threshold() {
        return;
    }
    super::telemetry::record_log(level_name, msg);
    let to_stderr = level >= LOG_LEVEL_WARN;
    if log_json() {
        // Go marshals a map[string]any → keys sorted alphabetically:
        // level, msg, time (no attrs are surfaced to JSON fields today —
        // the *With variants flatten into the message, matching Go's
        // plain-driver behaviour for the List-element call shape).
        let line = format!(
            "{{\"level\":{},\"msg\":{},\"time\":{}}}",
            json_str(level_name),
            json_str(msg),
            json_str(&rfc3339_nano_now()),
        );
        if to_stderr {
            write_stderr_line(&line);
        } else {
            write_stdout_line(&line);
        }
        return;
    }
    // Plain mode: sanitise before writing so control chars / embedded newlines
    // can't forge extra log lines (the JSON path is safe via json_escape already).
    let safe_msg = sanitise_log_msg(msg);
    let line = format!(
        "{} {} {}",
        rfc3339_nano_now(),
        level_name.to_ascii_uppercase(),
        safe_msg
    );
    if to_stderr {
        write_stderr_line(&line);
    } else {
        write_stdout_line(&line);
    }
}

/// `Log.println : String -> Task Error ()` — Go's `Log_println` is a bare
/// `fmt.Println`: NO timestamp, NO level, straight to stdout. Still mirrored into
/// the telemetry ring at info level so the console surfaces it.
pub fn log_println<E: Send + 'static>(msg: String) -> SkyTask<E, ()> {
    Box::pin(async move {
        super::telemetry::record_log("info", &msg);
        write_stdout_line(&msg);
        ok_res(())
    })
}

pub fn log_info<E: Send + 'static>(msg: String) -> SkyTask<E, ()> {
    Box::pin(async move {
        log_emit(LOG_LEVEL_INFO, "info", &msg);
        ok_res(())
    })
}

// `Log.*With : String -> List a -> Task` is polymorphic in the attr element
// (Sky callers pass a flat `List String` — `["errId", id]` — OR a key/value
// `List (String, String)` — `[("errId", id), …]`). The attrs slot is generic
// over its element type `A`, bounded by `SkyStringify` — the TOTAL Go-`%v`
// stringifier every Sky-representable type implements (String unquoted, tuples
// as `{k v}`, generated records/ADTs via their codegen-emitted impl). A plain
// `Display` bound was wrong: tuples + generated types don't implement `Display`,
// so it failed to compile at any tuple/record call site (the E0277 in
// routes_auth / routes_todos). `SkyStringify` is satisfiable at EVERY concrete
// element type codegen can emit, so no codegen change is needed — the call site
// passes its concrete `Vec<A>` and the bound always holds.
//
// Rendering mirrors Go's `renderLogMsgWithAttrs` byte-for-byte: the flat attr
// list is space-joined onto the message (`msg a1 a2 …`, each `ai` via `%v`),
// then handed to `log_emit` as a single pre-rendered line — so the plain path
// sanitises the attr values too (no newline-injection via an attr) and the JSON
// path surfaces them inside `msg` exactly as Go does for the List call shape
// (Go's With variants pass `ctx=nil`).

/// Flatten `(msg, attrs)` into one line, mirroring Go's `renderLogMsgWithAttrs`:
/// `msg` followed by a space + the `%v` of each attr element, in order.
fn render_with_attrs<A: SkyStringify>(msg: &str, attrs: &[A]) -> String {
    if attrs.is_empty() {
        return msg.to_string();
    }
    let mut out = String::from(msg);
    for a in attrs {
        out.push(' ');
        out.push_str(&a.sky_show());
    }
    out
}

pub fn log_info_with<E: Send + 'static, A: SkyStringify>(
    msg: String,
    attrs: Vec<A>,
) -> SkyTask<E, ()> {
    // Render BEFORE constructing the future so the captured value is a `Send`
    // `String` (no `A: Send` bound needed); the line write still fires on `.await`.
    let line = render_with_attrs(&msg, &attrs);
    Box::pin(async move {
        log_emit(LOG_LEVEL_INFO, "info", &line);
        ok_res(())
    })
}

pub fn log_error_with<E: Send + 'static, A: SkyStringify>(
    msg: String,
    attrs: Vec<A>,
) -> SkyTask<E, ()> {
    let line = render_with_attrs(&msg, &attrs);
    Box::pin(async move {
        log_emit(LOG_LEVEL_ERROR, "error", &line);
        ok_res(())
    })
}

pub fn log_debug<E: Send + 'static>(msg: String) -> SkyTask<E, ()> {
    Box::pin(async move {
        log_emit(LOG_LEVEL_DEBUG, "debug", &msg);
        ok_res(())
    })
}
pub fn log_warn<E: Send + 'static>(msg: String) -> SkyTask<E, ()> {
    Box::pin(async move {
        log_emit(LOG_LEVEL_WARN, "warn", &msg);
        ok_res(())
    })
}
pub fn log_error<E: Send + 'static>(msg: String) -> SkyTask<E, ()> {
    Box::pin(async move {
        log_emit(LOG_LEVEL_ERROR, "error", &msg);
        ok_res(())
    })
}
pub fn log_debug_with<E: Send + 'static, A: SkyStringify>(
    msg: String,
    attrs: Vec<A>,
) -> SkyTask<E, ()> {
    let line = render_with_attrs(&msg, &attrs);
    Box::pin(async move {
        log_emit(LOG_LEVEL_DEBUG, "debug", &line);
        ok_res(())
    })
}
pub fn log_warn_with<E: Send + 'static, A: SkyStringify>(
    msg: String,
    attrs: Vec<A>,
) -> SkyTask<E, ()> {
    let line = render_with_attrs(&msg, &attrs);
    Box::pin(async move {
        log_emit(LOG_LEVEL_WARN, "warn", &line);
        ok_res(())
    })
}
