//! Shared end-to-end build/run support for the golden parity gate.
//!
//! Every golden's emitted Rust project is built into ONE shared cargo target —
//! the machine-global `~/.cache/ipe-lang-target`, configured in the global
//! `~/.cargo/config.toml` (`target-dir = …`). cargo reads that file on every
//! invocation, including builds launched from an emitted project under
//! `std::env::temp_dir()`, so heavy dependencies (tokio / rsa / serde / …)
//! compile ONCE and are reused across all goldens and across runs. The target
//! is never deleted; reuse is the whole point.
//!
//! To let the per-golden root binaries coexist in that single target without
//! clobbering one another, this helper rewrites the emitted manifest to a unique
//! package (and therefore binary) name per golden before building.
//!
//! The produced binary is located ROBUSTLY by parsing
//! `cargo build --message-format=json` for the artifact's `executable` field —
//! never a hard-coded `target/debug/<name>` path, which would silently break the
//! moment the target dir moves (exactly the breakage the per-test
//! `CARGO_TARGET_DIR` override existed to paper over).
//!
//! Rigour: a build failure FAILS the test (the build assert carries cargo's
//! stderr); it is never skipped and never reported as a false green.
//!
//! The build/run plumbing lives in the shared [`e2e_support`] crate so every
//! test binary uses the same cargo-JSON parsing logic to locate the produced
//! binary.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The `ipe-lang` workspace root (two levels up from this crate's manifest).
///
/// Shared so every golden test resolves the golden tree the same way, rather
/// than each file carrying its own hand-rolled duplicate. Canonicalises the
/// `../..` join so downstream path comparisons see a normalised absolute path;
/// if canonicalisation fails (e.g. a component does not exist), the
/// un-normalised join is returned unchanged — the directory the tests read
/// always exists in a checked-out tree, so the fallback is never the green
/// path.
#[must_use]
#[allow(dead_code)] // adopted file-by-file as goldens migrate to the shared helper
pub fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

/// The directory holding a golden's `main.rs` — i.e. `golden.parent()` — with a
/// non-panicking fallback (a golden path built as `<dir>/main.rs` always has a
/// parent; the fallback keeps this `expect`-free under the workspace-wide
/// `clippy::expect_used` deny). Centralises the derivation for the 40+ golden
/// tests that need it rather than copy-pasting `golden.parent().expect(...)`.
#[allow(dead_code)] // shared helper: used by the 45 byte-diff goldens, not every binary
pub fn golden_dir_of(golden: &Path) -> &Path {
    golden.parent().unwrap_or(golden)
}

/// Collapse rustfmt's whitespace-driven line-wrap noise out of emitted Rust
/// source, so a golden substring assertion tracks *token adjacency*, not one
/// specific line layout.
///
/// Once a call, type, or struct literal exceeds rustfmt's line-width limit it
/// wraps: every run of whitespace becomes a newline + indentation, and a
/// wrapped call/generic/struct list gets a trailing comma before its closing
/// `)`/`]`/`}`/`>` that the compact single-line rendering never has. Neither
/// changes the code's meaning, so both are erased here — every whitespace run
/// collapses away, and a comma immediately preceding a closing bracket is
/// dropped. The compact and wrapped renderings of the same code converge on
/// one identical normalized string, so `contains` on the normalized text
/// checks the thing that actually matters (this token sequence occurs,
/// adjacent, in this order) rather than a rustfmt version's specific wrap
/// point — the stale-substring-assertion class fixed piecemeal for #269,
/// #191, #193, #195, #175 (`Ipe.Ui.Transition`) before this helper existed.
#[must_use]
#[allow(dead_code)] // adopted incrementally as fragile golden substring checks migrate to it
pub fn normalize_rustfmt_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_whitespace() {
            continue;
        }
        if matches!(c, ')' | ']' | '}' | '>') && out.ends_with(',') {
            out.pop();
        }
        out.push(c);
    }
    out
}

/// Assert every byte-comparable golden output under `golden_dir` matches its
/// counterpart in the emitted project rooted at `emitted_out`, byte-for-byte.
///
/// This is the directory-diff replacement for each golden's former ad hoc
/// `read_to_string(<out>/src/main.rs)` + `assert_eq!(_, <golden>/main.rs)`
/// pair. It preserves that assertion's discriminating power exactly — a golden
/// `main.rs` is always compared — and, where a golden dir ALSO checks in a
/// root `Cargo.toml`, it additionally compares that manifest, catching
/// manifest drift the single-file assertion could not.
///
/// **Scope is manifest-authoritative, not a blind recursive diff.** A golden
/// dir carries OTHER, non-emitted fixture files (`Main.ipe`, `expected_go.txt`,
/// `oracle.meta`, and a partial `ipe_runtime/` reference tree) that the emitted
/// project does not reproduce at those paths; the walk is scoped to exactly the
/// golden's byte-diffable emitted artifacts (`main.rs` -> `<out>/src/main.rs`,
/// and `Cargo.toml` -> `<out>/Cargo.toml` WHEN the golden dir checks one in),
/// so an unrelated fixture file can never be misread as a mismatch. Both
/// under-emission (a missing emitted file) and content drift fail loudly, each
/// with the offending relative path.
///
/// **This compares `Cargo.toml` too, not only `main.rs`.** Because
/// `Cargo.toml` is compared whenever the golden dir checks one in, a test using
/// this helper gets a `Cargo.toml` comparison in addition to `main.rs`. That is
/// usually desirable (it catches manifest drift), but a stale golden
/// `Cargo.toml` surfaces as a `Cargo.toml` mismatch — a genuine finding to fix
/// at root (refresh the stale manifest), not a helper bug to route around.
///
/// # Panics
///
/// Fails the calling test (via `assert!`) if any compared file is
/// missing/unreadable or differs from its golden counterpart, surfacing every
/// offending path in one message.
#[allow(dead_code)] // adopted file-by-file as goldens migrate to the shared helper
pub fn assert_emitted_project_matches_golden_dir(emitted_out: &Path, golden_dir: &Path) {
    // (golden-relative name, emitted path). `main.rs` is always compared;
    // `Cargo.toml` only when the golden dir checks one in — mirroring what the
    // hand-rolled per-file assertions did (all compared `main.rs`; only `basics`
    // carried a golden manifest, other goldens do not).
    let mut pairs: Vec<(String, PathBuf)> = vec![(
        "main.rs".to_owned(),
        emitted_out.join("src").join("main.rs"),
    )];
    if golden_dir.join("Cargo.toml").is_file() {
        pairs.push(("Cargo.toml".to_owned(), emitted_out.join("Cargo.toml")));
    }

    // Per-Ipê-module split files: when the emitted
    // project splits into `src/ipe_mods/<mod>.rs`, each is compared byte-for-byte
    // against `<golden_dir>/ipe_mods/<mod>.rs`. The comparison is SYMMETRIC —
    // the union of the emitted set and the golden set is walked, so an emitted
    // file the golden lacks (under-checked-in) AND a golden file the split no
    // longer emits (stale/over-checked-in) both fail loudly, mirroring
    // `prune_orphaned_files`'s manifest-is-authoritative discipline. A program
    // that collapses to a single file (the §3.3 Spine-collapse invariant) has
    // no `ipe_mods/` on EITHER side, so this adds nothing for those goldens.
    let mut ipe_mod_names: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for dir in [
        emitted_out.join("src").join("ipe_mods"),
        golden_dir.join("ipe_mods"),
    ] {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if path.extension().is_some_and(|x| x == "rs")
                    && let Some(name) = path.file_name().and_then(|n| n.to_str())
                {
                    ipe_mod_names.insert(name.to_owned());
                }
            }
        }
    }
    for name in &ipe_mod_names {
        pairs.push((
            format!("ipe_mods/{name}"),
            emitted_out.join("src").join("ipe_mods").join(name),
        ));
    }

    let mut mismatches = Vec::new();
    for (rel, emitted_path) in &pairs {
        let want_path = golden_dir.join(rel);
        match (
            std::fs::read_to_string(&want_path),
            std::fs::read_to_string(emitted_path),
        ) {
            (Ok(want_text), Ok(got_text)) if want_text == got_text => {}
            (Ok(want_text), Ok(got_text)) => mismatches.push(format!(
                "{rel}: emitted != golden ({} vs {} bytes)\n  emitted: {}\n  golden:  {}",
                got_text.len(),
                want_text.len(),
                emitted_path.display(),
                want_path.display(),
            )),
            (Err(e), _) => mismatches.push(format!(
                "{rel}: golden missing or unreadable at {}: {e}",
                want_path.display()
            )),
            (_, Err(e)) => mismatches.push(format!(
                "{rel}: emitted missing or unreadable at {}: {e}",
                emitted_path.display()
            )),
        }
    }
    assert!(
        mismatches.is_empty(),
        "golden mismatch under {}:\n{}",
        golden_dir.display(),
        mismatches.join("\n")
    );
}

/// Concatenate the text of every emitted Ipê-side `.rs` file under
/// `<emitted_out>/src` — `src/main.rs` plus every `src/ipe_mods/*.rs` the
/// per-Ipê-module split may have written — into ONE
/// string, so a substring assertion is robust to WHICH file the split placed
/// a symbol in.
///
/// The vendored `src/ipe_runtime/` tree is deliberately EXCLUDED: it is the
/// fixed kernel runtime, identical for every program, and a substring test
/// asserting "the emitted program carries symbol X" means X in the emitted
/// USER/stdlib-source code, never a coincidental match inside the runtime
/// shim. Scoping to `main.rs` + `ipe_mods/*.rs` keeps that discrimination
/// exactly as sharp as the old `read_to_string(src/main.rs)` had it while no
/// longer caring whether the split relocated X out of `main.rs` into a
/// `ipe_mods/<mod>.rs` file (the correct new multi-file behaviour).
///
/// # Panics
///
/// Fails the calling test (via `assert!`) if `src/main.rs` is missing or
/// unreadable — a green path always emits it, so its absence is a real
/// failure, never silently a no-op empty haystack. A missing `ipe_mods/`
/// directory is NOT a failure: the Spine-collapse invariant (§3.3) legitimately
/// emits no `ipe_mods/` for a single-home program.
#[must_use]
#[allow(dead_code)] // adopted file-by-file as substring goldens migrate to the shared helper
pub fn read_all_emitted_src(emitted_out: &Path) -> String {
    let src = emitted_out.join("src");
    let main_rs = src.join("main.rs");
    let main_text = std::fs::read_to_string(&main_rs);
    assert!(
        main_text.is_ok(),
        "emitted main.rs must exist and be readable at {}: {:?}",
        main_rs.display(),
        main_text.as_ref().err(),
    );
    let mut combined = main_text.unwrap_or_default();

    // Append every `src/ipe_mods/*.rs` the split may have written, in a
    // deterministic (sorted) order so the concatenation is stable across
    // filesystems that hand back directory entries in arbitrary order. If the
    // program collapsed to a single file, this directory does not exist and
    // the loop is a no-op.
    let ipe_mods = src.join("ipe_mods");
    if let Ok(entries) = std::fs::read_dir(&ipe_mods) {
        let mut files: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "rs"))
            .collect();
        files.sort();
        for path in files {
            if let Ok(text) = std::fs::read_to_string(&path) {
                combined.push('\n');
                combined.push_str(&text);
            }
        }
    }
    combined
}

/// Outcome of building and running an emitted Ipê project.
// Fields are only accessed from E2E test functions (`build_and_run_emitted`
// callers), which are absent from test binaries that skip the E2E suite (e.g.
// `golden_mm`).  `build_and_run_emitted` itself carries its own allow;
// the struct-level allow keeps the field warnings silent in those binaries.
#[allow(dead_code)]
pub struct RunOutcome {
    /// The program's standard output, decoded lossily from UTF-8.
    pub stdout: String,
    /// The process exit code (`None` if the process was killed by a signal).
    pub exit_code: Option<i32>,
}

/// Build the emitted project at `emitted_dir` into the shared target WITHOUT
/// running it, returning `Ok(())` on a successful `cargo build` or `Err` with
/// cargo's stderr. Used by SEAL goldens whose kernel is network-effectful (e.g.
/// `Email.send`) so a run has no deterministic stdout — the SEAL proof there is
/// that ipe-0 ⇒ the emitted crate `cargo build`s. Delegates to
/// [`e2e_support::build_rust_binary`].
#[allow(dead_code)] // not every golden test binary exercises every helper
pub fn build_emitted(golden_name: &str, emitted_dir: &Path) -> Result<(), String> {
    e2e_support::build_rust_binary(golden_name, emitted_dir).map(|_| ())
}

/// Build the emitted project at `emitted_dir` into the shared target and run the
/// resulting binary, returning its captured stdout and exit code.
///
/// Delegates to [`e2e_support::build_and_run_rust`] (the same core the refresh tool
/// uses) and wraps its `Result` in a test assertion.
///
/// # Panics
///
/// Fails the calling test (via `assert!`) if the manifest cannot be retargeted,
/// if `cargo build` fails (surfacing cargo's stderr), if the produced binary
/// cannot be located in the JSON output, or if the binary cannot be executed.
/// It never returns a placeholder on a green path — a broken golden cannot pass.
#[must_use]
#[allow(dead_code)] // not every golden test binary exercises every helper
pub fn build_and_run_emitted(golden_name: &str, emitted_dir: &Path) -> RunOutcome {
    let result = e2e_support::build_and_run_rust(golden_name, emitted_dir);
    assert!(
        result.is_ok(),
        "{}",
        result.as_ref().err().map_or("", String::as_str)
    );
    let Ok(result) = result else {
        // Unreachable on a green path: the assert above already failed the test.
        return RunOutcome {
            stdout: String::new(),
            exit_code: None,
        };
    };
    RunOutcome {
        stdout: result.stdout,
        exit_code: result.exit_code,
    }
}

/// Build the emitted project and run its binary with `stdin_bytes` piped to its
/// stdin, then closed (signalling EOF), returning its captured stdout and exit
/// code.
///
/// Used by goldens that drive an interactive/line-oriented loop (e.g.
/// `Terminal.appLines`) past its first stdin read, which [`build_and_run_emitted`]
/// cannot exercise since it runs the binary with stdin already at EOF.
///
/// # Panics
/// Fails the calling test if the binary cannot be located (surfacing cargo's
/// stderr), or cannot be spawned with a piped stdin.
#[must_use]
#[allow(dead_code)] // only stdin-driven goldens exercise this helper
pub fn build_and_run_emitted_with_stdin(
    golden_name: &str,
    emitted_dir: &Path,
    stdin_bytes: &[u8],
) -> RunOutcome {
    let exe = e2e_support::build_rust_binary(golden_name, emitted_dir);
    assert!(
        exe.is_ok(),
        "{}",
        exe.as_ref().err().map_or("", String::as_str)
    );
    let Ok(exe) = exe else {
        return RunOutcome {
            stdout: String::new(),
            exit_code: None,
        };
    };

    let child = Command::new(&exe)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn();
    assert!(
        child.is_ok(),
        "{golden_name}: failed to spawn `{exe}`: {:?}",
        child.as_ref().err()
    );
    let Ok(mut child) = child else {
        return RunOutcome {
            stdout: String::new(),
            exit_code: None,
        };
    };

    let stdin = child.stdin.take();
    assert!(stdin.is_some(), "{golden_name}: child stdin must be piped");
    let Some(mut stdin) = stdin else {
        return RunOutcome {
            stdout: String::new(),
            exit_code: None,
        };
    };
    let wrote = stdin.write_all(stdin_bytes);
    assert!(
        wrote.is_ok(),
        "{golden_name}: failed to write stdin: {wrote:?}"
    );
    drop(stdin); // close stdin so the child's reader sees EOF after these bytes

    let output = child.wait_with_output();
    assert!(
        output.is_ok(),
        "{golden_name}: failed to wait on `{exe}`: {:?}",
        output.as_ref().err()
    );
    let Ok(output) = output else {
        return RunOutcome {
            stdout: String::new(),
            exit_code: None,
        };
    };
    RunOutcome {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        exit_code: output.status.code(),
    }
}

/// Build the emitted project and run its binary with the MAIN-THREAD stack
/// capped to `stack_kib` KiB, via `bash -c 'ulimit -s <kib>; exec "$0"' <bin>`.
///
/// A deep non-TCO recursion then overflows deterministically at a few thousand
/// frames instead of needing ~10^6, so the constant-stack proof is fast and
/// robust. A stack overflow trips the guard page and `abort()`s (SIGABRT) — NOT
/// a catchable panic — so the child dies by signal and `exit_code` is `None`;
/// the TCO'd binary instead exits cleanly with `Some(0)`. Linux/macOS only (the
/// Rust backend's target).
///
/// # Panics
/// Fails the calling test if `cargo build` fails (surfacing cargo's stderr), the
/// binary cannot be located, or the `bash`/`ulimit` runner cannot be spawned.
#[must_use]
#[allow(dead_code)] // only the constant-stack golden exercises this helper
pub fn build_and_run_stack_limited(
    golden_name: &str,
    emitted_dir: &Path,
    stack_kib: u32,
) -> RunOutcome {
    let exe = e2e_support::build_rust_binary(golden_name, emitted_dir);
    assert!(
        exe.is_ok(),
        "{}",
        exe.as_ref().err().map_or("", String::as_str)
    );
    let Ok(exe) = exe else {
        return RunOutcome {
            stdout: String::new(),
            exit_code: None,
        };
    };
    // `exec "$0"` replaces the shell after `ulimit -s` lowers the soft stack
    // limit, so the child binary runs under the capped main-thread stack.
    let output = Command::new("bash")
        .arg("-c")
        .arg(format!("ulimit -s {stack_kib}; exec \"$0\""))
        .arg(&exe)
        .output();
    assert!(
        output.is_ok(),
        "{golden_name}: failed to spawn stack-limited runner: {:?}",
        output.as_ref().err()
    );
    let Ok(output) = output else {
        return RunOutcome {
            stdout: String::new(),
            exit_code: None,
        };
    };
    RunOutcome {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        exit_code: output.status.code(),
    }
}

/// Assert ipe's stdout matches the golden's captured expected output.
///
/// Reads `tests/golden/<name>/expected.txt` (the self-regression anchor) and
/// fails loudly if the file is absent or the output differs. A missing file is
/// always a hard failure — never a skip — so a golden without a captured
/// expected output cannot pass silently.
#[allow(dead_code)] // exercised by goldens with a captured expected output
pub fn assert_self_regression(golden_name: &str, golden_dir: &Path, ipe_stdout: &str) {
    let expected = e2e_support::read_expected(golden_dir);
    assert!(
        expected.is_ok(),
        "{golden_name}: {}",
        expected.as_ref().err().map_or("", String::as_str)
    );
    let Ok(expected) = expected else { return };
    assert_eq!(
        ipe_stdout, expected,
        "{golden_name}: stdout does not match expected.txt"
    );
}

/// Compatibility alias: callers that previously used `assert_go_parity` now get
/// the self-regression check (`expected.txt` instead of `expected_go.txt` +
/// `oracle.meta`). Keeping the name avoids a mass-rename across the ~72 call
/// sites — the semantics are identical: hard-fail on mismatch or missing file.
#[allow(dead_code)]
pub fn assert_go_parity(golden_name: &str, golden_dir: &Path, ipec_stdout: &str) {
    assert_self_regression(golden_name, golden_dir, ipec_stdout);
}
