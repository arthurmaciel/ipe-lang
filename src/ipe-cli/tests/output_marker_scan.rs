#![forbid(unsafe_code)]
//! The ownership marker and the claim file are acted on only by the claim protocol.
//!
//! Every create, rename, unlink, open, or path join that names
//! `OWNERSHIP_MARKER`, `CLAIM_FILE`, or a `.ipe-output` literal lives in
//! `output_dir/held.rs`, which runs it under the claim lock. Outside it, a
//! non-test function that names either and also reaches a filesystem act fails
//! this test; the one allow-listed site is `HandoverDir::release_to_user`,
//! the hand-over to the user. No `fn adopt` or `fn write_marker` exists, and
//! `fn publish_marker` is never `pub`.
//!
//! The source is read with comments and string literals stripped, so a name in
//! a comment is not a use and a brace in a string does not shift the items.

use std::path::{Path, PathBuf};

/// The file whose functions run the claim protocol.
const PROTOCOL_FILE: &str = "output_dir/held.rs";

/// Non-test functions outside [`PROTOCOL_FILE`] allowed to act on a marker name, by file and name.
const ALLOWED: &[(&str, &str)] = &[("output_dir.rs", "release_to_user")];

/// The word a string literal holding `.ipe-output` is replaced by.
const MARKER_LITERAL: &str = "__ipe_marker_literal__";

/// The words that name the marker or the claim file.
const MARKER_WORDS: &[&str] = &["OWNERSHIP_MARKER", "CLAIM_FILE", MARKER_LITERAL];

/// The text of a filesystem act or of a path built from a name.
const FS_ACTS: &[&str] = &[
    "fs::",
    "File::",
    "OpenOptions",
    ".join(",
    ".unlink(",
    ".rename(",
    ".create_new(",
    ".create_claim(",
    ".open_claim(",
    ".open_file(",
    ".write_file(",
    ".remove_entry(",
];

/// Whether `byte` can be part of an identifier.
const fn is_ident(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// The identifier ending just before `at` in `bytes`.
fn ident_before(bytes: &[u8], at: usize) -> &[u8] {
    let head = bytes.get(..at).unwrap_or_default();
    let start = head
        .iter()
        .rposition(|&byte| !is_ident(byte))
        .map_or(0, |pos| pos.saturating_add(1));
    head.get(start..).unwrap_or_default()
}

/// Whether a raw string literal starts at `at` (an `r`, after nothing or a `b`/`c` prefix).
fn is_raw_start(bytes: &[u8], at: usize) -> bool {
    let prefix = ident_before(bytes, at);
    if !(prefix.is_empty() || prefix == b"b" || prefix == b"c") {
        return false;
    }
    let hashes = bytes
        .get(at.saturating_add(1)..)
        .unwrap_or_default()
        .iter()
        .take_while(|&&byte| byte == b'#')
        .count();
    bytes.get(at.saturating_add(1).saturating_add(hashes)) == Some(&b'"')
}

/// Skip the string literal starting at `at` and append its stand-in to `out`.
///
/// The stand-in is [`MARKER_LITERAL`] for a literal holding `.ipe-output`,
/// else an empty literal. Returns the index just past the literal.
fn skip_string(bytes: &[u8], at: usize, out: &mut Vec<u8>) -> usize {
    let mut i = at;
    let mut hashes = 0;
    let raw = bytes.get(i) == Some(&b'r');
    if raw {
        i = i.saturating_add(1);
        while bytes.get(i) == Some(&b'#') {
            hashes += 1;
            i = i.saturating_add(1);
        }
    }
    i = i.saturating_add(1);
    let start = i;
    let end = loop {
        match bytes.get(i) {
            None => break i,
            Some(b'\\') if !raw => i = i.saturating_add(2),
            Some(b'"') => {
                let close = bytes
                    .get(i.saturating_add(1)..)
                    .unwrap_or_default()
                    .iter()
                    .take_while(|&&byte| byte == b'#')
                    .count();
                if close >= hashes {
                    break i;
                }
                i = i.saturating_add(1);
            }
            Some(_) => i = i.saturating_add(1),
        }
    };
    let content = bytes.get(start..end).unwrap_or_default();
    if content
        .windows(b".ipe-output".len())
        .any(|w| w == b".ipe-output")
    {
        out.push(b' ');
        out.extend_from_slice(MARKER_LITERAL.as_bytes());
        out.push(b' ');
    } else {
        out.extend_from_slice(b"\"\"");
    }
    end.saturating_add(1).saturating_add(hashes)
}

/// Skip the char literal or lifetime starting at the quote `at`, appending its stand-in to `out`.
fn skip_quote(src: &str, at: usize, out: &mut Vec<u8>) -> usize {
    let bytes = src.as_bytes();
    let after = at.saturating_add(1);
    if bytes.get(after) == Some(&b'\\') {
        let from = after.saturating_add(2);
        let close = bytes
            .get(from..)
            .unwrap_or_default()
            .iter()
            .position(|&byte| byte == b'\'')
            .map_or(bytes.len(), |pos| from.saturating_add(pos));
        out.extend_from_slice(b"' '");
        return close.saturating_add(1);
    }
    let width = src
        .get(after..)
        .and_then(|rest| rest.chars().next())
        .map_or(0, char::len_utf8);
    if width > 0 && bytes.get(after.saturating_add(width)) == Some(&b'\'') {
        out.extend_from_slice(b"' '");
        return after.saturating_add(width).saturating_add(1);
    }
    out.push(b'\'');
    after
}

/// `src` with every comment blanked and every string or char literal replaced by a stand-in.
fn strip(src: &str) -> String {
    let bytes = src.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&byte) = bytes.get(i) {
        let next = bytes.get(i.saturating_add(1)).copied();
        match (byte, next) {
            (b'/', Some(b'/')) => {
                while let Some(&byte) = bytes.get(i) {
                    if byte == b'\n' {
                        break;
                    }
                    out.push(b' ');
                    i = i.saturating_add(1);
                }
            }
            (b'/', Some(b'*')) => {
                let mut depth = 0_usize;
                while let Some(&byte) = bytes.get(i) {
                    let next = bytes.get(i.saturating_add(1)).copied();
                    if byte == b'/' && next == Some(b'*') {
                        depth = depth.saturating_add(1);
                        out.extend_from_slice(b"  ");
                        i = i.saturating_add(2);
                    } else if byte == b'*' && next == Some(b'/') {
                        depth = depth.saturating_sub(1);
                        out.extend_from_slice(b"  ");
                        i = i.saturating_add(2);
                        if depth == 0 {
                            break;
                        }
                    } else {
                        out.push(if byte == b'\n' { b'\n' } else { b' ' });
                        i = i.saturating_add(1);
                    }
                }
            }
            (b'"', _) => i = skip_string(bytes, i, &mut out),
            (b'r', _) if is_raw_start(bytes, i) => i = skip_string(bytes, i, &mut out),
            (b'\'', _) => i = skip_quote(src, i, &mut out),
            _ => {
                out.push(byte);
                i = i.saturating_add(1);
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The index of the bracket closing the one at `open` in `code`, or the end of `code`.
fn matching(code: &[u8], open: usize) -> usize {
    let (up, down) = match code.get(open) {
        Some(b'[') => (b'[', b']'),
        Some(b'(') => (b'(', b')'),
        _ => (b'{', b'}'),
    };
    let mut depth = 0_usize;
    for (i, &byte) in code.iter().enumerate().skip(open) {
        if byte == up {
            depth = depth.saturating_add(1);
        } else if byte == down {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return i;
            }
        }
    }
    code.len()
}

/// The end of the item starting at `from`: its closing brace, or its `;`.
fn item_end(code: &[u8], from: usize) -> usize {
    let mut i = from;
    while let Some(&byte) = code.get(i) {
        match byte {
            b'(' | b'[' => i = matching(code, i).saturating_add(1),
            b'{' => return matching(code, i),
            b';' => return i,
            _ => i = i.saturating_add(1),
        }
    }
    code.len()
}

/// The byte ranges of `code` that only test builds compile: items under `#[test]` or `#[cfg(test)]`.
fn test_regions(code: &str) -> Vec<(usize, usize)> {
    let bytes = code.as_bytes();
    let mut regions = Vec::new();
    for (at, _) in code.match_indices("#[") {
        let open = at.saturating_add(1);
        let close = matching(bytes, open);
        let attr: String = code
            .get(open.saturating_add(1)..close)
            .unwrap_or_default()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        if attr == "test" || attr == "cfg(test)" || attr.starts_with("cfg(all(test,") {
            regions.push((at, item_end(bytes, close.saturating_add(1))));
        }
    }
    regions
}

/// A function of a source file: its name and the byte range from its `fn` to its body's end.
struct Function {
    name: String,
    span: (usize, usize),
}

/// Every function with a body in `code`, nested ones included.
fn functions(code: &str) -> Vec<Function> {
    let bytes = code.as_bytes();
    let mut found = Vec::new();
    for (at, _) in code.match_indices("fn") {
        let before = at.checked_sub(1).and_then(|i| bytes.get(i)).copied();
        let after = bytes.get(at.saturating_add(2)).copied();
        if before.is_some_and(is_ident) || !after.is_some_and(|b| b.is_ascii_whitespace()) {
            continue;
        }
        let rest = code.get(at.saturating_add(2)..).unwrap_or_default();
        let name: String = rest
            .trim_start()
            .chars()
            .take_while(|&c| c.is_alphanumeric() || c == '_')
            .collect();
        if name.is_empty() {
            continue;
        }
        let end = item_end(bytes, at);
        if bytes.get(end) == Some(&b'}') {
            found.push(Function {
                name,
                span: (at, end),
            });
        }
    }
    found
}

/// Whether `text` holds `word` with no identifier character on either side.
fn has_word(text: &str, word: &str) -> bool {
    let bytes = text.as_bytes();
    text.match_indices(word).any(|(at, _)| {
        let before = at.checked_sub(1).and_then(|i| bytes.get(i)).copied();
        let after = bytes.get(at.saturating_add(word.len())).copied();
        !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
    })
}

/// The non-test functions of `src` that name a marker and reach a filesystem act.
fn marker_acts(src: &str) -> Vec<String> {
    let code = strip(src);
    if code.contains("#![cfg(test)]") {
        return Vec::new();
    }
    let regions = test_regions(&code);
    functions(&code)
        .into_iter()
        .filter(|function| {
            !regions
                .iter()
                .any(|&(start, end)| start <= function.span.0 && function.span.0 <= end)
        })
        .filter(|function| {
            let body = code
                .get(function.span.0..=function.span.1)
                .unwrap_or_default();
            MARKER_WORDS.iter().any(|word| has_word(body, word))
                && FS_ACTS.iter().any(|act| body.contains(act))
        })
        .map(|function| function.name)
        .collect()
}

/// The banned marker writers `src` declares: `fn adopt`, `fn write_marker`, or a `pub fn publish_marker`.
fn banned_writers(src: &str) -> Vec<String> {
    let code = strip(src);
    functions(&code)
        .into_iter()
        .filter_map(|function| {
            let head = code.get(..function.span.0).unwrap_or_default();
            let qualifiers = head
                .rfind(['\n', ';', '{', '}'])
                .and_then(|at| head.get(at.saturating_add(1)..))
                .unwrap_or(head);
            let public = qualifiers
                .split_whitespace()
                .any(|word| word.starts_with("pub"));
            match function.name.as_str() {
                "adopt" | "write_marker" => Some(function.name),
                "publish_marker" if public => Some(function.name),
                _ => None,
            }
        })
        .collect()
}

/// Every `.rs` file under `root` that a non-test build compiles, relative to `root` with `/` separators.
fn production_files(root: &Path) -> Vec<(String, PathBuf)> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|e| e == "rs")
                && let Ok(rel) = path.strip_prefix(root)
            {
                let parts: Vec<String> = rel
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect();
                let test_only = parts.iter().any(|part| {
                    part == "tests" || part == "tests.rs" || part.ends_with("_tests.rs")
                });
                if !test_only {
                    found.push((parts.join("/"), path));
                }
            }
        }
    }
    found.sort();
    found
}

#[test]
fn marker_and_claim_file_acts_stay_in_the_claim_protocol() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let files = production_files(&root);
    assert!(
        files.iter().any(|(rel, _)| rel == PROTOCOL_FILE),
        "the protocol file `{PROTOCOL_FILE}` is scanned from `{}`",
        root.display()
    );
    let mut violations = Vec::new();
    let mut allowed_seen = Vec::new();
    for (rel, path) in &files {
        let src = std::fs::read_to_string(path).expect("read source file");
        for writer in banned_writers(&src) {
            violations.push(format!("{rel}: `fn {writer}` is banned"));
        }
        if rel == PROTOCOL_FILE {
            continue;
        }
        for name in marker_acts(&src) {
            if ALLOWED.contains(&(rel.as_str(), name.as_str())) {
                allowed_seen.push((rel.clone(), name));
            } else {
                violations.push(format!(
                    "{rel}: `fn {name}` acts on the marker or claim file outside `{PROTOCOL_FILE}`"
                ));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "marker acts outside the claim protocol:\n{}",
        violations.join("\n")
    );
    for &(file, name) in ALLOWED {
        assert!(
            allowed_seen
                .iter()
                .any(|(rel, seen)| rel == file && seen == name),
            "the allow-listed `{file}::{name}` no longer acts on the marker; drop it from `ALLOWED`"
        );
    }
}

#[test]
fn a_planted_marker_act_is_caught() {
    let planted = r#"
        use std::path::Path;
        fn braces() -> &'static str { "}{" }
        fn forge(dir: &Path) {
            let _ = std::fs::write(dir.join(OWNERSHIP_MARKER), "x");
        }
        fn forge_claim(dir: &Path) {
            let _ = std::fs::remove_file(dir.join(".ipe-output.claim"));
        }
        fn char_brace(c: char) -> bool { c == '{' }
        fn raw_forge(dir: &Path) {
            let _ = dir.unlink(r"x/.ipe-output");
        }
    "#;
    assert_eq!(
        marker_acts(planted),
        ["forge", "forge_claim", "raw_forge"],
        "each planted act is caught"
    );
}

#[test]
fn test_code_comments_and_plain_acts_are_not_marker_acts() {
    let clean = r#"
        fn display() -> String { format!("{}", OWNERSHIP_MARKER) }
        fn commented(dir: &Path) {
            // std::fs::write(dir.join(OWNERSHIP_MARKER), "x");
            /* dir.unlink(CLAIM_FILE) */
            let _ = std::fs::create_dir(dir);
        }
        #[cfg(test)]
        mod tests {
            fn plant(dir: &Path) {
                let _ = std::fs::write(dir.join(OWNERSHIP_MARKER), "x");
            }
        }
        #[test]
        fn planted() {
            let _ = std::fs::write(Path::new(".ipe-output"), "x");
        }
    "#;
    assert!(
        marker_acts(clean).is_empty(),
        "got {:?}",
        marker_acts(clean)
    );
}

#[test]
fn a_banned_marker_writer_is_caught() {
    let banned = "
        impl HeldDir {
            pub fn adopt(&self) {}
            fn write_marker(&self) {}
            pub(crate) fn publish_marker(&self) {}
        }
        impl Other {
            pub fn publish_marker(&self) {}
        }
    ";
    assert_eq!(
        banned_writers(banned),
        ["adopt", "write_marker", "publish_marker", "publish_marker"]
    );
    let private = "impl HeldDir { fn publish_marker(&self) {} }";
    assert!(
        banned_writers(private).is_empty(),
        "a private publish is the protocol's"
    );
}
