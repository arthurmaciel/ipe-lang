//! Private scratch paths for temporary I/O, re-exported from the one shared primitive.
//!
//! [`ipe_sandbox::scratch`] owns creation and verification: a private 0700
//! directory under a verified base, files opened `O_EXCL` + `O_NOFOLLOW`, and
//! typed refusals. Read what an external writer wrote through
//! [`ScratchFile::read_all`], never by re-opening the path.

pub use ipe_sandbox::scratch::{ScratchDir, ScratchFile, ScratchLeaf};

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    /// Two distinct `ScratchDir` names from the same prefix never collide and
    /// neither equals a bare `<prefix>-<pid>` string (the old predictable form).
    #[test]
    fn scratch_dir_names_are_unpredictable_and_unique() {
        let pid_only = format!("ipe-publish-{}", std::process::id());
        let mut seen = std::collections::HashSet::new();
        for _ in 0..20 {
            let sd = ScratchDir::new("ipe-publish").expect("scratch dir");
            let name = sd
                .path()
                .file_name()
                .and_then(|n| n.to_str())
                .expect("utf-8 name")
                .to_owned();
            assert_ne!(
                name, pid_only,
                "name must not equal the predictable pid-only form"
            );
            assert!(seen.insert(name.clone()), "duplicate scratch name: {name}");
            // path is removed on drop — verified by raii_cleanup below
        }
    }

    /// `ScratchDir` is removed on drop.
    #[test]
    fn scratch_dir_raii_cleanup() {
        let path = {
            let sd = ScratchDir::new("ipe-raii-test").expect("scratch dir");
            let p = sd.path().to_path_buf();
            assert!(p.exists(), "dir should exist while live");
            p
        };
        assert!(!path.exists(), "dir should be gone after drop");
    }

    /// `ScratchFile` is removed on drop.
    #[test]
    fn scratch_file_raii_cleanup() {
        let path = {
            let sf = ScratchFile::create("ipe-raii-test").expect("scratch file");
            let p = sf.path().to_path_buf();
            assert!(p.exists(), "file should exist while live");
            p
        };
        assert!(!path.exists(), "file should be gone after drop");
    }

    /// Reading through the retained handle returns the bytes written to the
    /// path, even when the path name is renamed away after writing — the
    /// handle holds the original inode open.
    #[test]
    fn scratch_file_handle_reads_original_inode() {
        let original_bytes = b"original content";
        let decoy_bytes = b"swapped content";

        let mut sf = ScratchFile::create("ipe-inode-test").expect("scratch file");
        sf.file.write_all(original_bytes).expect("write");

        // Rename a decoy file over the scratch path (simulates a name-race).
        let decoy = sf.path().with_extension("decoy");
        std::fs::write(&decoy, decoy_bytes).expect("write decoy");
        std::fs::rename(&decoy, sf.path()).expect("rename decoy over path");

        // Reading through the retained handle still returns the original bytes,
        // not the decoy — the handle is bound to the original inode.
        let read_back = sf.read_all().expect("read_all");
        assert_eq!(
            read_back, original_bytes,
            "retained handle must read the original inode, not the swapped name"
        );
    }

    /// A symlink pre-seeded at the exact scratch path is not followed: the
    /// constructor retries and eventually creates a real file or directory at
    /// a fresh name, leaving the symlink target untouched.
    #[test]
    fn symlink_preseed_is_not_followed() {
        // Create a canary file that a symlink would point to.
        let canary = std::env::temp_dir().join(format!(
            "ipe-canary-{}-{}",
            std::process::id(),
            "symlink-preseed-test"
        ));
        std::fs::write(&canary, b"canary").expect("write canary");

        // Exhausting retries is hard to do reliably in a unit test because we
        // cannot control the random names.  What we CAN assert is that a
        // successful ScratchFile::create always produces a REGULAR file (not a
        // symlink), and the canary is intact.
        let sf = ScratchFile::create("ipe-preseed-test").expect("scratch file");
        let meta = std::fs::symlink_metadata(sf.path()).expect("metadata");
        assert!(
            meta.file_type().is_file(),
            "scratch file must be a regular file, not a symlink"
        );
        assert_eq!(
            std::fs::read(&canary).expect("canary readable"),
            b"canary",
            "canary must be untouched"
        );

        let _ = std::fs::remove_file(&canary);
    }

    /// No PRODUCTION code in the `ipe-cli` or `ipe-wrapper` crates (outside the
    /// sanctioned `scratch` modules) derives a temp path from `temp_dir()` — all
    /// such paths must go through a `scratch` module's exclusively-created
    /// constructors.
    ///
    /// Both the single-line form (`temp_dir().join(name)`) and the split form
    /// (`let base = temp_dir(); base.join(name)`) are caught: a bare
    /// `temp_dir()` binding in production is flagged the moment its value is
    /// `.join`-ed to build a path.
    ///
    /// Test-only code (`#[cfg(test)]` / `mod tests` blocks) is exempt: test
    /// helpers that use predictable names in isolated temp dirs do not expose the
    /// verify/exec or verify/read identity gap that the production paths do.
    ///
    /// This is the class gate: it keeps the `toctou-verify-one-exec-other-scratch`
    /// class closed against future PRODUCTION regressions across both crates.
    #[test]
    fn no_predictable_temp_names_in_production_code() {
        // ipe-cli src, and the sibling ipe-wrapper src (the two crates that
        // construct jail scratch / embedded-app temp paths).
        let cli_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let wrapper_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .map(|p| p.join("ipe-wrapper").join("src"));

        for src_root in std::iter::once(cli_src).chain(wrapper_src) {
            let mut rs_files: Vec<std::path::PathBuf> = Vec::new();
            collect_rs_files(&src_root, &mut rs_files);
            for path in &rs_files {
                // The sanctioned scratch modules are the one place `temp_dir()`
                // may be joined (behind exclusive-create + entropy).
                if path.file_name().and_then(|n| n.to_str()) == Some("scratch.rs") {
                    continue;
                }
                // An out-of-line test module carries no inline `#[cfg(test)]`
                // marker for the region tracker, so a confirmed one is exempt
                // whole; an unconfirmed one stays in scope.
                let is_test_module = path
                    .strip_prefix(&src_root)
                    .is_ok_and(|rel| panic_scan::is_verified_test_path(&src_root, rel));
                if is_test_module {
                    continue;
                }
                // An unread file is unaudited, not clean.
                let source = std::fs::read_to_string(path);
                assert!(
                    source.is_ok(),
                    "cannot read {}: {source:?} — an unread file cannot be audited",
                    path.display()
                );
                let Ok(source) = source else { return };
                assert_predictable_temp_free(path, &source);
            }
        }
    }

    /// Assert `source` derives no temp path from `temp_dir()` in production code.
    ///
    /// Tracks `#[cfg(test)]`/`mod tests` regions by brace depth to exempt test
    /// helpers, and tracks production `let <ident> = ...temp_dir();` bindings so a
    /// later `<ident>.join(` (the split form) is caught as well as the inline
    /// `temp_dir().join(` form.  Also flags `temp_dir` passed as a bare function
    /// reference (e.g. `map_or_else(std::env::temp_dir, ...)` or
    /// `map_or_else(temp_dir, ...)`), which is semantically equivalent to the
    /// split form and was the original source of this defect class.
    fn assert_predictable_temp_free(path: &std::path::Path, source: &str) {
        let lines: Vec<&str> = source.lines().collect();
        let mut in_test_region = false;
        let mut brace_depth_at_test_entry: Option<usize> = None;
        let mut brace_depth: usize = 0;
        // Production idents currently bound to a `temp_dir()` value.
        let mut temp_bindings: std::collections::HashSet<String> = std::collections::HashSet::new();

        for (i, &line) in lines.iter().enumerate() {
            if line.contains("#[cfg(test)]") || line.contains("mod tests") {
                in_test_region = true;
                brace_depth_at_test_entry = Some(brace_depth);
            }
            brace_depth += line.chars().filter(|&c| c == '{').count();
            brace_depth = brace_depth.saturating_sub(line.chars().filter(|&c| c == '}').count());
            if in_test_region
                && let Some(entry_depth) = brace_depth_at_test_entry
                && brace_depth < entry_depth
            {
                in_test_region = false;
                brace_depth_at_test_entry = None;
                temp_bindings.clear();
            }

            if in_test_region {
                continue;
            }

            // Inline form: `temp_dir().join(...)` on one line.
            assert!(
                !line.contains("temp_dir().join"),
                "predictable temp_dir().join in production code at {}:{} — \
                 use ScratchDir or ScratchFile instead.\n  line: {}",
                path.display(),
                i + 1,
                line.trim()
            );

            // Fn-reference form: `temp_dir` used as a bare callable (without `()`)
            // in a combinator such as `map_or_else(std::env::temp_dir, ...)` or
            // `map_or_else(temp_dir, ...)`.  Both spellings route through `temp_dir`
            // without the `()` that the inline form requires, so they evade that
            // check; this clause closes the gap.
            let is_fn_ref = line.contains("map_or_else(std::env::temp_dir")
                || (line.contains("map_or_else(")
                    && line.contains("temp_dir")
                    && !line.contains("temp_dir()"));
            assert!(
                !is_fn_ref,
                "bare temp_dir fn-reference in production code at {}:{} — \
                 use ScratchDir::new_under with an explicit base instead.\n  line: {}",
                path.display(),
                i + 1,
                line.trim()
            );

            // Split form, part 1: record a `let <ident> = ...temp_dir();` binding
            // (that does not itself `.join`).
            if line.contains("temp_dir()")
                && !line.contains(".join")
                && let Some(ident) = binding_ident(line)
            {
                temp_bindings.insert(ident);
            }

            // Split form, part 2: a `.join(` on a recorded temp-derived binding.
            for ident in &temp_bindings {
                let joined = format!("{ident}.join(");
                assert!(
                    !line.contains(&joined),
                    "predictable split-form temp path (`{ident} = temp_dir(); {ident}.join(...)`) \
                     in production code at {}:{} — use ScratchDir or ScratchFile instead.\n  line: {}",
                    path.display(),
                    i + 1,
                    line.trim()
                );
            }
        }
    }

    /// Extract the bound identifier from a `let <ident> = ...;` line, if any.
    fn binding_ident(line: &str) -> Option<String> {
        let after_let = line.trim_start().strip_prefix("let ")?;
        let name = after_let
            .trim_start_matches("mut ")
            .split([' ', ':', '='])
            .next()?
            .trim();
        if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return None;
        }
        Some(name.to_owned())
    }

    fn collect_rs_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_rs_files(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                out.push(path);
            }
        }
    }

    /// The class gate flags the inline predictable-temp form in production code.
    #[test]
    fn class_gate_flags_inline_predictable_temp() {
        let injected = "fn make() -> std::path::PathBuf {\n    \
            std::env::temp_dir().join(\"predictable-name\")\n}\n";
        let caught = std::panic::catch_unwind(|| {
            assert_predictable_temp_free(std::path::Path::new("injected.rs"), injected);
        })
        .is_err();
        assert!(
            caught,
            "gate must flag inline temp_dir().join in production"
        );
    }

    /// The class gate flags the split predictable-temp form in production code.
    #[test]
    fn class_gate_flags_split_predictable_temp() {
        let injected = "fn make() -> std::path::PathBuf {\n    \
            let base = std::env::temp_dir();\n    \
            base.join(\"predictable-name\")\n}\n";
        let caught = std::panic::catch_unwind(|| {
            assert_predictable_temp_free(std::path::Path::new("injected.rs"), injected);
        })
        .is_err();
        assert!(
            caught,
            "gate must flag split-form temp_dir()+join in production"
        );
    }

    /// The class gate flags `temp_dir` passed as a bare fn reference in a
    /// `map_or_else` combinator (the fn-reference form).
    #[test]
    fn class_gate_flags_map_or_else_temp_dir_fn_ref() {
        let injected = "fn make(home: Option<PathBuf>) -> PathBuf {\n    \
            home.map_or_else(std::env::temp_dir, |h| h.join(\".cache\"))\n}\n";
        let caught = std::panic::catch_unwind(|| {
            assert_predictable_temp_free(std::path::Path::new("injected.rs"), injected);
        })
        .is_err();
        assert!(
            caught,
            "gate must flag map_or_else(std::env::temp_dir, ...) in production"
        );
    }

    /// The class gate flags the short `map_or_else(temp_dir, ...)` form (no
    /// path qualifier) the same way it flags the fully-qualified form.
    #[test]
    fn class_gate_flags_map_or_else_temp_dir_short() {
        let injected = "fn make(home: Option<PathBuf>) -> PathBuf {\n    \
            home.map_or_else(temp_dir, |h| h.join(\".cache\"))\n}\n";
        let caught = std::panic::catch_unwind(|| {
            assert_predictable_temp_free(std::path::Path::new("injected.rs"), injected);
        })
        .is_err();
        assert!(
            caught,
            "gate must flag map_or_else(temp_dir, ...) in production"
        );
    }

    /// The class gate passes on clean production code (a `ScratchDir`-based
    /// construction and a bare non-joined `temp_dir()` value) and on test-region
    /// predictable temps.
    #[test]
    fn class_gate_passes_clean_code() {
        let clean = "fn make() -> std::io::Result<()> {\n    \
            let _dir = ScratchDir::new(\"ipe-run\")?;\n    \
            let _base = std::env::temp_dir();\n    \
            Ok(())\n}\n\
            #[cfg(test)]\nmod tests {\n    \
            fn helper() { let _ = std::env::temp_dir().join(\"ok-in-test\"); }\n}\n";
        // Must not panic.
        assert_predictable_temp_free(std::path::Path::new("clean.rs"), clean);
    }

    /// The class gate passes on `ScratchDir::new_under` usage (the correct
    /// pattern for a caller-supplied base path).
    #[test]
    fn class_gate_passes_new_under() {
        let clean = "fn make(base: &Path) -> std::io::Result<ScratchDir> {\n    \
            ScratchDir::new_under(base, \"ipe-add\")\n}\n";
        // Must not panic.
        assert_predictable_temp_free(std::path::Path::new("clean.rs"), clean);
    }
}
