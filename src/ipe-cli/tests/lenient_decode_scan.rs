#![forbid(unsafe_code)]
//! Pins the runtime's lenient decoders, a second layer beneath the clippy deny.
//!
//! Every URL component the runtime reads is decoded by the one strict core,
//! `encoding::decode_component`. The runtime `clippy.toml` denies the lenient
//! percent decoders and lossy UTF-8; this scan pins that set independently:
//! no runtime source names a lenient percent decoder at all, and lossy UTF-8
//! appears only in the files below, each converting bytes that are not a URL
//! component.

use std::path::{Path, PathBuf};

/// The runtime crate, relative to the workspace root.
const RUNTIME_ROOT: &str = "src/runtime/rust";

/// Spellings of the lenient percent decoders, refused in every runtime source.
const LENIENT_PERCENT_DECODERS: &[&str] = &["percent_decode", "decode_utf8_lossy"];

/// The lossy UTF-8 conversion.
const LOSSY_UTF8: &str = "from_utf8_lossy";

/// Runtime-relative files that may convert bytes to text lossily, each under a
/// per-site `clippy::disallowed_methods` allow naming its reason.
const LOSSY_TEXT_FILES: &[&str] = &[
    // CSV writer output, fed only `String` fields.
    "src/csv.rs",
    // An email provider's response body.
    "src/email.rs",
    // An HTTP response body.
    "src/http_client.rs",
    // A streamed HTTP response chunk.
    "src/http_stream.rs",
    // A request body and a WebSocket binary frame.
    "src/server.rs",
    // Subprocess output.
    "src/system.rs",
    // Terminal input bytes.
    "src/tui/key.rs",
    // A test's SSE reader, whose chunks may split a UTF-8 sequence.
    "src/web/mod.rs",
];

/// The clippy paths the runtime `clippy.toml` must deny.
const DENIED_PATHS: &[&str] = &[
    "percent_encoding::percent_decode_str",
    "percent_encoding::percent_decode",
    "percent_encoding::PercentDecode::decode_utf8_lossy",
    "std::string::String::from_utf8_lossy",
];

/// The runtime crate's directory.
fn runtime() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(RUNTIME_ROOT)
}

/// The code of `src` with each line's `//` comment dropped.
fn code_of(src: &str) -> String {
    src.lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
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

/// Every runtime source, crate sources and integration tests alike.
fn runtime_sources() -> Vec<(String, String)> {
    let root = runtime();
    let mut out = Vec::new();
    rust_sources(&root.join("src"), &root, &mut out);
    rust_sources(&root.join("tests"), &root, &mut out);
    assert!(
        out.iter().any(|(rel, _)| rel == "src/encoding.rs"),
        "the scan must reach the decoder core; scanned {} files",
        out.len()
    );
    out
}

/// Whether `src` names a lenient percent decoder in code.
fn names_lenient_percent_decoder(src: &str) -> bool {
    let code = code_of(src);
    LENIENT_PERCENT_DECODERS
        .iter()
        .any(|name| code.contains(name))
}

/// Whether `src` converts bytes to text lossily in code.
fn names_lossy_utf8(src: &str) -> bool {
    code_of(src).contains(LOSSY_UTF8)
}

#[test]
fn no_runtime_source_names_a_lenient_percent_decoder() {
    let offenders: Vec<_> = runtime_sources()
        .into_iter()
        .filter(|(_, text)| names_lenient_percent_decoder(text))
        .map(|(rel, _)| rel)
        .collect();
    assert!(
        offenders.is_empty(),
        "a lenient percent decoder in the runtime; decode a URL component through \
         `encoding::decode_component`: {offenders:?}"
    );
}

#[test]
fn lossy_utf8_stays_in_its_pinned_files() {
    let mut lossy: Vec<_> = runtime_sources()
        .into_iter()
        .filter(|(_, text)| names_lossy_utf8(text))
        .map(|(rel, _)| rel)
        .collect();
    lossy.sort();
    let mut pinned: Vec<_> = LOSSY_TEXT_FILES
        .iter()
        .map(|rel| (*rel).to_owned())
        .collect();
    pinned.sort();
    assert_eq!(
        lossy, pinned,
        "lossy UTF-8 must stay in its pinned files: a URL component goes through \
         `encoding::decode_component`; other text needs a reasoned per-site allow \
         and a pin here"
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
    assert!(names_lossy_utf8("let t = String::from_utf8_lossy(&b);"));
    // A mention in a comment is not a call.
    assert!(!names_lenient_percent_decoder(
        "// never `percent_decode_str` here"
    ));
    assert!(!names_lossy_utf8(
        "let t = strict(b); // not from_utf8_lossy"
    ));
}
