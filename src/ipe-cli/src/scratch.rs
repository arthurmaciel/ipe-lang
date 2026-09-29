//! Private scratch paths for temporary I/O, re-exported from the one shared primitive.
//!
//! [`ipe_sandbox::scratch`] owns creation and verification: a private 0700
//! directory under a verified base, files opened `O_EXCL` + `O_NOFOLLOW`, and
//! typed refusals. Read what an external writer wrote through
//! [`ScratchFile::read_all`], never by re-opening the path.

pub use ipe_sandbox::scratch::{LeafName, ScratchDir, ScratchFile};

/// The watcher mirrors the atomic-replace sibling suffix (it cannot depend on
/// the sandbox crate); the build breaks the instant the two drift.
const _: [(); 0] = [(); if bytes_eq(
    ipe_watch::scope::TEMP_SIBLING_SUFFIX.as_bytes(),
    ipe_sandbox::scratch::TEMP_SIBLING_SUFFIX.as_bytes(),
) {
    0
} else {
    1
}];

/// Byte equality usable in a `const` context.
const fn bytes_eq(a: &[u8], b: &[u8]) -> bool {
    match (a, b) {
        ([], []) => true,
        ([x, a @ ..], [y, b @ ..]) => *x == *y && bytes_eq(a, b),
        ([], [_, ..]) | ([_, ..], []) => false,
    }
}

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

    /// The sanctioned scratch modules, by path relative to their scanned root.
    ///
    /// The one place `temp_dir()` may be joined (behind exclusive-create +
    /// entropy). Exact names only — `scratch_helpers.rs` or `x/scratch.rs` is
    /// audited like any other file.
    const SANCTIONED_SCRATCH_MODULES: [&str; 3] =
        ["scratch.rs", "scratch_core.rs", "scratch_host.rs"];

    /// Whether root-relative `rel` is one of [`SANCTIONED_SCRATCH_MODULES`].
    fn is_sanctioned_scratch_module(rel: &std::path::Path) -> bool {
        SANCTIONED_SCRATCH_MODULES
            .iter()
            .any(|m| rel == std::path::Path::new(m))
    }

    /// Production lines exempt from the literal-`/tmp` rule.
    ///
    /// The base is only ever handed to `ScratchDir::new_under` (verified base,
    /// CSPRNG name, exclusive 0700 create). `(root-relative file, trimmed line)`;
    /// every entry must still match, so a stale exemption fails the gate.
    const TMP_LITERAL_EXEMPT: [(&str, &str); 1] = [(
        "ssrf.rs",
        ".chain(std::iter::once(PathBuf::from(\"/tmp\")))",
    )];

    /// No production code derives a temp path from `temp_dir()` or `/tmp`.
    ///
    /// Covers the `ipe-cli`, `ipe-wrapper`, runtime, and sandbox crates outside
    /// [`SANCTIONED_SCRATCH_MODULES`]: all such paths must go through a
    /// `scratch` module's exclusively-created constructors.
    ///
    /// Both the single-line form (`temp_dir().join(name)`) and the split form
    /// (`let base = temp_dir(); base.join(name)`) are caught: a bare
    /// `temp_dir()` binding in production is flagged the moment its value is
    /// `.join`-ed to build a path. A `"/tmp"` literal is flagged when it builds
    /// a path (`Path::new`, `PathBuf::from`, `.join`, `format!`) or is bound and
    /// later joined.
    ///
    /// Test-only code (`#[cfg(test)]` items / `mod tests` blocks) is exempt: test
    /// helpers that use predictable names in isolated temp dirs do not expose the
    /// verify/exec or verify/read identity gap that the production paths do.
    ///
    /// The walk fails closed: every root must exist and yield `.rs` files, and
    /// any unreadable directory or file fails the gate rather than shrinking it.
    #[test]
    fn no_predictable_temp_names_in_production_code() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let src = manifest.parent();
        assert!(src.is_some(), "{} has no parent", manifest.display());
        let Some(src) = src else { return };
        let roots = [
            manifest.join("src"),
            src.join("ipe-wrapper").join("src"),
            src.join("runtime").join("rust").join("src"),
            src.join("compiler").join("sandbox").join("src"),
        ];
        let mut exempt_hits = [0_usize; TMP_LITERAL_EXEMPT.len()];

        for src_root in &roots {
            let mut rs_files: Vec<std::path::PathBuf> = Vec::new();
            let walked = collect_rs_files(src_root, &mut rs_files);
            assert!(
                walked.is_ok(),
                "cannot walk {}: {walked:?} — an unwalked tree cannot be audited",
                src_root.display()
            );
            assert!(
                !rs_files.is_empty(),
                "{} yields no .rs files — a vanished root is not a clean one",
                src_root.display()
            );
            for path in &rs_files {
                let rel = path.strip_prefix(src_root);
                assert!(
                    rel.is_ok(),
                    "{} is outside {}",
                    path.display(),
                    src_root.display()
                );
                let Ok(rel) = rel else { return };
                if is_sanctioned_scratch_module(rel) {
                    continue;
                }
                // An out-of-line test module carries no inline `#[cfg(test)]`
                // marker for the region tracker, so a confirmed one is exempt
                // whole; an unconfirmed one stays in scope.
                if panic_scan::is_verified_test_path(src_root, rel) {
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
                let exempt: Vec<(usize, &str)> = TMP_LITERAL_EXEMPT
                    .iter()
                    .enumerate()
                    .filter(|(_, (file, _))| rel == std::path::Path::new(file))
                    .map(|(i, (_, line))| (i, *line))
                    .collect();
                let lines: Vec<&str> = exempt.iter().map(|(_, l)| *l).collect();
                for used in audit_predictable_temp(path, &source, &lines) {
                    if let Some((i, _)) = exempt.iter().find(|(_, l)| *l == used)
                        && let Some(hits) = exempt_hits.get_mut(*i)
                    {
                        *hits += 1;
                    }
                }
            }
        }
        for ((file, line), hits) in TMP_LITERAL_EXEMPT.iter().zip(exempt_hits) {
            assert!(
                hits > 0,
                "stale /tmp exemption {file}: `{line}` matched no line — remove it"
            );
        }
    }

    /// Assert `source` derives no temp path from `temp_dir()` or a literal `/tmp`
    /// in production code.
    fn assert_predictable_temp_free(path: &std::path::Path, source: &str) {
        let _ = audit_predictable_temp(path, source, &[]);
    }

    /// Whether `line` opens a test-only region: a `#[cfg(test)]` attribute or a
    /// `mod tests` item. Comments never do, so prose naming `mod tests` cannot
    /// exempt the production code after it.
    fn opens_test_region(line: &str) -> bool {
        let t = line.trim_start();
        !t.starts_with("//")
            && (t.starts_with("#[cfg(test)]")
                || ["mod tests", "pub mod tests", "pub(crate) mod tests"]
                    .iter()
                    .any(|p| t.starts_with(p)))
    }

    /// Whether `line` builds a path from a literal `/tmp` base.
    fn builds_tmp_literal_path(line: &str) -> bool {
        line.contains("\"/tmp")
            && ["Path::new(", "PathBuf::from(", ".join(", "format!("]
                .iter()
                .any(|c| line.contains(c))
    }

    /// Audit `source`, panicking on the first predictable temp path in
    /// production code. `exempt_tmp` lists trimmed lines allowed to name the
    /// literal `/tmp` base; returns the exempt lines that were hit.
    ///
    /// A test region opens at a line [`opens_test_region`] accepts and covers
    /// the item it decorates: through its closing `}` when a `{` opens first,
    /// or through its `;` when the item ends before any brace (`#[cfg(test)]
    /// use x;`), so a braceless test item never exempts what follows. Braces are
    /// counted textually: one inside a string or comment is miscounted (the
    /// documented limit of this line-level tracker). Production
    /// `let <ident> = ...temp_dir();` / `let <ident> = ..."/tmp"...` bindings are
    /// tracked so a later `<ident>.join(` (the split form) is caught as well as
    /// the inline forms. `temp_dir` passed as a bare function reference (e.g.
    /// `map_or_else(std::env::temp_dir, ...)`) is caught as the split form's
    /// equivalent.
    fn audit_predictable_temp<'e>(
        path: &std::path::Path,
        source: &str,
        exempt_tmp: &[&'e str],
    ) -> Vec<&'e str> {
        let mut used = Vec::new();
        let mut brace_depth: usize = 0;
        // Depth at the marker, and whether the decorated item opened its brace.
        let mut test_region: Option<(usize, bool)> = None;
        // Production idents currently bound to a temp base.
        let mut temp_bindings: std::collections::HashSet<String> = std::collections::HashSet::new();

        for (i, &line) in source.lines().enumerate() {
            if test_region.is_none() && opens_test_region(line) {
                test_region = Some((brace_depth, false));
            }
            let in_test_region = test_region.is_some();
            for c in line.chars() {
                match (c, test_region) {
                    ('{', Some((entry, false))) => {
                        brace_depth += 1;
                        test_region = Some((entry, true));
                    }
                    ('{', _) => brace_depth += 1,
                    ('}', Some((entry, true))) => {
                        brace_depth = brace_depth.saturating_sub(1);
                        if brace_depth <= entry {
                            test_region = None;
                            temp_bindings.clear();
                        }
                    }
                    ('}', _) => brace_depth = brace_depth.saturating_sub(1),
                    (';', Some((entry, false))) if brace_depth == entry => {
                        test_region = None;
                    }
                    _ => {}
                }
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

            // Literal `/tmp` base building a path, unless exempt.
            if builds_tmp_literal_path(line) {
                let exempt = exempt_tmp.iter().find(|e| line.trim() == **e);
                assert!(
                    exempt.is_some(),
                    "predictable literal /tmp path in production code at {}:{} — \
                     use temp_root() + ScratchDir::new_under instead.\n  line: {}",
                    path.display(),
                    i + 1,
                    line.trim()
                );
                if let Some(e) = exempt {
                    used.push(*e);
                }
            }

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

            // Split form, part 1: record a `let <ident> = ...temp_dir();` or
            // `let <ident> = ..."/tmp"...;` binding (that does not itself `.join`).
            if (line.contains("temp_dir()") || line.contains("\"/tmp"))
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
                    "predictable split-form temp path (`{ident} = <temp base>; {ident}.join(...)`) \
                     in production code at {}:{} — use ScratchDir or ScratchFile instead.\n  line: {}",
                    path.display(),
                    i + 1,
                    line.trim()
                );
            }
        }
        used
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

    /// Collect every `.rs` file under `dir`, failing on any unreadable directory
    /// or entry. Symlinks are not followed (no loop, no escape from the root).
    fn collect_rs_files(
        dir: &std::path::Path,
        out: &mut Vec<std::path::PathBuf>,
    ) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let kind = entry.file_type()?;
            if kind.is_dir() {
                collect_rs_files(&path, out)?;
            } else if kind.is_file() && path.extension().and_then(|e| e.to_str()) == Some("rs") {
                out.push(path);
            }
        }
        Ok(())
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

    /// Whether auditing `source` panics (the gate refuses it).
    fn gate_refuses(source: &str) -> bool {
        std::panic::catch_unwind(|| {
            assert_predictable_temp_free(std::path::Path::new("injected.rs"), source);
        })
        .is_err()
    }

    /// A literal `/tmp` base building a path is refused, inline and split.
    #[test]
    fn class_gate_flags_literal_tmp_paths() {
        for injected in [
            "fn make() -> PathBuf {\n    PathBuf::from(\"/tmp\").join(\"n\")\n}\n",
            "fn make() -> &'static Path {\n    Path::new(\"/tmp/ipe-fixed\")\n}\n",
            "fn make() -> String {\n    format!(\"/tmp/ipe-{}\", 1)\n}\n",
            "fn make() {\n    let root = \"/tmp\";\n    root.join(\"n\");\n}\n",
        ] {
            assert!(gate_refuses(injected), "gate must flag: {injected}");
        }
    }

    /// An exempt `/tmp` line passes and is reported as used; the same line not
    /// listed is refused.
    #[test]
    fn class_gate_tmp_exemption_is_exact_and_reported() {
        let line = ".chain(std::iter::once(PathBuf::from(\"/tmp\")))";
        let source = format!("fn bases() -> Vec<PathBuf> {{\n    roots\n        {line}\n}}\n");
        let used = audit_predictable_temp(std::path::Path::new("ssrf.rs"), &source, &[line]);
        assert_eq!(used, vec![line]);
        assert!(gate_refuses(&source), "an unlisted /tmp line is refused");
    }

    /// A braceless `#[cfg(test)]` item and prose naming `mod tests` exempt
    /// nothing after them.
    #[test]
    fn class_gate_test_region_never_leaks_into_production() {
        for injected in [
            "#[cfg(test)]\nuse std::env::temp_dir;\nfn make() -> PathBuf {\n    \
             std::env::temp_dir().join(\"n\")\n}\n",
            "// helpers live in mod tests below\nfn make() -> PathBuf {\n    \
             std::env::temp_dir().join(\"n\")\n}\n",
            "#[cfg(test)]\nmod tests;\nfn make() -> PathBuf {\n    \
             std::env::temp_dir().join(\"n\")\n}\n",
            "#[cfg(test)]\nmod tests {\n    fn t() {}\n}\nfn make() -> PathBuf {\n    \
             std::env::temp_dir().join(\"n\")\n}\n",
        ] {
            assert!(gate_refuses(injected), "gate must flag: {injected}");
        }
    }

    /// Only the exact sanctioned module paths are exempt.
    #[test]
    fn class_gate_sanctions_exact_scratch_modules_only() {
        for ok in SANCTIONED_SCRATCH_MODULES {
            assert!(
                is_sanctioned_scratch_module(std::path::Path::new(ok)),
                "{ok}"
            );
        }
        for audited in [
            "scratch_helpers.rs",
            "scratchy.rs",
            "web/scratch.rs",
            "scratch.rs.bak",
        ] {
            assert!(
                !is_sanctioned_scratch_module(std::path::Path::new(audited)),
                "{audited} must be audited"
            );
        }
    }

    /// A missing root fails the walk rather than yielding an empty, clean tree.
    #[test]
    fn class_gate_walk_fails_closed_on_a_missing_root() {
        let root = ScratchDir::new("ipe-gate-walk").expect("scratch dir");
        let mut files = Vec::new();
        assert!(collect_rs_files(&root.path().join("absent"), &mut files).is_err());
        assert!(files.is_empty());
    }
}
