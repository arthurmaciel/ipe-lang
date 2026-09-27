#![forbid(unsafe_code)]
//! Refuses every read of the process environment that bypasses an audited
//! reader, as a second layer beneath the clippy `disallowed-methods` deny.
//!
//! An unset, empty, or relative `HOME` resolves against whatever the current
//! working directory is, so every home read goes through one validated accessor
//! that yields an absolute path or nothing: `ipe_sandbox::home::home_dir` on the
//! compiler side and `system::home_dir` in the standalone runtime. Every other
//! compiler-side variable is read through `ipe_env`, which refuses each home
//! name however it is spelled or computed, and a jail's granted variables are
//! forwarded verbatim by `ipe_sandbox::host_env::granted`.
//!
//! The root `clippy.toml` denies `std::env::{var, var_os, vars, vars_os}`, so
//! the audited readers are the only raw readers. This scan pins that set
//! independently: it refuses a literal home read in production sources, the
//! private home-name constant outside its module, a raw `std::env` read or
//! whole-environment iterator outside the audited files, and the escape-hatch
//! allow outside the pinned allow files.

use std::path::{Path, PathBuf};

/// Workspace-relative files that own a validated home accessor.
const ACCESSOR_FILES: &[&str] = &[
    "src/compiler/sandbox/src/home.rs",
    "src/runtime/rust/src/system.rs",
];

/// Workspace-relative files that hold the only raw environment reads outside
/// the runtime crate, each under a per-site `clippy::disallowed_methods` allow.
const ENV_ALLOW_FILES: &[&str] = &[
    "src/compiler/env/src/lib.rs",
    "src/compiler/sandbox/src/home.rs",
    "src/compiler/sandbox/src/host_env.rs",
];

/// The runtime crate: built with its own `clippy.toml` and its own home
/// accessor, so the compiler-side env rules do not apply beneath it.
const RUNTIME_ROOT: &str = "src/runtime/rust/";

/// The module that owns the private home-name constant.
const HOME_MODULE: &str = "src/compiler/sandbox/src/home.rs";

/// Whitespace-free spellings of a raw home read.
const RAW_HOME_READS: &[&str] = &[
    "var(\"HOME\")",
    "var_os(\"HOME\")",
    "var(\"USERPROFILE\")",
    "var_os(\"USERPROFILE\")",
    "env::home_dir",
    "dirs::home_dir",
];

/// Whitespace-free code spellings that reach the raw environment readers:
/// a path to `var`/`var_os`/`vars`/`vars_os`, or a glob or group import of
/// `std::env` that would let them be called unqualified.
const RAW_ENV_PATHS: &[&str] = &["env::var", "std::env::{", "std::env::*"];

/// The raw home reads `src` contains, whitespace and line breaks ignored.
fn raw_home_reads(src: &str) -> Vec<&'static str> {
    let flat: String = src.chars().filter(|c| !c.is_whitespace()).collect();
    RAW_HOME_READS
        .iter()
        .copied()
        .filter(|needle| flat.contains(needle))
        .collect()
}

/// Whether `c` can continue a Rust identifier.
fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Skip a string literal whose opening quote ends at `start`; returns the
/// index after the closing quote (or the end of input).
fn skip_quoted(chars: &[char], start: usize) -> usize {
    let mut i = start;
    while let Some(&c) = chars.get(i) {
        match c {
            '\\' => i += 2,
            '"' => return i + 1,
            _ => i += 1,
        }
    }
    chars.len()
}

/// Skip a raw string `r#*"…"#*` beginning at `start` (the `r`); `None` when
/// `start` is not a raw-string opener.
fn skip_raw(chars: &[char], start: usize) -> Option<usize> {
    let mut i = start + 1;
    let mut hashes = 0;
    while chars.get(i) == Some(&'#') {
        hashes += 1;
        i += 1;
    }
    if chars.get(i) != Some(&'"') {
        return None;
    }
    i += 1;
    while i < chars.len() {
        let closes =
            chars.get(i) == Some(&'"') && (1..=hashes).all(|k| chars.get(i + k) == Some(&'#'));
        if closes {
            return Some(i + 1 + hashes);
        }
        i += 1;
    }
    Some(chars.len())
}

/// Skip a (possibly nested) block comment beginning at `start`.
fn skip_block_comment(chars: &[char], start: usize) -> usize {
    let mut i = start;
    let mut depth = 0_usize;
    while i < chars.len() {
        match (chars.get(i), chars.get(i + 1)) {
            (Some('/'), Some('*')) => {
                depth += 1;
                i += 2;
            }
            (Some('*'), Some('/')) => {
                depth = depth.saturating_sub(1);
                i += 2;
                if depth == 0 {
                    return i;
                }
            }
            _ => i += 1,
        }
    }
    chars.len()
}

/// The whitespace-free code of `src`: comments dropped, and every string and
/// character literal collapsed to an empty `""`, so a path spelled only inside
/// a literal or comment is never mistaken for a call.
fn code_only(src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        let next = chars.get(i + 1).copied();
        let prev_ident = i > 0 && chars.get(i - 1).is_some_and(|&p| is_ident_char(p));
        if c == '/' && next == Some('/') {
            i = chars
                .iter()
                .skip(i)
                .position(|&ch| ch == '\n')
                .map_or(chars.len(), |off| i + off);
        } else if c == '/' && next == Some('*') {
            i = skip_block_comment(&chars, i);
        } else if let Some(end) = (!prev_ident && (c == 'r' || (c == 'b' && next == Some('r'))))
            .then(|| skip_raw(&chars, if c == 'b' { i + 1 } else { i }))
            .flatten()
        {
            out.push_str("\"\"");
            i = end;
        } else if c == '"' || (c == 'b' && next == Some('"') && !prev_ident) {
            out.push_str("\"\"");
            i = skip_quoted(&chars, if c == 'b' { i + 2 } else { i + 1 });
        } else if c == '\'' && next == Some('\\') {
            i = chars
                .iter()
                .skip(i + 2)
                .position(|&ch| ch == '\'')
                .map_or(chars.len(), |off| i + 3 + off);
        } else if c == '\'' && chars.get(i + 2) == Some(&'\'') {
            i += 3;
        } else {
            if !c.is_whitespace() {
                out.push(c);
            }
            i += 1;
        }
    }
    out
}

/// Whether `needle` occurs in `code` not glued to a preceding identifier, so
/// `ipe_env::var` is not read as `env::var`.
fn has_path(code: &str, needle: &str) -> bool {
    code.match_indices(needle)
        .any(|(at, _)| !ident_before(code, at))
}

/// Whether an identifier character immediately precedes byte offset `at`.
fn ident_before(code: &str, at: usize) -> bool {
    code.get(..at)
        .and_then(|head| head.chars().next_back())
        .is_some_and(is_ident_char)
}

/// Whether an identifier character immediately follows byte offset `at`.
fn ident_after(code: &str, at: usize) -> bool {
    code.get(at..)
        .and_then(|tail| tail.chars().next())
        .is_some_and(is_ident_char)
}

/// The raw environment reads `src`'s code performs.
fn raw_env_reads(src: &str) -> Vec<&'static str> {
    let code = code_only(src);
    RAW_ENV_PATHS
        .iter()
        .copied()
        .filter(|needle| has_path(&code, needle))
        .collect()
}

/// Whether `src`'s code names the private home-name constant.
fn names_home_var(src: &str) -> bool {
    let code = code_only(src);
    code.match_indices("HOME_VAR")
        .any(|(at, m)| !ident_before(&code, at) && !ident_after(&code, at + m.len()))
}

/// Whether `src`'s code carries the `disallowed_methods` escape hatch.
fn allows_disallowed_methods(src: &str) -> bool {
    code_only(src).contains("clippy::disallowed_methods")
}

/// The workspace root.
fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Recursively collect every `.rs` file under `dir` into `out`.
///
/// Build output and hidden directories are skipped; integration-test trees
/// are skipped too unless `with_tests`.
fn collect_rs(dir: &Path, with_tests: bool, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            let skipped = path.file_name().is_some_and(|n| {
                n == "target"
                    || n.to_string_lossy().starts_with('.')
                    || (!with_tests && n == "tests")
            });
            if !skipped {
                collect_rs(&path, with_tests, out);
            }
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// Every `.rs` file under `src/`, `tools/`, and `examples/`, keyed by its
/// workspace-relative `/`-separated path.
fn workspace_sources(with_tests: bool) -> Vec<(String, String)> {
    let root = workspace();
    let mut files = Vec::new();
    for top in ["src", "tools", "examples"] {
        collect_rs(&root.join(top), with_tests, &mut files);
    }
    files
        .into_iter()
        .filter_map(|path| {
            let rel = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            std::fs::read_to_string(&path).ok().map(|text| (rel, text))
        })
        .collect()
}

#[test]
fn no_production_source_reads_the_home_directly() {
    let files = workspace_sources(false);
    assert!(
        !files.is_empty(),
        "the scan found no sources; the walk root is wrong"
    );
    let offenders: Vec<_> = files
        .iter()
        .filter(|(rel, _)| !ACCESSOR_FILES.contains(&rel.as_str()))
        .map(|(rel, text)| (rel, raw_home_reads(text)))
        .filter(|(_, hits)| !hits.is_empty())
        .collect();
    assert!(
        offenders.is_empty(),
        "raw home reads found; use `ipe_sandbox::home::home_dir` (compiler) or \
         `system::home_dir` (runtime) instead: {offenders:?}"
    );
}

#[test]
fn no_source_reads_the_environment_outside_an_audited_reader() {
    let files = workspace_sources(true);
    let offenders: Vec<_> = files
        .iter()
        .filter(|(rel, _)| {
            !rel.starts_with(RUNTIME_ROOT) && !ENV_ALLOW_FILES.contains(&rel.as_str())
        })
        .map(|(rel, text)| (rel, raw_env_reads(text)))
        .filter(|(_, hits)| !hits.is_empty())
        .collect();
    assert!(
        offenders.is_empty(),
        "raw environment reads found; read named keys through `ipe_env`, the \
         home through `ipe_sandbox::home::home_dir`: {offenders:?}"
    );
}

#[test]
fn the_home_name_constant_stays_in_its_module() {
    let files = workspace_sources(true);
    let offenders: Vec<_> = files
        .iter()
        .filter(|(rel, text)| rel != HOME_MODULE && names_home_var(text))
        .map(|(rel, _)| rel)
        .collect();
    assert!(
        offenders.is_empty(),
        "`HOME_VAR` named outside `{HOME_MODULE}`; read the home through \
         `ipe_sandbox::home::home_dir`: {offenders:?}"
    );
}

#[test]
fn the_env_escape_hatch_is_pinned_to_the_audited_readers() {
    let files = workspace_sources(true);
    let offenders: Vec<_> = files
        .iter()
        .filter(|(rel, text)| {
            !rel.starts_with(RUNTIME_ROOT)
                && !ENV_ALLOW_FILES.contains(&rel.as_str())
                && allows_disallowed_methods(text)
        })
        .map(|(rel, _)| rel)
        .collect();
    assert!(
        offenders.is_empty(),
        "`clippy::disallowed_methods` allowed outside the audited readers: {offenders:?}"
    );
}

#[test]
fn every_pinned_file_exists() {
    let root = workspace();
    for rel in ACCESSOR_FILES.iter().chain(ENV_ALLOW_FILES) {
        assert!(
            root.join(rel).is_file(),
            "pinned file `{rel}` is gone; drop it from the list"
        );
    }
}

#[test]
fn a_planted_raw_home_read_is_detected() {
    let planted = [
        "let h = std::env::var_os(\"HOME\");",
        "let h = std::env::var( \"HOME\" ).ok();",
        "let h = env::var_os(\n    \"USERPROFILE\",\n);",
        "let h = read_env_var_os(\"HOME\");",
        "#[allow(deprecated)] let h = std::env::home_dir();",
        "use std::env::home_dir;",
        "let h = dirs::home_dir();",
    ];
    for src in planted {
        assert!(
            !raw_home_reads(src).is_empty(),
            "the scan missed a raw home read: {src:?}"
        );
    }
}

#[test]
fn a_planted_environment_bypass_is_detected() {
    let planted = [
        // The constant path: the home name held in a constant.
        "let h = std::env::var_os(HOME_VAR);",
        // The alias: the home name bound to a local first.
        "let k = \"HOME\";\nlet h = std::env::var(k);",
        // The whole-environment iterator: the home under its own name.
        "let h = std::env::vars().find(|(k, _)| k == \"HOME\");",
        "let h = std::env::vars_os().next();",
        "use std::env;\nlet h = env::var_os(key);",
        "use std::env::{var, var_os};",
        "use std::env::*;",
        "let h = ::std::env::var(key);",
    ];
    for src in planted {
        assert!(
            !raw_env_reads(src).is_empty(),
            "the scan missed a raw environment read: {src:?}"
        );
    }
    assert!(names_home_var("let h = home::HOME_VAR;"));
    assert!(allows_disallowed_methods(
        "#[allow(clippy::disallowed_methods)]\nfn f() {}"
    ));
}

#[test]
fn the_audited_readers_and_non_reads_are_not_flagged() {
    let clean = [
        "let h = ipe_sandbox::home::home_dir();",
        "let v = ipe_env::var_os(\"HOMEBREW_PREFIX\");",
        "let v = ipe_env::var(key).ok();",
        "let v = ipe_sandbox::host_env::granted(name);",
        "cmd.env(\"HOME\", scratch);",
        "let s = \"std::env::var_os(\\\"HOME\\\")\";",
        "let s = r#\"::std::env::var(\"IPE\")\"#;",
        "// std::env::vars() walks the whole environment",
        "/* std::env::var_os(HOME_VAR) */",
        "use crate::env::{Scope, Frame};",
        "let c = '\"'; let d = std::env::args();",
    ];
    for src in clean {
        assert!(
            raw_env_reads(src).is_empty() && !names_home_var(src),
            "the scan flagged a sanctioned form: {src:?}"
        );
    }
    assert!(!names_home_var("const HOME_VARS: u8 = 0;"));
    assert!(!allows_disallowed_methods(
        "// allow(clippy::disallowed_methods) only in audited readers"
    ));
}

#[test]
fn a_home_write_and_a_neighbouring_key_are_not_home_reads() {
    let clean = [
        "let h = ipe_sandbox::home::home_dir();",
        "let h = crate::env_dir::home();",
        "cmd.env(\"HOME\", scratch);",
        "let v = std::env::var_os(\"HOMEBREW_PREFIX\");",
    ];
    for src in clean {
        assert!(
            raw_home_reads(src).is_empty(),
            "the scan flagged a sanctioned form: {src:?}"
        );
    }
}
