#![forbid(unsafe_code)]
//! Pins the runtime's lenient decoders, a second layer beneath the clippy deny.
//!
//! Every URL component the runtime reads is decoded by the one strict core,
//! `encoding::decode_component` / `encoding::decode_form_query`. The runtime
//! `clippy.toml` denies the lenient percent decoders, the lenient query readers,
//! axum's lenient `Query`/`Form` extractors and lossy UTF-8; this scan pins that
//! set independently: no runtime source names a lenient percent decoder or a
//! lenient extractor at all, and each lenient query reader and lossy UTF-8
//! conversion appears only at the inventoried sites below.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The runtime crate, relative to the workspace root.
const RUNTIME_ROOT: &str = "src/runtime/rust";

/// Spellings of the lenient percent decoders, refused in every runtime source.
const LENIENT_PERCENT_DECODERS: &[&str] = &["percent_decode", "decode_utf8_lossy"];

/// Spellings of axum's lenient request extractors, refused in every runtime
/// source.
const LENIENT_EXTRACTORS: &[&str] = &["extract::Query", "extract::Form"];

/// The extractor names refused inside an `extract::{…}` import group.
const LENIENT_EXTRACTOR_NAMES: &[&str] = &["Query", "Form"];

/// The lossy UTF-8 conversion.
const LOSSY_UTF8: &str = "from_utf8_lossy";

/// Every site that may convert bytes to text lossily, as (runtime-relative
/// file, number of sites), each under a per-site `clippy::disallowed_methods`
/// allow naming its reason.
const LOSSY_TEXT_SITES: &[(&str, usize)] = &[
    // CSV writer output, fed only `String` fields.
    ("src/csv.rs", 1),
    // An email provider's response body.
    ("src/email.rs", 1),
    // HTTP response bodies.
    ("src/http_client.rs", 2),
    // Streamed HTTP response chunks.
    ("src/http_stream.rs", 2),
    // A request body and a WebSocket binary frame.
    ("src/server.rs", 2),
    // Subprocess output.
    ("src/system.rs", 4),
    // Terminal input bytes.
    ("src/tui/key.rs", 1),
    // A test's SSE reader, whose chunks may split a UTF-8 sequence.
    ("src/web/mod.rs", 1),
];

/// Spellings of the lenient query readers.
const LENIENT_QUERY_READERS: &[&str] = &["query_pairs", "form_urlencoded::parse"];

/// Every site that may read a query leniently, as (runtime-relative file,
/// number of sites).
const LENIENT_QUERY_SITES: &[(&str, usize)] = &[
    // `ssrf::DriverParityQuery`: a database URL read exactly as its driver
    // reads it, so the SSRF gate vets the host the driver dials.
    ("src/ssrf.rs", 1),
    // A test oracle inverting the query serializer.
    ("src/url.rs", 1),
];

/// The clippy paths the runtime `clippy.toml` must deny.
const DENIED_PATHS: &[&str] = &[
    "percent_encoding::percent_decode_str",
    "percent_encoding::percent_decode",
    "percent_encoding::PercentDecode::decode_utf8_lossy",
    "std::string::String::from_utf8_lossy",
    "url::form_urlencoded::parse",
    "url::Url::query_pairs",
    "axum::extract::Query",
    "axum::extract::Form",
];

/// The runtime crate's directory.
fn runtime() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(RUNTIME_ROOT)
}

/// Whether `c` can continue an identifier.
fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The end of the line comment starting at `i`: the index of its line break.
fn skip_line_comment(chars: &[char], mut i: usize) -> usize {
    while chars.get(i).is_some_and(|&c| c != '\n') {
        i += 1;
    }
    i
}

/// The index past the (nested) block comment opening at `i`, pushing its line
/// breaks to `out`.
fn skip_block_comment(chars: &[char], mut i: usize, out: &mut String) -> usize {
    let mut depth = 0_usize;
    while let Some(&c) = chars.get(i) {
        let next = chars.get(i + 1).copied();
        if c == '/' && next == Some('*') {
            depth += 1;
            i += 2;
        } else if c == '*' && next == Some('/') {
            depth = depth.saturating_sub(1);
            i += 2;
            if depth == 0 {
                break;
            }
        } else {
            if c == '\n' {
                out.push('\n');
            }
            i += 1;
        }
    }
    i
}

/// The index past the literal whose body starts at `i` and ends at `close`,
/// honouring `\` escapes unless `raw`, pushing its line breaks to `out`.
fn skip_literal_body(
    chars: &[char],
    mut i: usize,
    close: char,
    hashes: usize,
    raw: bool,
    out: &mut String,
) -> usize {
    while let Some(&c) = chars.get(i) {
        if c == '\n' {
            out.push('\n');
        }
        if !raw && c == '\\' {
            if chars.get(i + 1) == Some(&'\n') {
                out.push('\n');
            }
            i += 2;
            continue;
        }
        i += 1;
        if c == close && (0..hashes).all(|k| chars.get(i + k) == Some(&'#')) {
            return i + hashes;
        }
    }
    i
}

/// The raw-string opener at `i` (`r"`, `r#"`, `br"`, `cr#"`, ...): the index of
/// its body and its number of `#`s.
fn raw_string_open(chars: &[char], i: usize) -> Option<(usize, usize)> {
    let mut j = i;
    if matches!(chars.get(j), Some('b' | 'c')) {
        j += 1;
    }
    if chars.get(j) != Some(&'r') {
        return None;
    }
    j += 1;
    let mut hashes = 0;
    while chars.get(j) == Some(&'#') {
        hashes += 1;
        j += 1;
    }
    (chars.get(j) == Some(&'"')).then_some((j + 1, hashes))
}

/// Whether the `'` at `i` opens a character literal rather than a lifetime.
fn opens_char_literal(chars: &[char], i: usize) -> bool {
    chars.get(i + 1) == Some(&'\\') || chars.get(i + 2) == Some(&'\'')
}

/// The code of `src`: every comment dropped and every string, byte-string and
/// character literal's contents blanked, line breaks kept.
fn code_of(src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        let next = chars.get(i + 1).copied();
        let after_ident = i
            .checked_sub(1)
            .and_then(|p| chars.get(p))
            .is_some_and(|&p| is_ident(p));
        if c == '/' && next == Some('/') {
            i = skip_line_comment(&chars, i);
        } else if c == '/' && next == Some('*') {
            out.push(' ');
            i = skip_block_comment(&chars, i, &mut out);
        } else if let Some((body, hashes)) = raw_string_open(&chars, i).filter(|_| !after_ident) {
            out.push_str("\"\"");
            i = skip_literal_body(&chars, body, '"', hashes, true, &mut out);
        } else if c == '"' || (matches!(c, 'b' | 'c') && next == Some('"') && !after_ident) {
            let body = if c == '"' { i + 1 } else { i + 2 };
            out.push_str("\"\"");
            i = skip_literal_body(&chars, body, '"', 0, false, &mut out);
        } else if c == 'b' && next == Some('\'') && !after_ident {
            out.push_str("''");
            i = skip_literal_body(&chars, i + 2, '\'', 0, false, &mut out);
        } else if c == '\'' && opens_char_literal(&chars, i) {
            out.push_str("''");
            i = skip_literal_body(&chars, i + 1, '\'', 0, false, &mut out);
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// Every `.rs` file under `dir`, as `(runtime-relative path, text)`.
fn rust_sources(dir: &Path, base: &Path, out: &mut Vec<(String, String)>) {
    let entries = std::fs::read_dir(dir);
    assert!(entries.is_ok(), "read {}: {entries:?}", dir.display());
    let Ok(entries) = entries else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let hidden = entry.file_name().to_string_lossy().starts_with('.');
        if hidden {
            continue;
        }
        if path.is_dir() {
            rust_sources(&path, base, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let text = std::fs::read_to_string(&path);
            assert!(text.is_ok(), "read {}: {text:?}", path.display());
            let Ok(text) = text else { continue };
            let rel = path
                .strip_prefix(base)
                .map_or_else(|_| path.clone(), Path::to_path_buf);
            let rel = rel.to_string_lossy().replace('\\', "/");
            out.push((rel, text));
        }
    }
}

/// Every runtime source's code, crate sources and integration tests alike.
fn runtime_code() -> Vec<(String, String)> {
    let root = runtime();
    let mut out = Vec::new();
    rust_sources(&root.join("src"), &root, &mut out);
    rust_sources(&root.join("tests"), &root, &mut out);
    assert!(
        out.iter().any(|(rel, _)| rel == "src/encoding.rs"),
        "the scan must reach the decoder core; scanned {} files",
        out.len()
    );
    out.into_iter()
        .map(|(rel, text)| (rel, code_of(&text)))
        .collect()
}

/// Whether `src` names a lenient percent decoder in code.
fn names_lenient_percent_decoder(src: &str) -> bool {
    let code = code_of(src);
    LENIENT_PERCENT_DECODERS
        .iter()
        .any(|name| code.contains(name))
}

/// Whether `code` names `word` as a whole identifier.
fn names_word(code: &str, word: &str) -> bool {
    code.match_indices(word).any(|(at, _)| {
        let before = code.get(..at).and_then(|s| s.chars().next_back());
        let after = code.get(at + word.len()..).and_then(|s| s.chars().next());
        !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
    })
}

/// Whether `src` names a lenient axum extractor in code, by path or through an
/// `extract::{…}` import group.
fn names_lenient_extractor(src: &str) -> bool {
    let code = code_of(src);
    let by_path = LENIENT_EXTRACTORS
        .iter()
        .any(|path| names_word(&code, path));
    let by_group = code.match_indices("extract::{").any(|(at, open)| {
        let group = code.get(at + open.len()..).unwrap_or("");
        let group = group.split('}').next().unwrap_or("");
        LENIENT_EXTRACTOR_NAMES
            .iter()
            .any(|name| names_word(group, name))
    });
    by_path || by_group
}

/// The number of sites naming any of `spellings`, per runtime-relative file,
/// omitting files with none.
fn inventory(code: &[(String, String)], spellings: &[&str]) -> BTreeMap<String, usize> {
    code.iter()
        .filter_map(|(rel, code)| {
            let sites: usize = spellings
                .iter()
                .map(|spelling| code.matches(spelling).count())
                .sum();
            (sites > 0).then(|| (rel.clone(), sites))
        })
        .collect()
}

/// `pins` as an inventory.
fn pinned(pins: &[(&str, usize)]) -> BTreeMap<String, usize> {
    pins.iter()
        .map(|&(rel, sites)| (rel.to_owned(), sites))
        .collect()
}

#[test]
fn no_runtime_source_names_a_lenient_percent_decoder() {
    let offenders: Vec<_> = runtime_code()
        .into_iter()
        .filter(|(_, code)| names_lenient_percent_decoder(code))
        .map(|(rel, _)| rel)
        .collect();
    assert!(
        offenders.is_empty(),
        "a lenient percent decoder in the runtime; decode a URL component through \
         `encoding::decode_component`: {offenders:?}"
    );
}

#[test]
fn no_runtime_source_names_a_lenient_extractor() {
    let offenders: Vec<_> = runtime_code()
        .into_iter()
        .filter(|(_, code)| names_lenient_extractor(code))
        .map(|(rel, _)| rel)
        .collect();
    assert!(
        offenders.is_empty(),
        "a lenient axum `Query`/`Form` extractor in the runtime; read the query \
         through `server::strict_url_query`: {offenders:?}"
    );
}

#[test]
fn lossy_utf8_stays_at_its_pinned_sites() {
    assert_eq!(
        inventory(&runtime_code(), &[LOSSY_UTF8]),
        pinned(LOSSY_TEXT_SITES),
        "lossy UTF-8 must stay at its pinned sites: a URL component goes through \
         `encoding::decode_component`; other text needs a reasoned per-site allow \
         and a pin here"
    );
}

#[test]
fn lenient_query_readers_stay_at_their_pinned_sites() {
    assert_eq!(
        inventory(&runtime_code(), LENIENT_QUERY_READERS),
        pinned(LENIENT_QUERY_SITES),
        "a lenient query reader outside its pinned sites: decode a query through \
         `encoding::decode_form_query`, or a driver-parity read through \
         `ssrf::DriverParityQuery`"
    );
}

#[test]
fn the_lenient_decoders_are_denied_by_the_runtime_clippy_config() {
    let config = std::fs::read_to_string(runtime().join("clippy.toml"));
    assert!(config.is_ok(), "read the runtime clippy.toml: {config:?}");
    let Ok(config) = config else { return };
    let missing: Vec<_> = DENIED_PATHS
        .iter()
        .filter(|path| !config.contains(&format!("path = \"{path}\"")))
        .collect();
    assert!(
        missing.is_empty(),
        "the runtime clippy.toml must deny {missing:?}"
    );
}

#[test]
fn a_planted_lenient_decoder_is_detected() {
    assert!(names_lenient_percent_decoder(
        "let d = percent_encoding::percent_decode_str(raw).decode_utf8();"
    ));
    assert!(names_lenient_percent_decoder(
        "let d = p.decode_utf8_lossy();"
    ));
    // A `//` inside a string does not hide the call after it.
    assert!(names_lenient_percent_decoder(
        "let u = \"http://x\"; let d = percent_decode_str(u);"
    ));
    assert!(names_lenient_percent_decoder(
        "let u = r#\"a\"//b\"#; let d = percent_decode(u);"
    ));
    // A lifetime is not a character literal that could swallow code.
    assert!(names_lenient_percent_decoder(
        "fn f<'a>(s: &'a str) { percent_decode(s) }"
    ));
    assert!(names_lenient_extractor(
        "async fn h(axum::extract::Query(q): axum::extract::Query<M>) {}"
    ));
    assert!(names_lenient_extractor("use axum::extract::{State, Form};"));
    // A mention in a comment or a string is not a call.
    assert!(!names_lenient_percent_decoder(
        "// never `percent_decode_str` here"
    ));
    assert!(!names_lenient_percent_decoder(
        "/* outer /* nested */ percent_decode */ let x = 1;"
    ));
    assert!(!names_lenient_percent_decoder(
        "let msg = \"no \\\"percent_decode\\\" here\";"
    ));
    // A `'"'` character literal does not open a string that hides the call.
    assert!(names_lenient_percent_decoder(
        "let c = '\"'; percent_decode(x); let s = \"y\";"
    ));
    assert!(!names_lenient_extractor(
        "use axum::extract::{State, QueryPlan}; // extract::Query"
    ));
    assert!(!names_lenient_extractor(
        "type DbQuery<'q> = sqlx::query::Query<'q, D, A>;"
    ));
    let lossy = code_of("let t = String::from_utf8_lossy(&b); // from_utf8_lossy");
    assert_eq!(lossy.matches(LOSSY_UTF8).count(), 1, "one site: {lossy}");
    let reader = code_of("for p in u.query_pairs() {} let s = \"query_pairs\";");
    assert_eq!(
        inventory(&[("f.rs".to_owned(), reader)], LENIENT_QUERY_READERS),
        pinned(&[("f.rs", 1)])
    );
}
