#![forbid(unsafe_code)]
//! Refuses a direct read of the home directory anywhere in the workspace's
//! production sources.
//!
//! An unset, empty, or relative `HOME` resolves against whatever the current
//! working directory is, so every home read goes through one validated accessor
//! that yields an absolute path or nothing: `ipe_sandbox::home::home_dir` on the
//! compiler side and `system::home_dir` in the standalone runtime.
//!
//! The scan lexes each production file (comments dropped, `#[cfg(test)]` items
//! skipped, string literals decoded) and refuses:
//! - any string literal, raw or not, spelling a home variable name — so a
//!   `var("HOME")`, a `const` key, `env!`/`option_env!`, or a key handed to a
//!   computed-key reader is caught at the literal;
//! - the std/`dirs` home helpers and whole-environment iteration;
//! - an environment read whose key is neither a literal nor a `SCREAMING_CASE`
//!   constant (whose own literal the first rule sees), and a reader passed or
//!   imported as a value.
//!
//! Each exception names one function in one file with its reason
//! ([`LITERAL_ALLOWED`], [`DYNAMIC_KEY_ALLOWED`]); the rest of that file is
//! scanned like any other. A key assembled at run time from pieces
//! (`concat!`, `format!` over fragments) inside an allowlisted computed-key
//! reader is beyond a lexical scan.

use std::ops::Range;
use std::path::{Path, PathBuf};

/// The environment variable names that carry the user's home directory.
const HOME_NAMES: &[&str] = &["HOME", "USERPROFILE"];

/// A function allowed to hold a construct the scan otherwise refuses.
struct Allowed {
    /// Workspace-relative file.
    file: &'static str,
    /// The function, by name, whose body is exempt.
    func: &'static str,
    /// Why this site is sound.
    reason: &'static str,
}

/// Functions allowed to spell a home variable name.
const LITERAL_ALLOWED: &[Allowed] = &[
    Allowed {
        file: "src/compiler/sandbox/src/home.rs",
        func: "home_dir",
        reason: "the compiler-side home accessor; parses the value to an absolute path",
    },
    Allowed {
        file: "src/runtime/rust/src/system.rs",
        func: "home_dir",
        reason: "the runtime home accessor; parses the value to an absolute path",
    },
    Allowed {
        file: "src/ipe-cli/src/audit_native.rs",
        func: "cargo_home_env",
        reason: "writes the accessor's validated home into a child's environment; reads nothing",
    },
];

/// The reason shared by the jail spawners' `host_env` reader closures.
const JAIL_HOST_ENV: &str = "`host_env` closure: yields only the jail's fixed \
     `LANG`/`PATH`/`SystemRoot` and the profile's consented `env_allowlist`";

/// The reason shared by the runtime's `System.getenv*` kernels.
const PROGRAM_GETENV: &str = "a `System.getenv*` kernel: the key is the Ipê program's own \
     choice, under the program's own consented environment capability";

/// Functions allowed to read the environment under a computed key.
const DYNAMIC_KEY_ALLOWED: &[Allowed] = &[
    Allowed {
        file: "src/ipe-cli/src/env_dir.rs",
        func: "ambient_home",
        reason: "callers pass `XDG_*` literals, which the literal rule scans",
    },
    Allowed {
        file: "src/ipe-cli/src/env_dir.rs",
        func: "tool_home",
        reason: "callers pass `CARGO_HOME`/`RUSTUP_HOME` literals, which the literal rule scans",
    },
    Allowed {
        file: "src/ipe-cli/src/wasi_run.rs",
        func: "build_ctx",
        reason: "forwards the capability-granted names to the guest verbatim; derives no path",
    },
    Allowed {
        file: "src/ipe-cli/src/build_plan.rs",
        func: "preflight",
        reason: "reads `CC_<triple>`, a fixed prefix over a parsed target triple",
    },
    Allowed {
        file: "src/ipe-cli/src/watch.rs",
        func: "env_flag_on",
        reason: "callers pass flag-name literals, which the literal rule scans",
    },
    Allowed {
        file: "src/ipe-cli/src/ffi.rs",
        func: "jail_limits",
        reason: "reads the fixed `IPE_FFI_*` cap overrides named in its own body",
    },
    Allowed {
        file: "src/compiler/sandbox/src/build_jail.rs",
        func: "build_in_jail",
        reason: JAIL_HOST_ENV,
    },
    Allowed {
        file: "src/compiler/sandbox/src/run_jail/linux.rs",
        func: "exec_in_run_jail",
        reason: JAIL_HOST_ENV,
    },
    Allowed {
        file: "src/compiler/sandbox/src/run_jail/linux.rs",
        func: "exec_embedded_in_run_jail",
        reason: JAIL_HOST_ENV,
    },
    Allowed {
        file: "src/compiler/sandbox/src/run_jail/macos.rs",
        func: "exec_in_run_jail",
        reason: JAIL_HOST_ENV,
    },
    Allowed {
        file: "src/compiler/sandbox/src/run_jail/windows.rs",
        func: "run_confined",
        reason: JAIL_HOST_ENV,
    },
    Allowed {
        file: "src/runtime/rust/src/system.rs",
        func: "read_env_var",
        reason: "the runtime's overlay-aware reader; every caller's key is scanned at its site",
    },
    Allowed {
        file: "src/runtime/rust/src/system.rs",
        func: "read_env_var_os",
        reason: "the runtime's overlay-aware reader; every caller's key is scanned at its site",
    },
    Allowed {
        file: "src/runtime/rust/src/system.rs",
        func: "locked_set_var_if_absent",
        reason: "a presence probe before a default write; returns no value",
    },
    Allowed {
        file: "src/runtime/rust/src/system.rs",
        func: "system_getenv",
        reason: PROGRAM_GETENV,
    },
    Allowed {
        file: "src/runtime/rust/src/system.rs",
        func: "system_getenv_or",
        reason: PROGRAM_GETENV,
    },
    Allowed {
        file: "src/runtime/rust/src/system.rs",
        func: "system_getenv_int",
        reason: PROGRAM_GETENV,
    },
    Allowed {
        file: "src/runtime/rust/src/system.rs",
        func: "system_getenv_bool",
        reason: PROGRAM_GETENV,
    },
    Allowed {
        file: "src/runtime/rust/src/app_config.rs",
        func: "ipe_app_from_env",
        reason: "reads the program's declared config variable, typed as a `Secret`",
    },
    Allowed {
        file: "src/runtime/rust/src/app_config.rs",
        func: "ipe_app_from_env_required",
        reason: "reads the program's declared config variable, typed as a `Secret`",
    },
    Allowed {
        file: "src/runtime/rust/src/email.rs",
        func: "email_endpoint",
        reason: "reads a fixed per-provider endpoint override name",
    },
    Allowed {
        file: "src/runtime/rust/src/web/mod.rs",
        func: "num",
        reason: "callers pass server-limit literals, which the literal rule scans",
    },
];

/// Whitespace-free code fragments that read the home or the whole environment.
const RAW_HOME_CALLS: &[&str] = &["env::home_dir", "dirs::home_dir", "env::vars"];

/// One production file, lexed.
struct Lexed {
    /// The source with comments and literal bodies blanked to spaces; byte
    /// offsets match the original.
    code: Vec<u8>,
    /// Each string literal's starting offset and decoded text.
    literals: Vec<(usize, String)>,
}

/// Whether `b` can continue an identifier.
const fn ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

/// The byte at `i`, or `0` past the end.
fn at(bytes: &[u8], i: usize) -> u8 {
    bytes.get(i).copied().unwrap_or(0)
}

/// The byte before `i`, or `0` at the start.
fn before(bytes: &[u8], i: usize) -> u8 {
    i.checked_sub(1).map_or(0, |k| at(bytes, k))
}

/// Blank `range` of `code` to spaces, keeping line breaks.
fn blank(code: &mut [u8], range: Range<usize>) {
    for b in code.get_mut(range).into_iter().flatten() {
        if *b != b'\n' {
            *b = b' ';
        }
    }
}

/// Decode the escapes of a non-raw string body.
fn unescape(body: &str) -> String {
    let mut out = String::new();
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('0') => out.push('\0'),
            Some('x') => {
                let hex: String = chars.by_ref().take(2).collect();
                if let Some(ch) = u8::from_str_radix(&hex, 16).ok().map(char::from) {
                    out.push(ch);
                }
            }
            Some('u') => {
                let hex: String = chars
                    .by_ref()
                    .skip_while(|c| *c == '{')
                    .take_while(|c| *c != '}')
                    .collect();
                if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                    out.push(ch);
                }
            }
            Some('\n') => while chars.next_if(|c| c.is_whitespace()).is_some() {},
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// The offset of `needle` in `src` at or after `from`, or `src`'s length.
fn find_from(src: &str, from: usize, needle: &str) -> usize {
    src.get(from..)
        .and_then(|rest| rest.find(needle))
        .map_or(src.len(), |n| from + n)
}

/// Lex `src`: drop comments, record and blank string literals, and blank
/// character literals so a quoted bracket or quote cannot desynchronize the
/// scan.
fn lex(src: &str) -> Lexed {
    let bytes = src.as_bytes();
    let mut code = bytes.to_vec();
    let mut literals = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = at(bytes, i);
        if b == b'/' && at(bytes, i + 1) == b'/' {
            let end = find_from(src, i, "\n");
            blank(&mut code, i..end);
            i = end;
        } else if b == b'/' && at(bytes, i + 1) == b'*' {
            let start = i;
            let mut depth = 0usize;
            while i < bytes.len() {
                if at(bytes, i) == b'/' && at(bytes, i + 1) == b'*' {
                    depth += 1;
                    i += 2;
                } else if at(bytes, i) == b'*' && at(bytes, i + 1) == b'/' {
                    depth = depth.saturating_sub(1);
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            blank(&mut code, start..i);
        } else if b == b'r' && matches!(at(bytes, i + 1), b'"' | b'#') && raw_prefix(bytes, i) {
            let mut j = i + 1;
            while at(bytes, j) == b'#' {
                j += 1;
            }
            if at(bytes, j) != b'"' {
                i = j;
                continue;
            }
            let closer: String = std::iter::once('"')
                .chain(std::iter::repeat_n('#', j - i - 1))
                .collect();
            let body_end = find_from(src, j + 1, &closer);
            let body = src.get(j + 1..body_end).unwrap_or("");
            literals.push((i, body.to_owned()));
            blank(&mut code, j + 1..body_end);
            i = body_end + closer.len();
        } else if b == b'"' {
            let mut j = i + 1;
            while j < bytes.len() && at(bytes, j) != b'"' {
                j += if at(bytes, j) == b'\\' { 2 } else { 1 };
            }
            let body_end = j.min(bytes.len());
            let body = src.get(i + 1..body_end).unwrap_or("");
            literals.push((i, unescape(body)));
            blank(&mut code, i + 1..body_end);
            i = body_end + 1;
        } else if b == b'\'' && char_quote(bytes, i) {
            i = skip_char_literal(src, &mut code, i);
        } else {
            i += 1;
        }
    }
    Lexed { code, literals }
}

/// Whether the `r` at `i` opens a raw string (`r"`, `br"`, `cr"`) rather than
/// ending an identifier.
fn raw_prefix(bytes: &[u8], i: usize) -> bool {
    let prev = before(bytes, i);
    !ident_byte(prev)
        || (matches!(prev, b'b' | b'c')
            && !ident_byte(i.checked_sub(1).map_or(0, |k| before(bytes, k))))
}

/// Whether the quote at `i` can open a character literal (`'x'`, `b'x'`)
/// rather than sit inside an identifier.
fn char_quote(bytes: &[u8], i: usize) -> bool {
    let prev = before(bytes, i);
    !ident_byte(prev)
        || (prev == b'b' && !ident_byte(i.checked_sub(1).map_or(0, |k| before(bytes, k))))
}

/// Skip a character literal starting at `i` (blanking it), or step past a
/// lifetime's quote.
fn skip_char_literal(src: &str, code: &mut [u8], i: usize) -> usize {
    let bytes = src.as_bytes();
    if at(bytes, i + 1) == b'\\' {
        let end = find_from(src, i + 3, "'");
        blank(code, i + 1..end);
        return end + 1;
    }
    let width = src
        .get(i + 1..)
        .and_then(|rest| rest.chars().next())
        .map_or(1, char::len_utf8);
    if at(bytes, i + 1 + width) == b'\'' {
        blank(code, i + 1..i + 1 + width);
        return i + 2 + width;
    }
    i + 1
}

/// The index just past the bracket that closes the one at `open`.
fn close_of(code: &[u8], open: usize) -> usize {
    let mut depth = 0usize;
    for (k, b) in code.iter().enumerate().skip(open) {
        match b {
            b'{' | b'(' | b'[' => depth += 1,
            b'}' | b')' | b']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return k + 1;
                }
            }
            _ => {}
        }
    }
    code.len()
}

/// The end of the item starting at `from`: its matching `}` when a body opens
/// before any top-level `;`, else just past that `;`.
fn item_end(code: &[u8], from: usize) -> usize {
    let mut k = from;
    while k < code.len() {
        match at(code, k) {
            b'(' | b'[' => k = close_of(code, k),
            b'{' => return close_of(code, k),
            b';' => return k + 1,
            _ => k += 1,
        }
    }
    code.len()
}

/// The index just past whitespace starting at `k`.
fn skip_ws(code: &[u8], mut k: usize) -> usize {
    while at(code, k).is_ascii_whitespace() {
        k += 1;
    }
    k
}

/// The index of the last non-whitespace byte before `k`, plus one.
fn skip_ws_back(code: &[u8], mut k: usize) -> usize {
    while k > 0 && before(code, k).is_ascii_whitespace() {
        k -= 1;
    }
    k
}

/// Whether `code` spells `word` as a whole token at `k`.
fn token_at(code: &[u8], k: usize, word: &str) -> bool {
    let end = k + word.len();
    code.get(k..end) == Some(word.as_bytes())
        && !ident_byte(before(code, k))
        && !ident_byte(at(code, end))
}

/// The index past the whitespace-separated tokens `words` at `k`, if they match.
fn tokens_at(code: &[u8], k: usize, words: &[&str]) -> Option<usize> {
    let mut p = k;
    for word in words {
        p = skip_ws(code, p);
        if code.get(p..p + word.len()) != Some(word.as_bytes()) {
            return None;
        }
        p += word.len();
    }
    Some(p)
}

/// The byte ranges of items under an exact `#[cfg(test)]` (attribute included),
/// and the rest of the file after an inner `#![cfg(test)]`.
fn test_item_ranges(code: &[u8]) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut k = 0;
    while k < code.len() {
        if at(code, k) != b'#' {
            k += 1;
            continue;
        }
        if tokens_at(code, k + 1, &["!", "[", "cfg", "(", "test", ")", "]"]).is_some() {
            out.push(k..code.len());
            break;
        }
        let Some(mut p) = tokens_at(code, k + 1, &["[", "cfg", "(", "test", ")", "]"]) else {
            k += 1;
            continue;
        };
        p = skip_ws(code, p);
        while at(code, p) == b'#' {
            p = skip_ws(code, close_of(code, skip_ws(code, p + 1)));
        }
        let end = item_end(code, p);
        out.push(k..end);
        k = end;
    }
    out
}

/// The byte range of every `fn <name>` item in `code`.
fn fn_ranges(code: &[u8], name: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut k = 0;
    while k < code.len() {
        if token_at(code, k, "fn") {
            let p = skip_ws(code, k + 2);
            if token_at(code, p, name) {
                let end = item_end(code, p);
                out.push(k..end);
                k = end;
                continue;
            }
        }
        k += 1;
    }
    out
}

/// A refused construct: its line and what it is.
#[derive(Debug, PartialEq, Eq)]
struct Hit {
    line: usize,
    what: String,
}

/// The 1-based line of byte offset `offset` in `code`.
fn line_of(code: &[u8], offset: usize) -> usize {
    code.get(..offset)
        .map_or(0, |pre| pre.iter().filter(|b| **b == b'\n').count())
        + 1
}

/// Whether the call argument starting at `p` is a literal or a
/// `SCREAMING_CASE` constant path, rather than a computed key.
fn key_is_static(code: &[u8], p: usize) -> bool {
    let mut p = skip_ws(code, p);
    if at(code, p) == b'&' {
        p = skip_ws(code, p + 1);
    }
    if at(code, p) == b'"' || (at(code, p) == b'r' && matches!(at(code, p + 1), b'"' | b'#')) {
        return true;
    }
    let mut end = p;
    while ident_byte(at(code, end)) || at(code, end) == b':' {
        end += 1;
    }
    if !matches!(at(code, skip_ws(code, end)), b')' | b',') {
        return false;
    }
    let path = code.get(p..end).unwrap_or(&[]);
    let last = path.rsplit(|b| *b == b':').next().unwrap_or(&[]);
    !last.is_empty()
        && last
            .iter()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || *b == b'_')
}

/// The path segment ending just before a `::` that ends at `k`, if any.
fn owner_before(code: &[u8], k: usize) -> Option<&[u8]> {
    let colons = skip_ws_back(code, k);
    if colons < 2 || before(code, colons) != b':' || before(code, colons - 1) != b':' {
        return None;
    }
    let end = skip_ws_back(code, colons - 2);
    let mut start = end;
    while start > 0 && ident_byte(before(code, start)) {
        start -= 1;
    }
    code.get(start..end)
}

/// Whether the token at `k` is a std environment reader: `var`/`var_os` under
/// `env::`, or the runtime's `read_env_var`/`read_env_var_os` called bare or
/// under `system::` — never a method, a definition, or another type's path.
fn is_env_reader(code: &[u8], k: usize, name: &str) -> bool {
    let owner = owner_before(code, k);
    match name {
        "var" | "var_os" => owner == Some(b"env".as_slice()),
        _ => {
            let prev_end = skip_ws_back(code, k);
            let prev_start = {
                let mut s = prev_end;
                while s > 0 && ident_byte(before(code, s)) {
                    s -= 1;
                }
                s
            };
            let prev_word = code.get(prev_start..prev_end).unwrap_or(&[]);
            match owner {
                Some(owner) => owner == b"system",
                None => before(code, prev_end) != b'.' && prev_word != b"fn",
            }
        }
    }
}

/// Every refused construct in `src`, outside `#[cfg(test)]` items and outside
/// the named exempt functions for each rule.
fn raw_home_reads(src: &str, literal_ok: &[&str], dynamic_ok: &[&str]) -> Vec<Hit> {
    let Lexed { mut code, literals } = lex(src);
    let tests = test_item_ranges(&code);
    let in_any =
        |ranges: &[Range<usize>], offset: usize| ranges.iter().any(|r| r.contains(&offset));
    let literal_exempt: Vec<Range<usize>> = literal_ok
        .iter()
        .flat_map(|n| fn_ranges(&code, n))
        .collect();
    let dynamic_exempt: Vec<Range<usize>> = dynamic_ok
        .iter()
        .flat_map(|n| fn_ranges(&code, n))
        .collect();
    let mut hits = Vec::new();

    for (start, text) in &literals {
        if HOME_NAMES.contains(&text.as_str())
            && !in_any(&tests, *start)
            && !in_any(&literal_exempt, *start)
        {
            hits.push(Hit {
                line: line_of(&code, *start),
                what: format!("literal {text:?}"),
            });
        }
    }

    for range in &tests {
        blank(&mut code, range.clone());
    }

    let flat: Vec<(usize, u8)> = code
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, b)| !b.is_ascii_whitespace())
        .collect();
    let flat_bytes: Vec<u8> = flat.iter().map(|(_, b)| *b).collect();
    for needle in RAW_HOME_CALLS {
        for (k, window) in flat_bytes.windows(needle.len()).enumerate() {
            if window == needle.as_bytes() {
                let offset = flat.get(k).map_or(0, |(o, _)| *o);
                hits.push(Hit {
                    line: line_of(&code, offset),
                    what: (*needle).to_owned(),
                });
            }
        }
    }

    for k in 0..code.len() {
        if token_at(&code, k, "env")
            && let Some(open) = tokens_at(&code, k + 3, &["::", "{"])
        {
            let group = code.get(open - 1..close_of(&code, open - 1)).unwrap_or(&[]);
            let imports_reader =
                (0..group.len()).any(|g| ["var", "var_os"].iter().any(|w| token_at(group, g, w)));
            if imports_reader {
                hits.push(Hit {
                    line: line_of(&code, k),
                    what: "`env::{..}` imports a reader".to_owned(),
                });
            }
        }
        for reader in ["var", "var_os", "read_env_var", "read_env_var_os"] {
            if !token_at(&code, k, reader) || !is_env_reader(&code, k, reader) {
                continue;
            }
            let open = skip_ws(&code, k + reader.len());
            let dynamic = if at(&code, open) == b'(' {
                !key_is_static(&code, open + 1)
            } else {
                matches!(reader, "var" | "var_os")
            };
            if dynamic && !in_any(&dynamic_exempt, k) {
                hits.push(Hit {
                    line: line_of(&code, k),
                    what: format!("computed key or reader value at `{reader}`"),
                });
            }
        }
    }
    hits
}

/// Recursively collect every production `.rs` file under `dir` into `out`.
///
/// Integration-test trees and build output are skipped: neither ships.
fn collect_production_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            let skipped = path
                .file_name()
                .is_some_and(|n| n == "tests" || n == "target");
            if !skipped {
                collect_production_rs(&path, out);
            }
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// The exempt function names `list` grants in workspace file `rel`.
fn exempt_in(list: &[Allowed], rel: &str) -> Vec<&'static str> {
    list.iter()
        .filter(|a| a.file == rel)
        .map(|a| a.func)
        .collect()
}

/// The workspace root.
fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn no_production_source_reads_the_home_directly() {
    let workspace = workspace();
    let mut files = Vec::new();
    collect_production_rs(&workspace.join("src"), &mut files);
    assert!(
        !files.is_empty(),
        "the scan found no sources; the walk root is wrong"
    );

    let mut offenders = Vec::new();
    for path in files {
        let rel = path
            .strip_prefix(&workspace)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let text = std::fs::read_to_string(&path);
        assert!(text.is_ok(), "unreadable production source `{rel}`");
        let Ok(text) = text else { continue };
        let hits = raw_home_reads(
            &text,
            &exempt_in(LITERAL_ALLOWED, &rel),
            &exempt_in(DYNAMIC_KEY_ALLOWED, &rel),
        );
        if !hits.is_empty() {
            offenders.push((rel, hits));
        }
    }

    assert!(
        offenders.is_empty(),
        "raw home reads found; use `ipe_sandbox::home::home_dir` (compiler) or \
         `system::home_dir` (runtime), or add a reasoned allowlist entry: {offenders:#?}"
    );
}

#[test]
fn every_allowlisted_function_exists_with_a_reason() {
    let workspace = workspace();
    for entry in LITERAL_ALLOWED.iter().chain(DYNAMIC_KEY_ALLOWED) {
        assert!(
            !entry.reason.trim().is_empty(),
            "`{}::{}` needs a reason",
            entry.file,
            entry.func
        );
        let text = std::fs::read_to_string(workspace.join(entry.file));
        assert!(
            text.is_ok(),
            "allowlisted file `{}` is gone; drop its entry",
            entry.file
        );
        let Ok(text) = text else { continue };
        assert!(
            !fn_ranges(&lex(&text).code, entry.func).is_empty(),
            "allowlisted `fn {}` is gone from `{}`; drop its entry",
            entry.func,
            entry.file
        );
    }
}

/// The refused constructs in a planted snippet, with no exemptions.
fn planted(src: &str) -> Vec<Hit> {
    raw_home_reads(src, &[], &[])
}

#[test]
fn a_planted_raw_home_read_is_detected() {
    let planted_reads = [
        "let h = std::env::var_os(\"HOME\");",
        "let h = std::env::var( \"HOME\" ).ok();",
        "let h = env::var_os(\n    \"USERPROFILE\",\n);",
        "let h = read_env_var_os(\"HOME\");",
        "#[allow(deprecated)] let h = std::env::home_dir();",
        "use std::env::home_dir;",
        "let h = dirs::home_dir();",
        "const K: &str = \"HOME\"; fn f() { let _ = std::env::var(K); }",
        "pub const HOME_VAR: &str = \"HOME\"; fn f() { std::env::var_os(HOME_VAR); }",
        "let h = std::env::var_os(r\"HOME\");",
        "let h = std::env::var_os(r#\"USERPROFILE\"#);",
        "let h = std::env::var_os(\"\\x48OME\");",
        "let h = std::env::var_os(\"\\u{48}OME\");",
        "cmd.env(\"HOME\", scratch);",
        "const H: &str = env!(\"HOME\");",
        "let h = option_env!(\"HOME\");",
        "let h = option_env!(r\"USERPROFILE\");",
        "for (k, v) in std::env::vars() {}",
        "for (k, v) in std::env::vars_os() {}",
        "use std::env::var_os;",
        "use std::env::{self, var};",
        "let h = keys.iter().map(std::env::var_os);",
        "fn f(key: &str) { let _ = std::env::var_os(key); }",
        "fn f(p: &Profile) { let _ = std::env::var(&p.key); }",
        "fn f() { let _ = crate::system::read_env_var(&format!(\"{}\", k)); }",
        "#[cfg(any(test, feature = \"x\"))]\nfn f() { let _ = std::env::var(\"HOME\"); }",
    ];
    for src in planted_reads {
        assert!(
            !planted(src).is_empty(),
            "the scan missed a raw home read: {src:?}"
        );
    }
}

#[test]
fn the_validated_accessors_and_unrelated_forms_are_not_flagged() {
    let clean = [
        "let h = ipe_sandbox::home::home_dir();",
        "let h = crate::env_dir::home();",
        "let v = std::env::var_os(\"HOMEBREW_PREFIX\");",
        "let v = std::env::var(REGISTRY_URL_ENV);",
        "let v = std::env::var_os(crate::REPLAY_ENV);",
        "let v = std::env::var(Self::ENV);",
        "// std::env::var_os(\"HOME\") in a comment",
        "/// `HOME` names the home: std::env::var_os(\"HOME\")",
        "/* \"HOME\" in a /* nested */ block comment */",
        "let t = Ty::var(id);",
        "let t = self.var(id);",
        "fn var(id: u32) {}",
        "fn read_env_var(key: &str) {}",
        "let css = \"color: var(--fg)\";",
        "let q = '\"'; let b = b'\"'; let v = std::env::var(\"PATH\");",
        "fn f<'a>(x: &'a str) -> &'a str { x }",
        "let s = r#\"a \"quoted\" HOME\"#;",
    ];
    for src in clean {
        assert_eq!(
            planted(src),
            Vec::new(),
            "the scan flagged a sanctioned form: {src:?}"
        );
    }
}

#[test]
fn test_items_are_skipped_but_production_code_beside_them_is_not() {
    let src = "#[cfg(test)]\nmod tests {\n    fn t() { let _ = std::env::var(\"HOME\"); }\n}\n\
               fn prod() { let _ = std::env::var(\"HOME\"); }\n";
    assert_eq!(
        planted(src),
        vec![Hit {
            line: 5,
            what: "literal \"HOME\"".to_owned(),
        }]
    );
    let inner = "#![cfg(test)]\nfn t() { let _ = std::env::var(\"HOME\"); }\n";
    assert_eq!(planted(inner), Vec::new());
}

#[test]
fn an_exemption_covers_its_function_only() {
    let src = "fn home_dir() { let _ = std::env::var_os(\"HOME\"); }\n\
               fn other() { let _ = std::env::var_os(\"HOME\"); }\n";
    assert_eq!(
        raw_home_reads(src, &["home_dir"], &[]),
        vec![Hit {
            line: 2,
            what: "literal \"HOME\"".to_owned(),
        }]
    );
    let dynamic = "fn reader(k: &str) { let _ = std::env::var(k); }\n\
                   fn stray(k: &str) { let _ = std::env::var(k); }\n";
    assert_eq!(
        raw_home_reads(dynamic, &[], &["reader"]),
        vec![Hit {
            line: 2,
            what: "computed key or reader value at `var`".to_owned(),
        }]
    );
    let literal_in_dynamic_reader = "fn reader() { let _ = std::env::var(\"HOME\"); }\n";
    assert_eq!(
        raw_home_reads(literal_in_dynamic_reader, &[], &["reader"]).len(),
        1,
        "a computed-key exemption never covers a home literal"
    );
}
