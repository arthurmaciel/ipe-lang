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
//! forwarded verbatim by the sandbox-private `ipe_sandbox::host_env::granted`,
//! reachable from outside the sandbox crate only as `granted_env` over a
//! profile's own allowlist, from the one pinned WASI launcher.
//!
//! The root `clippy.toml` denies `std::env::{var, var_os, vars, vars_os}`, so
//! the audited readers are the only raw readers. This scan pins that set
//! independently: it refuses a literal home read in production sources, a
//! home-dir crate (`home`, `dirs`, `directories`, `etcetera`, …) as a
//! manifest dependency or a source path, the private home-name constant outside its module, a raw `std::env` read or
//! whole-environment iterator outside the audited files, the escape-hatch
//! allow outside the pinned allow files, the jail passthrough outside the
//! sandbox crate and its pinned caller, and a `/proc/*/environ` read.

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

/// The sandbox crate's sources: the only callers of the crate-private raw
/// passthrough `host_env::granted`.
const SANDBOX_SRC: &str = "src/compiler/sandbox/src/";

/// Workspace-relative files outside the sandbox crate that may forward a
/// profile's granted variables through `host_env::granted_env`.
const JAIL_ENV_CALLERS: &[&str] = &["src/ipe-cli/src/wasi_run.rs"];

/// The profile-scoped passthrough's name.
const JAIL_ENV_FN: &str = "granted_env";

/// Whitespace-free code spellings that reach the raw passthrough
/// `host_env::granted`: its path, or a group or glob import of its module.
const RAW_PASSTHROUGH_PATHS: &[&str] = &["host_env::granted", "host_env::{", "host_env::*"];

/// The module that owns the private home-name constant.
const HOME_MODULE: &str = "src/compiler/sandbox/src/home.rs";

/// Whitespace-free call openers that read an environment variable named by a
/// literal, `{}` standing for the name.
///
/// Covers `std::env::var`/`var_os` and every wrapper ending in `var`,
/// `env!`/`option_env!`, and libc's `getenv` over a string, C-string, or
/// NUL-terminated byte literal.
const LITERAL_READ_SHAPES: &[&str] = &[
    "var(\"{}\"",
    "var_os(\"{}\"",
    "env!(\"{}\"",
    "getenv(\"{}\"",
    "getenv(\"{}\\0\"",
    "getenv(c\"{}\"",
    "getenv(b\"{}\\0\"",
];

/// Whitespace-free spellings of the deprecated `std` home reader.
const STD_HOME_READS: &[&str] = &["env::home_dir"];

/// Crates whose API resolves the invoking user's home or a directory beneath
/// it, by Cargo package name.
const HOME_DIR_CRATES: &[&str] = &[
    "home",
    "dirs",
    "dirs-next",
    "dirs-sys",
    "dirs-sys-next",
    "directories",
    "directories-next",
    "etcetera",
];

/// The whitespace-free literal home reads: every `LITERAL_READ_SHAPES` opener
/// over every `ipe_env::HOME_NAMES` name.
fn literal_home_reads() -> Vec<String> {
    LITERAL_READ_SHAPES
        .iter()
        .flat_map(|shape| {
            ipe_env::HOME_NAMES
                .iter()
                .map(move |name| shape.replace("{}", name))
        })
        .collect()
}

/// The raw home reads `src` contains, whitespace and line breaks ignored.
///
/// A hit is a literal home read, the `std` home reader, or a crate-root path
/// into a home-dir crate.
fn raw_home_reads(src: &str) -> Vec<String> {
    let flat: String = src.chars().filter(|c| !c.is_whitespace()).collect();
    let mut hits: Vec<String> = literal_home_reads()
        .into_iter()
        .chain(STD_HOME_READS.iter().map(|s| (*s).to_owned()))
        .filter(|needle| flat.contains(needle.as_str()))
        .collect();
    hits.extend(home_crate_paths(src));
    hits
}

/// The home-dir crate paths `src`'s code reaches from a crate root.
///
/// `crate::home::` or `ipe_sandbox::home::` is a sub-path, not the crate; a
/// crate-rooted `home::` or `::home::` is refused, and so, failing closed, is
/// one inside a group import (`{home::`).
fn home_crate_paths(src: &str) -> Vec<String> {
    let code = code_words(src);
    HOME_DIR_CRATES
        .iter()
        .map(|krate| format!("{}::", krate.replace('-', "_")))
        .filter(|needle| {
            code.match_indices(needle.as_str())
                .any(|(at, _)| is_crate_root_path(&code, at))
        })
        .collect()
}

/// Whether the path segment at byte offset `at` of `code` starts at a crate root.
///
/// It must be neither glued to an identifier nor preceded by `<ident>::`.
fn is_crate_root_path(code: &str, at: usize) -> bool {
    if ident_before(code, at) {
        return false;
    }
    let head = code.get(..at).map_or("", str::trim_end);
    head.strip_suffix("::").is_none_or(|parent| {
        !parent
            .trim_end()
            .chars()
            .next_back()
            .is_some_and(is_ident_char)
    })
}

/// The home-dir crates `manifest` depends on.
///
/// A dependency key in either spelling (`dirs-next`, `dirs_next`), a
/// `[…dependencies.<crate>]` table, or a `package = "<crate>"` rename counts.
fn home_crate_deps(manifest: &str) -> Vec<&'static str> {
    let lines: Vec<String> = manifest
        .lines()
        .map(|line| {
            line.split_once('#')
                .map_or(line, |(code, _)| code)
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect()
        })
        .collect();
    HOME_DIR_CRATES
        .iter()
        .copied()
        .filter(|krate| {
            let snake = krate.replace('-', "_");
            [*krate, snake.as_str()].into_iter().any(|name| {
                lines.iter().any(|line| {
                    line.strip_prefix(name)
                        .is_some_and(|rest| rest.starts_with(['=', '.']))
                        || line.contains(&format!("dependencies.{name}]"))
                        || line.contains(&format!("package=\"{name}\""))
                })
            })
        })
        .collect()
}

/// Whitespace-free code spellings that reach the raw environment readers:
/// a path to `var`/`var_os`/`vars`/`vars_os`, a glob or group import of
/// `std::env` that would let them be called unqualified, or libc's `getenv`.
const RAW_ENV_PATHS: &[&str] = &["env::var", "std::env::{", "std::env::*", "libc::getenv"];

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
    code_text(src, false)
}

/// [`code_only`], but each whitespace character kept as one space, so two
/// adjacent identifiers (`granted_env as g`) stay two words.
fn code_words(src: &str) -> String {
    code_text(src, true)
}

/// The code of `src` with comments dropped and literals collapsed to `""`;
/// whitespace kept as spaces when `spaced`, else dropped.
fn code_text(src: &str, spaced: bool) -> String {
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
            } else if spaced {
                out.push(' ');
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

/// Whether `src`'s code names the identifier `ident` (not as part of a longer
/// identifier).
fn names_ident(src: &str, ident: &str) -> bool {
    let code = code_words(src);
    code.match_indices(ident)
        .any(|(at, m)| !ident_before(&code, at) && !ident_after(&code, at + m.len()))
}

/// Whether `src`'s code names the private home-name constant.
fn names_home_var(src: &str) -> bool {
    names_ident(src, "HOME_VAR")
}

/// Whether `src`'s code reaches the raw passthrough `host_env::granted`: a
/// path to it, or a group or glob import of its module. Only a direct
/// `host_env::granted_env(..)` call is the profile-scoped form; any other
/// continuation (an alias included) is refused.
fn reaches_raw_passthrough(src: &str) -> bool {
    let code = code_only(src);
    RAW_PASSTHROUGH_PATHS.iter().any(|needle| {
        code.match_indices(needle).any(|(at, m)| {
            let scoped = code
                .get(at + m.len()..)
                .is_some_and(|tail| tail.starts_with("_env("));
            !ident_before(&code, at) && (needle.ends_with(['{', '*']) || !scoped)
        })
    })
}

/// Whether `src` names a procfs environment file (`/proc/self/environ`, a
/// `join("environ")`): literals included, line comments not.
fn reads_proc_environ(src: &str) -> bool {
    src.lines()
        .map(|line| line.split_once("//").map_or(line, |(code, _)| code))
        .any(|line| line.contains("environ\""))
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

/// Recursively collect every `Cargo.toml` under `dir` into `out`, build output
/// and hidden directories skipped.
fn collect_manifests(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            let skipped = path
                .file_name()
                .is_some_and(|n| n == "target" || n.to_string_lossy().starts_with('.'));
            if !skipped {
                collect_manifests(&path, out);
            }
        } else if path.file_name().is_some_and(|n| n == "Cargo.toml") {
            out.push(path);
        }
    }
}

/// The workspace root manifest and every `Cargo.toml` under `src/`, `tools/`,
/// and `examples/`, keyed by its workspace-relative path.
fn workspace_manifests() -> Vec<(String, String)> {
    let root = workspace();
    let mut files = vec![root.join("Cargo.toml")];
    for top in ["src", "tools", "examples"] {
        collect_manifests(&root.join(top), &mut files);
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
fn no_manifest_depends_on_a_home_dir_crate() {
    let manifests = workspace_manifests();
    assert!(
        manifests.iter().any(|(rel, _)| rel == "Cargo.toml"),
        "the scan found no root manifest; the walk root is wrong"
    );
    let offenders: Vec<_> = manifests
        .iter()
        .map(|(rel, text)| (rel, home_crate_deps(text)))
        .filter(|(_, hits)| !hits.is_empty())
        .collect();
    assert!(
        offenders.is_empty(),
        "a home-dir crate resolves the host home outside the validated \
         accessors; use `ipe_sandbox::home::home_dir`: {offenders:?}"
    );
}

#[test]
fn a_planted_home_dir_crate_dependency_is_detected() {
    for manifest in [
        "[dependencies]\nhome = \"0.5\"",
        "[dependencies]\ndirs = { version = \"6\" }",
        "[dev-dependencies]\ndirs-next = \"2\"",
        "[dependencies]\ndirs_sys = \"0.5\"",
        "[dependencies]\ndirectories.workspace = true",
        "[workspace.dependencies]\ndirectories-next = \"2\"",
        "[target.'cfg(unix)'.dependencies]\n  etcetera = \"0.8\"",
        "[dependencies.dirs-sys-next]\nversion = \"0.1\"",
        "[dependencies]\nhd = { package = \"home\", version = \"0.5\" }",
    ] {
        assert!(
            !home_crate_deps(manifest).is_empty(),
            "the scan missed a home-dir crate dependency: {manifest:?}"
        );
    }
    for manifest in [
        "[package]\nhomepage = \"https://example.org\"",
        "[dependencies]\ndirsx = \"1\"",
        "# home = \"0.5\"",
        "[dependencies]\nipe_env = { path = \"../compiler/env\" }",
    ] {
        assert!(
            home_crate_deps(manifest).is_empty(),
            "the scan flagged a non-home-dir manifest line: {manifest:?}"
        );
    }
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
fn the_jail_passthrough_stays_in_the_sandbox() {
    let files = workspace_sources(true);
    let offenders: Vec<_> = files
        .iter()
        .filter(|(rel, text)| {
            !rel.starts_with(SANDBOX_SRC)
                && (reaches_raw_passthrough(text)
                    || (!JAIL_ENV_CALLERS.contains(&rel.as_str())
                        && names_ident(text, JAIL_ENV_FN)))
        })
        .map(|(rel, _)| rel)
        .collect();
    assert!(
        offenders.is_empty(),
        "the jail env passthrough used outside the sandbox crate and its pinned \
         callers; read named keys through `ipe_env`: {offenders:?}"
    );
}

#[test]
fn no_production_source_reads_a_procfs_environment() {
    let files = workspace_sources(false);
    let offenders: Vec<_> = files
        .iter()
        .filter(|(rel, text)| !rel.starts_with(RUNTIME_ROOT) && reads_proc_environ(text))
        .map(|(rel, _)| rel)
        .collect();
    assert!(
        offenders.is_empty(),
        "a `/proc/*/environ` read hands out the whole environment, home \
         included; read named keys through `ipe_env`: {offenders:?}"
    );
}

#[test]
fn a_planted_passthrough_or_procfs_bypass_is_detected() {
    for src in [
        "let h = ipe_sandbox::host_env::granted(\"HOME\");",
        "let h = host_env::granted(k);",
        "use ipe_sandbox::host_env::{granted};",
        "use ipe_sandbox::host_env::*;",
        "use ipe_sandbox::host_env::granted as g;",
        "use ipe_sandbox::host_env::granted_env as g;",
    ] {
        assert!(
            reaches_raw_passthrough(src),
            "the scan missed a raw passthrough: {src:?}"
        );
    }
    for src in [
        "let e = ipe_sandbox::host_env::granted_env(&p);",
        "use ipe_sandbox::host_env::granted_env as g;",
        "use ipe_sandbox::{host_env::granted_env};",
    ] {
        assert!(
            names_ident(src, JAIL_ENV_FN),
            "the scan missed a jail env passthrough: {src:?}"
        );
    }
    for src in [
        "let e = std::fs::read(\"/proc/self/environ\");",
        "let e = std::fs::read(Path::new(\"/proc/1\").join(\"environ\"));",
    ] {
        assert!(
            reads_proc_environ(src),
            "the scan missed a procfs environment read: {src:?}"
        );
    }
    assert!(!raw_env_reads("let h = unsafe { libc::getenv(k) };").is_empty());
    assert!(!reaches_raw_passthrough(
        "let e = ipe_sandbox::host_env::granted_env(&p);"
    ));
    assert!(!names_ident("let e = granted_envs;", JAIL_ENV_FN));
    assert!(!reads_proc_environ("// read /proc/self/environ\"x\""));
    assert!(!reads_proc_environ("let e = \"the environment\";"));
}

#[test]
fn every_pinned_file_exists() {
    let root = workspace();
    for rel in ACCESSOR_FILES
        .iter()
        .chain(ENV_ALLOW_FILES)
        .chain(JAIL_ENV_CALLERS)
    {
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
        "const H: Option<&str> = option_env!(\"HOME\");",
        "const H: &str = env!(\"HOME\");",
        "const H: &str = env!(\n    \"USERPROFILE\",\n    \"needs a home\",\n);",
        "let h = option_env!(\"HOMEDRIVE\");",
        "let h = std::env::var_os(\"HOMEPATH\");",
        "let h = unsafe { libc::getenv(c\"HOME\".as_ptr()) };",
        "let h = unsafe { getenv(\"USERPROFILE\\0\".as_ptr().cast()) };",
        "let h = unsafe { libc::getenv(b\"HOME\\0\".as_ptr().cast()) };",
        "let h = home::home_dir();",
        "let c = home::cargo_home();",
        "use home::env::home_dir_with_env;",
        "let h = ::home::home_dir();",
        "let c = dirs::config_dir();",
        "let c = dirs::cache_dir();",
        "let d = dirs::data_dir();",
        "let d = dirs::data_local_dir();",
        "let s = dirs::state_dir();",
        "use dirs::{config_local_dir, runtime_dir};",
        "let h = dirs_next::home_dir();",
        "let c = dirs_next::config_dir();",
        "let h = dirs_sys::home_dir();",
        "let b = directories::BaseDirs::new();",
        "let p = directories_next::ProjectDirs::from(q, o, a);",
        "let s = etcetera::choose_base_strategy();",
        "use etcetera::{BaseStrategy, choose_base_strategy};",
        "use ipe_sandbox::{home::home_dir};",
    ];
    for src in planted {
        assert!(
            !raw_home_reads(src).is_empty(),
            "the scan missed a raw home read: {src:?}"
        );
    }
}

#[test]
fn every_home_name_is_a_literal_home_read() {
    let needles = literal_home_reads();
    for name in ipe_env::HOME_NAMES {
        for shape in LITERAL_READ_SHAPES {
            assert!(
                needles.contains(&shape.replace("{}", name)),
                "`{shape}` over `{name}` is not scanned"
            );
        }
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
        "let h = crate::home::home_dir();",
        "let h = super :: home::home_dir();",
        "let h = my_home::root();",
        "let h = homedir::root();",
        "let s = \"dirs::home_dir()\";",
        "// dirs::home_dir() would read the host home",
    ];
    for src in clean {
        assert!(
            raw_home_reads(src).is_empty(),
            "the scan flagged a sanctioned form: {src:?}"
        );
    }
}
