//! Refusal tests for the installer's terminal messages.
//!
//! A message's fixed text and its values are separate arguments: every write
//! `install.sh` makes to the terminal goes through the message-helper block,
//! whose helpers take a single-quoted format plus values and escape each value
//! through `safe_text`. These tests drive the whole script with hostile
//! environment values, drive `safe_text` and the release-tag parser directly,
//! and scan the script statically so that any other message shape is refused.
#![cfg(unix)]

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::iter::Peekable;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Output, Stdio};
use std::time::Duration;

use ipe_sandbox::scratch::{LeafName, ScratchDir};

const BEGIN: &str = "# >>> message helpers";
const END: &str = "# <<< message helpers";
const PARSERS: (&str, &str) = ("# >>> input parsers", "# <<< input parsers");

/// The helpers whose first argument is a message format.
const HELPERS: [&str; 8] = [
    "die",
    "info",
    "say",
    "prompt",
    "stage_start",
    "stage_ok",
    "stage_fail",
    "banner",
];

/// The block-internal helpers, which print text they do not escape.
const INTERNAL: [&str; 5] = [
    "render",
    "msg_text",
    "msg_style",
    "stage_settle_ok",
    "stage_settle_fail",
];

/// The words after which the next word is still in command position.
const KEYWORDS: [&str; 10] = [
    "{", "}", "!", "if", "then", "else", "elif", "do", "while", "until",
];

/// The code points >= U+0080 `safe_text` escapes: C1 (Cc), Cf, Zl and Zp.
const ESCAPED_RANGES: &[(u32, u32)] = &[
    (0x80, 0x9F),
    (0xAD, 0xAD),
    (0x600, 0x605),
    (0x61C, 0x61C),
    (0x6DD, 0x6DD),
    (0x70F, 0x70F),
    (0x890, 0x891),
    (0x8E2, 0x8E2),
    (0x180E, 0x180E),
    (0x200B, 0x200F),
    (0x2028, 0x202E),
    (0x2060, 0x206F),
    (0xFEFF, 0xFEFF),
    (0xFFF9, 0xFFFB),
    (0x110BD, 0x110BD),
    (0x110CD, 0x110CD),
    (0x13430, 0x1343F),
    (0x1BCA0, 0x1BCA3),
    (0x1D173, 0x1D17A),
    (0xE0001, 0xE0001),
    (0xE0020, 0xE007F),
];

fn installer_path() -> PathBuf {
    e2e_support::manifest_dir!().join("../../install.sh")
}

fn installer_script() -> io::Result<String> {
    std::fs::read_to_string(installer_path())
}

/// The `(begin, end)` marked block of `script`, markers included.
fn marked<'s>(script: &'s str, (begin, end): (&str, &str)) -> io::Result<&'s str> {
    script
        .find(begin)
        .and_then(|start| {
            script
                .get(start..)
                .and_then(|tail| tail.find(end).map(|at| (start, start + at + end.len())))
        })
        .and_then(|(start, stop)| script.get(start..stop))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("install.sh lost its `{begin}` markers"),
            )
        })
}

/// The output of `sh` running `body` after the message and input-parser
/// blocks in `locale`, with `args` as `$@`.
fn run_block(body: &str, args: &[&OsStr], locale: &str) -> io::Result<Output> {
    let script = installer_script()?;
    let program = format!(
        "{}\n{}\n{body}\n",
        marked(&script, (BEGIN, END))?,
        marked(&script, PARSERS)?
    );
    Command::new("sh")
        .env("LC_ALL", locale)
        .arg("-c")
        .arg(program)
        .arg("sh")
        .args(args)
        .output()
}

/// `safe_text` of each input in `locale`, one output per input.
fn safe_text_each(inputs: &[&[u8]], locale: &str) -> io::Result<Vec<Vec<u8>>> {
    let args: Vec<&OsStr> = inputs
        .iter()
        .map(|bytes| OsStr::from_bytes(bytes))
        .collect();
    let output = run_block(
        "for a in \"$@\"; do safe_text \"$a\"; printf '\\n'; done",
        &args,
        locale,
    )?;
    Ok(output
        .stdout
        .split(|byte| *byte == b'\n')
        .take(inputs.len())
        .map(<[u8]>::to_vec)
        .collect())
}

/// `bytes` written as the `\ooo` octal escapes `safe_text` prints.
fn octal(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("\\{byte:03o}")).collect()
}

/// Whether `safe_text` must escape the code point `cp` (>= U+0080).
fn escaped_code_point(cp: u32) -> bool {
    ESCAPED_RANGES
        .iter()
        .any(|&(lo, hi)| (lo..=hi).contains(&cp))
}

/// `ESCAPED_RANGES` spelled as the shell range list.
fn range_list() -> String {
    ESCAPED_RANGES
        .iter()
        .map(|&(lo, hi)| {
            if lo == hi {
                format!("{lo:X}")
            } else {
                format!("{lo:X}-{hi:X}")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Each escaped range's first and last code point, and its neighbours.
///
/// A first or last code point is paired with `true` (escaped); the neighbour
/// one step outside, when it is not itself escaped, with `false` (raw under
/// UTF-8).
fn range_cases() -> Vec<(char, bool)> {
    let mut cases = Vec::new();
    for &(lo, hi) in ESCAPED_RANGES {
        for cp in [lo, hi] {
            cases.extend(char::from_u32(cp).map(|c| (c, true)));
        }
        for cp in [lo.checked_sub(1), hi.checked_add(1)].into_iter().flatten() {
            if cp >= 0x80 && !escaped_code_point(cp) {
                cases.extend(char::from_u32(cp).map(|c| (c, false)));
            }
        }
    }
    cases
}

/// A test root under the per-binary target temp dir.
fn root(label: &str) -> io::Result<ScratchDir> {
    ScratchDir::new_under(Path::new(env!("CARGO_TARGET_TMPDIR")), label)
}

/// `name` inside the scratch root `r`.
fn leaf(r: &ScratchDir, name: &str) -> io::Result<PathBuf> {
    Ok(r.child(&LeafName::new(name)?))
}

fn mkdir_mode(path: &Path, mode: u32) -> io::Result<()> {
    std::fs::create_dir(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

/// The outcome of one whole-script installer run.
struct Run {
    /// The exit status, or `None` when the run timed out and was killed.
    status: Option<ExitStatus>,
    stderr: Vec<u8>,
    /// Every argument line the `curl` stub was called with.
    curl_log: Vec<u8>,
}

/// Run `sh install.sh` under the scratch root `r`.
///
/// The environment is cleared: a private `HOME`, `LC_ALL=C`, a `PATH` whose
/// `curl` is a stub that logs its arguments and exits 7, then `env` on top.
/// Stdin is empty; a run past 30 s is killed.
fn run_installer(r: &ScratchDir, env: &[(&str, &OsStr)]) -> io::Result<Run> {
    let bin = leaf(r, "bin")?;
    std::fs::create_dir(&bin)?;
    let curl = bin.join("curl");
    std::fs::write(
        &curl,
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >>\"$CURL_LOG\"\nexit 7\n",
    )?;
    std::fs::set_permissions(&curl, std::fs::Permissions::from_mode(0o755))?;
    let home = leaf(r, "home")?;
    mkdir_mode(&home, 0o700)?;
    let log = leaf(r, "curl.log")?;
    let stderr_path = leaf(r, "stderr")?;
    let mut path = OsString::from(bin.as_os_str());
    path.push(":/usr/bin:/bin");

    let mut child = Command::new("sh")
        .arg(installer_path())
        .env_clear()
        .env("PATH", path)
        .env("HOME", home)
        .env("CURL_LOG", &log)
        .env("LC_ALL", "C")
        .envs(env.iter().copied())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(File::create(&stderr_path)?)
        .spawn()?;
    let mut status = None;
    let finished = e2e_support::wait_for(Duration::from_secs(30), || {
        status = child.try_wait().ok().flatten();
        status.is_some()
    });
    if !finished {
        child.kill()?;
        child.wait()?;
    }
    let curl_log = match std::fs::read(log) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error),
    };
    Ok(Run {
        status,
        stderr: std::fs::read(stderr_path)?,
        curl_log,
    })
}

/// Assert that `run` refused safely and printed `escaped`.
///
/// Safely: exit 1, before any network call, and with no raw ESC or BEL byte.
fn assert_refused_escaped(run: &Run, escaped: &str, why: &str) {
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert_eq!(
        run.status.and_then(|status| status.code()),
        Some(1),
        "{why}: the installer must exit 1; stderr: {stderr}"
    );
    assert!(
        !run.stderr.iter().any(|byte| matches!(byte, 0x1b | 0x07)),
        "{why}: stderr must carry no raw ESC or BEL byte: {stderr}"
    );
    assert!(
        stderr.contains(escaped),
        "{why}: stderr must print `{escaped}`: {stderr}"
    );
    assert!(
        run.curl_log.is_empty(),
        "{why}: the refusal must come before any network call, got curl {}",
        String::from_utf8_lossy(&run.curl_log)
    );
}

#[test]
fn install_dir_with_osc_is_refused_and_escaped() -> io::Result<()> {
    let r = root("install-msg-dir")?;
    let dir = OsStr::new("/x\u{1b}]0;x\u{7}");
    let run = run_installer(&r, &[("IPE_INSTALL_DIR", dir)])?;
    assert_refused_escaped(&run, "/x\\033]0;x\\007", "an OSC in IPE_INSTALL_DIR");
    Ok(())
}

#[test]
fn tmpdir_with_osc_is_refused_and_escaped() -> io::Result<()> {
    let r = root("install-msg-tmpdir")?;
    let hostile = r.path().join("a\u{1b}]0;x\u{7}");
    mkdir_mode(&hostile, 0o777)?;
    let run = run_installer(&r, &[("TMPDIR", hostile.as_os_str())])?;
    assert_refused_escaped(&run, "a\\033]0;x\\007", "an OSC in TMPDIR");
    assert!(
        String::from_utf8_lossy(&run.stderr).contains("Refusing the temp directory"),
        "a non-sticky world-writable TMPDIR must be refused at the boundary"
    );
    Ok(())
}

#[test]
fn valid_utf8_prints_raw_only_in_utf8_locale() -> io::Result<()> {
    for (locale, shown) in [("C.UTF-8", "caf\u{e9}-open"), ("C", "caf\\303\\251-open")] {
        let r = root("install-msg-utf8")?;
        let open = r.path().join("caf\u{e9}-open");
        mkdir_mode(&open, 0o777)?;
        let run = run_installer(
            &r,
            &[("TMPDIR", open.as_os_str()), ("LC_ALL", OsStr::new(locale))],
        )?;
        assert_refused_escaped(&run, shown, locale);
    }
    Ok(())
}

#[test]
fn version_tag_outside_grammar_is_refused_before_network() -> io::Result<()> {
    for (tag, shown) in [("v1/../../x", "v1/../../x"), ("v1\u{1b}[2J", "v1\\033[2J")] {
        let r = root("install-msg-tag")?;
        let run = run_installer(&r, &[("IPE_VERSION", OsStr::new(tag))])?;
        assert_refused_escaped(&run, shown, tag);
        assert!(
            String::from_utf8_lossy(&run.stderr).contains("is not a version tag"),
            "IPE_VERSION `{shown}` must be refused as a malformed tag"
        );
    }
    Ok(())
}

#[test]
fn release_tag_ok_table() -> io::Result<()> {
    let longest = format!("v1.{}", "0".repeat(125));
    let too_long = format!("v1.{}", "0".repeat(126));
    let cases: [(&str, bool); 13] = [
        ("v0.2.6", true),
        ("ipe-v1.0.0-rc.1", true),
        ("v1.0+build.7", true),
        (longest.as_str(), true),
        ("", false),
        ("1.0", false),
        ("v1/2", false),
        ("v1?x", false),
        ("v1#", false),
        ("v1%2f", false),
        ("v1 2", false),
        ("v1\u{e9}", false),
        (too_long.as_str(), false),
    ];
    let args: Vec<&OsStr> = cases.iter().map(|(tag, _)| OsStr::new(tag)).collect();
    let output = run_block(
        "for a in \"$@\"; do if release_tag_ok \"$a\"; then echo y; else echo n; fi; done",
        &args,
        "C",
    )?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let verdicts: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        verdicts.len(),
        cases.len(),
        "one verdict per tag: {verdicts:?}"
    );
    for ((tag, accepted), verdict) in cases.iter().zip(&verdicts) {
        let expected = if *accepted { "y" } else { "n" };
        assert_eq!(
            *verdict,
            expected,
            "release_tag_ok `{tag}` ({} bytes) must {}",
            tag.len(),
            if *accepted { "accept" } else { "refuse" }
        );
    }
    Ok(())
}

#[test]
fn c1_and_invalid_bytes_are_escaped_in_utf8_locale() -> io::Result<()> {
    let script = installer_script()?;
    let shell_list = script
        .split_once("nr = split(\"")
        .and_then(|(_, tail)| tail.split_once('"'))
        .map(|(list, _)| list);
    assert_eq!(
        shell_list,
        Some(range_list().as_str()),
        "the safe_text range list must equal ESCAPED_RANGES"
    );

    let invalid: [&[u8]; 6] = [
        b"\xc2\x9b",
        b"\x9b",
        b"\xc0\xaf",
        b"\xed\xa0\x80",
        b"\xf4\x90\x80\x80",
        b"\xe2\x82",
    ];
    let outputs = safe_text_each(&invalid, "C.UTF-8")?;
    assert_eq!(outputs.len(), invalid.len(), "one output per input");
    for (input, output) in invalid.iter().zip(&outputs) {
        assert_eq!(
            String::from_utf8_lossy(output),
            octal(input),
            "C1, malformed, overlong, surrogate, out-of-range and truncated \
             sequences must print byte by byte as escapes"
        );
    }

    let cases = range_cases();
    let encoded: Vec<Vec<u8>> = cases
        .iter()
        .map(|&(c, _)| String::from(c).into_bytes())
        .collect();
    let inputs: Vec<&[u8]> = encoded.iter().map(Vec::as_slice).collect();
    for locale in ["C.UTF-8", "C"] {
        let outputs = safe_text_each(&inputs, locale)?;
        assert_eq!(outputs.len(), cases.len(), "one output per code point");
        for (((c, escaped), bytes), output) in cases.iter().zip(&encoded).zip(&outputs) {
            let expected = if *escaped || locale == "C" {
                octal(bytes).into_bytes()
            } else {
                bytes.clone()
            };
            assert_eq!(
                output,
                &expected,
                "U+{:04X} in {locale} must print {}",
                u32::from(*c),
                if *escaped { "escaped" } else { "raw" }
            );
        }
    }
    Ok(())
}

#[test]
fn backslash_and_style_token_in_value_stay_literal() -> io::Result<()> {
    let output = run_block(
        "C_BOLD=BOLD; render 'x @B@%s|%s' \"$1\" \"$2\"",
        &[OsStr::new("a\\033@B@"), OsStr::new("%s%n")],
        "C",
    )?;
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "x BOLDa\\\\033@B@|%s%n",
        "a value's backslash must double and its style token and directives stay literal"
    );
    assert!(
        !output.stdout.contains(&0x1b),
        "a value must never expand into an escape byte"
    );
    Ok(())
}

/// How a shell word is quoted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Unit {
    /// Exactly one `'…'` segment.
    Single,
    /// Exactly one `"…"` segment.
    Double,
    /// Anything else: bare text, an expansion, or several segments.
    Bare,
}

/// A shell word: its source text, its quoting, and the bodies of the command
/// substitutions it contains.
#[derive(Debug)]
struct Word {
    text: String,
    unit: Unit,
    substitutions: Vec<String>,
}

/// One lexed shell token.
#[derive(Debug)]
enum Token {
    Word(Word),
    /// A newline, `;`, `&`, `|`, `(` or `)`.
    Separator,
    /// A redirection operator (with any fd prefix) and its target.
    Redirect {
        op: String,
        target: String,
    },
}

type Chars<'s> = Peekable<std::str::Chars<'s>>;

/// Whether `c` ends a word.
const fn ends_word(c: char) -> bool {
    matches!(
        c,
        ' ' | '\t' | '\n' | ';' | '&' | '|' | '(' | ')' | '<' | '>'
    )
}

/// Lex `source` with `\`-continued lines joined and comments skipped.
fn lex(source: &str) -> Vec<Token> {
    let joined = source.replace("\\\n", "");
    let mut chars = joined.chars().peekable();
    let mut tokens = Vec::new();
    while let Some(&c) = chars.peek() {
        match c {
            ' ' | '\t' => {
                chars.next();
            }
            '\n' | ';' | '&' | '|' | '(' | ')' => {
                chars.next();
                tokens.push(Token::Separator);
            }
            '#' => while chars.next_if(|&n| n != '\n').is_some() {},
            '<' | '>' => tokens.push(redirect(&mut chars, String::new())),
            _ => {
                let word = word(&mut chars);
                let fd = word.unit == Unit::Bare && word.text.bytes().all(|b| b.is_ascii_digit());
                if fd && matches!(chars.peek(), Some(&('<' | '>'))) {
                    tokens.push(redirect(&mut chars, word.text));
                } else {
                    tokens.push(Token::Word(word));
                }
            }
        }
    }
    tokens
}

/// Lex a redirection whose operator starts at the next char, after the fd
/// prefix `op`.
fn redirect(chars: &mut Chars<'_>, mut op: String) -> Token {
    if let Some(first) = chars.next() {
        op.push(first);
        op.extend(chars.next_if_eq(&first));
        op.extend(chars.next_if(|&n| matches!(n, '&' | '|')));
    }
    let mut target = String::new();
    if op.ends_with('&') {
        while let Some(d) = chars.next_if(|&n| n.is_ascii_digit() || n == '-') {
            target.push(d);
        }
    } else {
        while chars.next_if(|&n| n == ' ' || n == '\t').is_some() {}
        if chars.peek().is_some_and(|&n| !ends_word(n)) {
            target = word(chars).text;
        }
    }
    Token::Redirect { op, target }
}

/// Lex one word starting at the next char.
fn word(chars: &mut Chars<'_>) -> Word {
    let mut text = String::new();
    let mut substitutions = Vec::new();
    let mut segments = 0_usize;
    let mut quote = Unit::Bare;
    let mut bare = false;
    while let Some(c) = chars.next_if(|&n| !ends_word(n)) {
        text.push(c);
        match c {
            '\'' => {
                single(chars, &mut text);
                segments += 1;
                quote = Unit::Single;
            }
            '"' => {
                double(chars, &mut text, &mut substitutions);
                segments += 1;
                quote = Unit::Double;
            }
            '\\' => {
                bare = true;
                text.extend(chars.next());
            }
            '$' => {
                bare = true;
                dollar(chars, &mut text, &mut substitutions);
            }
            '`' => {
                bare = true;
                backtick(chars, &mut text, &mut substitutions);
            }
            _ => bare = true,
        }
    }
    let unit = if !bare && segments == 1 {
        quote
    } else {
        Unit::Bare
    };
    Word {
        text,
        unit,
        substitutions,
    }
}

/// Copy the rest of a `'…'` segment, closing quote included.
fn single(chars: &mut Chars<'_>, out: &mut String) {
    for c in chars.by_ref() {
        out.push(c);
        if c == '\'' {
            return;
        }
    }
}

/// Copy the rest of a `"…"` segment, closing quote included, recording its
/// command substitutions.
fn double(chars: &mut Chars<'_>, out: &mut String, substitutions: &mut Vec<String>) {
    while let Some(c) = chars.next() {
        out.push(c);
        match c {
            '"' => return,
            '\\' => out.extend(chars.next()),
            '$' => dollar(chars, out, substitutions),
            '`' => backtick(chars, out, substitutions),
            _ => {}
        }
    }
}

/// Copy the expansion after a `$`: `$((…))`, `$(…)` (recorded as a command
/// substitution) or `${…}`; a plain `$name` is left to the caller.
fn dollar(chars: &mut Chars<'_>, out: &mut String, substitutions: &mut Vec<String>) {
    if chars.next_if_eq(&'(').is_some() {
        out.push('(');
        if chars.next_if_eq(&'(').is_some() {
            out.push('(');
            nested(chars, out, ')');
            out.extend(chars.next_if_eq(&')'));
        } else {
            let start = out.len();
            nested(chars, out, ')');
            let end = out.len().saturating_sub(1);
            substitutions.push(out.get(start..end).unwrap_or_default().to_owned());
        }
    } else if chars.next_if_eq(&'{').is_some() {
        out.push('{');
        nested(chars, out, '}');
    }
}

/// Copy the rest of a `` `…` `` command substitution and record its body.
fn backtick(chars: &mut Chars<'_>, out: &mut String, substitutions: &mut Vec<String>) {
    let start = out.len();
    while let Some(c) = chars.next() {
        out.push(c);
        if c == '`' {
            break;
        }
        if c == '\\' {
            out.extend(chars.next());
        }
    }
    let end = out.len().saturating_sub(1);
    substitutions.push(out.get(start..end).unwrap_or_default().to_owned());
}

/// Copy up to and including the `close` that balances an already-copied
/// opener, honouring quotes and nested groups.
fn nested(chars: &mut Chars<'_>, out: &mut String, close: char) {
    while let Some(c) = chars.next() {
        out.push(c);
        if c == close {
            return;
        }
        match c {
            '\\' => out.extend(chars.next()),
            '\'' => single(chars, out),
            '"' => double(chars, out, &mut Vec::new()),
            '`' => backtick(chars, out, &mut Vec::new()),
            '(' if close == ')' => nested(chars, out, ')'),
            '{' if close == '}' => nested(chars, out, '}'),
            _ => {}
        }
    }
}

/// What the message scan found.
#[derive(Debug, Default)]
struct Scan {
    /// The message-helper calls it checked.
    calls: usize,
    /// Every refused shape, described.
    violations: Vec<String>,
}

/// Scan `script` for message shapes outside the helper contract.
///
/// Its message block must appear exactly once. Outside it, every
/// message-helper call must take a single-quoted format plus one double-quoted
/// value per `%s`, and nothing may write to the terminal.
fn scan_script(script: &str) -> Scan {
    let mut scan = Scan::default();
    let once = script.matches(BEGIN).count() == 1 && script.matches(END).count() == 1;
    let outside = match (script.find(BEGIN), script.find(END)) {
        (Some(begin), Some(end)) if once && begin < end => format!(
            "{}{}",
            script.get(..begin).unwrap_or_default(),
            script.get(end + END.len()..).unwrap_or_default()
        ),
        _ => {
            scan.violations.push(format!(
                "the `{BEGIN}` and `{END}` markers must each appear once, in order"
            ));
            script.to_owned()
        }
    };
    scan_tokens(&lex(&outside), &mut scan);
    scan
}

/// Whether `op target` writes to the terminal.
fn writes_terminal(op: &str, target: &str) -> bool {
    let target = target.trim_matches(['"', '\'']);
    op.contains('>')
        && ((op.ends_with('&') && target == "2")
            || matches!(target, "/dev/tty" | "/dev/stderr" | "/dev/fd/2"))
}

/// Whether `text` is a `NAME=value` assignment word.
fn is_assignment(text: &str) -> bool {
    text.split_once('=').is_some_and(|(name, _)| {
        name.chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

fn scan_tokens(tokens: &[Token], scan: &mut Scan) {
    let mut command = true;
    for (at, token) in tokens.iter().enumerate() {
        match token {
            Token::Separator => command = true,
            Token::Redirect { op, target } => {
                if writes_terminal(op, target) {
                    scan.violations.push(format!(
                        "`{op}{target}` writes to the terminal outside the message block"
                    ));
                }
            }
            Token::Word(word) => {
                for body in &word.substitutions {
                    scan_tokens(&lex(body), scan);
                }
                if command {
                    command = KEYWORDS.contains(&word.text.as_str()) || is_assignment(&word.text);
                    if !command {
                        let rest = tokens.get(at + 1..).unwrap_or_default();
                        check_command(word, rest, scan);
                    }
                }
            }
        }
    }
}

/// Check the command named by `name`, whose arguments start `rest`.
fn check_command(name: &Word, rest: &[Token], scan: &mut Scan) {
    let name = name.text.as_str();
    if INTERNAL.contains(&name) {
        scan.violations
            .push(format!("`{name}` is internal to the message block"));
        return;
    }
    if !HELPERS.contains(&name) {
        return;
    }
    scan.calls += 1;
    let args: Vec<&Word> = rest
        .iter()
        .take_while(|token| !matches!(token, Token::Separator))
        .filter_map(|token| match token {
            Token::Word(word) => Some(word),
            Token::Separator | Token::Redirect { .. } => None,
        })
        .collect();
    if let Some(why) = call_refusal(&args) {
        let call: Vec<&str> = args.iter().map(|word| word.text.as_str()).collect();
        scan.violations
            .push(format!("`{name} {}`: {why}", call.join(" ")));
    }
}

/// Why a message-helper call with `args` is refused, if it is.
fn call_refusal(args: &[&Word]) -> Option<String> {
    let Some((format, values)) = args.split_first() else {
        return Some("no format".to_owned());
    };
    if format.unit != Unit::Single {
        return Some("the format is not one single-quoted literal".to_owned());
    }
    let literal = format
        .text
        .strip_prefix('\'')
        .and_then(|text| text.strip_suffix('\''))
        .unwrap_or_default();
    if literal.contains('$') {
        return Some("the format contains `$`".to_owned());
    }
    let mut fills = 0_usize;
    let mut chars = literal.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            match chars.next() {
                Some('s') => fills += 1,
                Some('%') => {}
                other => {
                    return Some(format!(
                        "the directive `%{}` is neither `%s` nor `%%`",
                        other.map(String::from).unwrap_or_default()
                    ));
                }
            }
        }
    }
    if fills != values.len() {
        return Some(format!("{fills} `%s` for {} values", values.len()));
    }
    values
        .iter()
        .find(|value| value.unit != Unit::Double)
        .map(|value| format!("the value `{}` is not double-quoted", value.text))
}

/// `fixture` after a message block that itself writes to stderr.
fn with_block(fixture: &str) -> String {
    format!("{BEGIN}\nsay() {{ printf '%s\\n' \"$1\" >&2; }}\n{END}\n{fixture}\n")
}

#[test]
fn installer_messages_take_values_as_arguments() -> io::Result<()> {
    let scan = scan_script(&installer_script()?);
    assert!(
        scan.violations.is_empty(),
        "install.sh message shapes outside the helper contract: {:#?}",
        scan.violations
    );
    assert!(
        scan.calls >= 40,
        "the scan must see the installer's message calls, saw {}",
        scan.calls
    );
    Ok(())
}

#[test]
fn message_scan_refuses_every_bypass() {
    let braced = "info \"${".to_owned() + "A}\"";
    let refused = [
        "die \"x $y\"",
        "die \"$(f)\"",
        "die $x",
        braced.as_str(),
        "die 'x %s'",
        "die 'x %d' \"$v\"",
        "printf '%s' \"$x\" >&2",
        "{ die \"a $b\"; }",
        "foo || die \"$c\"",
        "die \\\n\"$x\"",
        "die 'x' \"$y\"",
        "die 'a'\\''b'",
        "die 'cost $5'",
        "say 'x %s' $v",
        "say 'x %s' \"$a\"b",
        "x=\"$(die \"$y\")\"",
        "if true; then info \"$m\"; fi",
        "echo hi 1>&2",
        "printf x >/dev/tty",
        "printf x >/dev/stderr",
        "msg_text 'x'",
        "stage_settle_ok \"$x\"",
        "die() { :; }",
    ];
    for fixture in refused {
        let scan = scan_script(&with_block(fixture));
        assert!(
            !scan.violations.is_empty(),
            "the scan must refuse `{fixture}`"
        );
    }

    let accepted = [
        "",
        "die 'x %s' \"$y\"",
        "foo || die 'a %s %%' \"$b\"",
        "stage_ok 'Found %s.' \"$(f \"$t\" | cut -d' ' -f2)\"",
        "printf '%s' \"$x\" >\"$file\" 2>/dev/null",
        "IFS= read -r ans </dev/tty",
        "case $x in\n  *) die 'y %s' \"$x\" ;;\nesac",
        "# die \"$x\" in a comment",
        "die 'a %s' \\\n  \"$b\"",
    ];
    for fixture in accepted {
        let scan = scan_script(&with_block(fixture));
        assert!(
            scan.violations.is_empty(),
            "the scan must accept `{fixture}`: {:?}",
            scan.violations
        );
    }

    for (script, why) in [
        (
            format!("{BEGIN}\nsay() {{ :; }}\n"),
            "a block with no end marker",
        ),
        (
            format!("{}{}", with_block(""), with_block("")),
            "a second message block",
        ),
        (format!("{END}\n{BEGIN}\n"), "markers out of order"),
    ] {
        assert!(
            !scan_script(&script).violations.is_empty(),
            "the scan must refuse {why}"
        );
    }
}
