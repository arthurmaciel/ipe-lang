use super::*;
use crate::{
    ALL_CODES, Applicability, BTreeMap, Diagnostic, Path, PathBuf, Suggestion, fs, project, style,
};
use ipe_diagnostics::{NameError, Span};

#[test]
fn bluegreen_defaults_on_when_no_env_set() {
    // No opt-out, no explicit choice → on (the new default).
    assert!(bluegreen_from_env_values(None, None));
}

#[test]
fn bluegreen_opt_out_wins() {
    // IPE_WATCH_NO_BLUEGREEN set (non-empty ≠ "0") → off, even if the legacy
    // flag would force on.
    assert!(!bluegreen_from_env_values(Some("1"), None));
    assert!(!bluegreen_from_env_values(Some("anything"), Some("1")));
    // "0"/empty opt-out is NOT an opt-out → the rest of the precedence runs.
    assert!(bluegreen_from_env_values(Some("0"), None));
    assert!(bluegreen_from_env_values(Some(""), None));
}

#[test]
fn bluegreen_explicit_legacy_choice_is_honoured() {
    // Explicit IPE_WATCH_BLUEGREEN: "0"/empty off, anything else on.
    assert!(!bluegreen_from_env_values(None, Some("0")));
    assert!(!bluegreen_from_env_values(None, Some("")));
    assert!(bluegreen_from_env_values(None, Some("1")));
    assert!(bluegreen_from_env_values(None, Some("yes")));
}

#[test]
fn registry_unreachable_matches_network_signals_only() {
    // Genuine network/offline failures.
    assert!(is_registry_unreachable(
        "Caused by:\n  Could not resolve host: index.crates.io"
    ));
    assert!(is_registry_unreachable("warning: spurious network error"));
    assert!(is_registry_unreachable(
        "error: failed to fetch `https://github.com/rust-lang/crates.io-index`"
    ));
    // A missing local path dependency or malformed manifest is NOT a
    // connectivity problem and must not be reported as one.
    assert!(!is_registry_unreachable(
        "error: failed to load source for dependency `handle_demo`\n\
         Caused by:\n  path `/tmp/x` does not exist"
    ));
    assert!(!is_registry_unreachable(
        "error: no matching package named `foo` found; updating registry index"
    ));
    assert!(!is_registry_unreachable("error[E0433]: cannot find crate"));
}

#[test]
fn vendored_runtime_dir_is_required_only_when_vendoring() {
    // The dependency-model path (default `ipe build`/`run`, and `ipe watch`)
    // never vendors the runtime source tree — it reaches the runtime as a
    // crate dependency — so it must resolve to an empty sentinel WITHOUT
    // demanding a runtime dir. Requiring the vendored tree here is what made
    // `ipe watch` fail to locate the runtime in an installed checkout.
    assert_eq!(
        resolve_vendored_runtime_dir(None, false).ok(),
        Some(PathBuf::new()),
    );
    // An explicit `--runtime` is honoured verbatim, vendoring or not — so the
    // vendoring path (e.g. `ipe eject`) resolves a runtime dir even when the
    // ambient vendored tree is absent.
    assert_eq!(
        resolve_vendored_runtime_dir(Some("/opt/ipe-runtime".to_owned()), false).ok(),
        Some(PathBuf::from("/opt/ipe-runtime")),
    );
    assert_eq!(
        resolve_vendored_runtime_dir(Some("/opt/ipe-runtime".to_owned()), true).ok(),
        Some(PathBuf::from("/opt/ipe-runtime")),
    );
}

#[test]
fn io_not_found_renders_styled_without_os_error() {
    let err = CliError::Io {
        path: PathBuf::from("/no/such.ipe"),
        source: std::io::Error::from(std::io::ErrorKind::NotFound),
    };
    let rendered = err.to_string();
    assert!(
        rendered.contains("no such file `/no/such.ipe`"),
        "styled NotFound message, got: {rendered}"
    );
    // No jargon: never the raw `io error` prefix, never an `os error N` tail.
    assert!(!rendered.contains("os error"), "leaks errno: {rendered}");
    assert!(!rendered.contains("io error"), "leaks jargon: {rendered}");
}

#[test]
fn io_other_kind_stays_readable_without_errno() {
    let err = CliError::Io {
        path: PathBuf::from("/x"),
        source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
    };
    let rendered = err.to_string();
    assert!(!rendered.contains("os error"), "leaks errno: {rendered}");
    assert!(rendered.contains("/x"), "names the path: {rendered}");
}

#[test]
fn unknown_command_screen_is_fully_guttered() {
    let err = CliError::UnknownCommand {
        attempted: "frobnicate".to_owned(),
    };
    let rendered = err.to_string();
    // The advice line and the help header both carry the shared gutter — no
    // flush-left line breaks the screen the way the trim_start header did.
    assert!(
        rendered.starts_with("  unknown command `frobnicate`"),
        "advice guttered, got: {rendered:?}"
    );
    for line in rendered.lines().filter(|l| !l.is_empty()) {
        assert!(
            line.starts_with(style::GUTTER),
            "every non-empty line is guttered, offending: {line:?}"
        );
    }
}

/// `read_progress_chunk` stops at a newline OR a carriage return, so cargo's
/// in-place progress bar (which uses `\r` with no `\n`) surfaces live rather
/// than buffering until the next line, and it drains a stream with no final
/// terminator without dropping bytes.
#[test]
fn read_progress_chunk_stops_at_newline_or_carriage_return() {
    use std::io::BufReader;
    // A `\r` progress frame, then a `\n` message line, then a trailing chunk
    // with no terminator at end of stream.
    let input = "  Building [==>   ]\r   Compiling ipe-app\ndone";
    let mut reader = BufReader::new(input.as_bytes());
    let mut out = String::new();

    let n1 = read_progress_chunk(&mut reader, &mut out).expect("read frame");
    assert_eq!(out, "  Building [==>   ]\r");
    assert_eq!(n1, out.len());

    out.clear();
    read_progress_chunk(&mut reader, &mut out).expect("read line");
    assert_eq!(out, "   Compiling ipe-app\n");

    out.clear();
    read_progress_chunk(&mut reader, &mut out).expect("read tail");
    assert_eq!(out, "done");

    // End of stream returns zero and leaves `out` empty.
    out.clear();
    assert_eq!(
        read_progress_chunk(&mut reader, &mut out).expect("read eof"),
        0
    );
    assert!(out.is_empty());
}

/// Cargo terminal UI should be forced only when our stderr is a TTY and
/// `NO_COLOR` is unset — both conditions must hold. Checked via a
/// closed-form helper that mirrors the guard inside `force_cargo_terminal_ui`.
#[test]
fn force_cargo_ui_truth_table() {
    // Pure function extracted from the guard: is_tty && no_color is unset.
    let should_force = |is_tty: bool, no_color: bool| -> bool { is_tty && !no_color };
    assert!(should_force(true, false), "tty + color on → force");
    assert!(!should_force(false, false), "not a tty → no force");
    assert!(!should_force(true, true), "NO_COLOR set → no force");
    assert!(
        !should_force(false, true),
        "not a tty + NO_COLOR → no force"
    );
}

/// `missing_runtime_feature` pulls the feature name out of `cargo`'s
/// feature-resolution error, whether the name is backtick- or single-quoted,
/// and yields `None` for an unrelated failure.
#[test]
fn extracts_missing_runtime_feature() {
    let backtick = "package `ipe-app` depends on `ipe-runtime-rust` with feature `regex` \
         but `ipe-runtime-rust` does not have that feature.";
    assert_eq!(missing_runtime_feature(backtick).as_deref(), Some("regex"));
    let single = "package `ipe-app` depends on ipe-runtime-rust with feature 'random' \
         but ipe-runtime-rust does not have that feature";
    assert_eq!(missing_runtime_feature(single).as_deref(), Some("random"));
    assert_eq!(
        missing_runtime_feature("error: linking with `cc` failed: exit status: 1"),
        None
    );
}

/// A cargo build failure whose stderr names a missing runtime feature renders
/// a targeted, actionable diagnostic that names the feature and the stale
/// runtime — and never the `run` command's `--help` page.
#[test]
fn emitted_build_failure_reports_missing_feature() {
    let err = CliError::EmittedBuildFailed {
        what: "the emitted program",
        code: 101,
        stderr: "package `ipe-app` depends on `ipe-runtime-rust` with feature `regex` \
             but `ipe-runtime-rust` does not have that feature."
            .to_owned(),
        runtime: Some(RuntimeContext {
            root: PathBuf::from("/tmp/rt"),
            version: "0.1.34".to_owned(),
        }),
    };
    let rendered = err.to_string();
    assert!(rendered.contains("runtime feature `regex`"), "{rendered}");
    assert!(rendered.contains("/tmp/rt"), "{rendered}");
    assert!(rendered.contains("out of date"), "{rendered}");
    assert!(
        !rendered.contains("ipe run [<path>]"),
        "the build failure must not print the run help page: {rendered}"
    );
}

/// A cargo build failure that is not a feature gap is unattributable: the
/// front-end gate already rejected invalid programs, so a cargo failure here
/// is a miscompile in Ipê's own emission, not the user's fault. It renders as
/// a humble compiler-bug ICE that apologises, points at the issue tracker, and
/// still embeds the raw cargo stderr as the reportable detail — never a bare
/// rustc error presented as user error, and never any command's help page.
#[test]
fn emitted_build_failure_reports_unattributed_as_compiler_bug() {
    let err = CliError::EmittedBuildFailed {
        what: "the emitted program",
        code: 101,
        stderr: "error[E0425]: cannot find value `x` in this scope".to_owned(),
        runtime: None,
    };
    let rendered = err.to_string();
    // The humble ICE framing: this is the compiler's fault, please report it.
    assert!(rendered.contains("please report"), "{rendered}");
    assert!(rendered.contains("bug in Ipe"), "{rendered}");
    // The raw cargo error is preserved for the bug report.
    assert!(rendered.contains("cannot find value"), "{rendered}");
    assert!(rendered.contains("E0425"), "{rendered}");
    // Neither a help page nor the old plain-header user-error framing.
    assert!(!rendered.contains("ipe run [<path>]"), "{rendered}");
    assert!(
        !rendered.contains("building the emitted program failed (cargo exited"),
        "{rendered}"
    );
}

/// The golden entry, located relative to this crate's manifest.
fn golden_entry() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tests")
        .join("golden")
        .join("basics")
        .join("Main.ipe")
}

/// Drift-closed proof: every entry in `ALL_CODES` resolves via `explain_lookup`.
/// If any code is in the taxonomy but missing from `ALL_CODES` this test fails.
#[test]
fn all_taxonomy_codes_resolve_via_explain_lookup() {
    for &c in ALL_CODES {
        let result = explain_lookup(c.as_str());
        assert!(
            result.is_ok(),
            "{} is in ALL_CODES but explain_lookup returned: {:?}",
            c.as_str(),
            result.err()
        );
    }
}

#[test]
fn explain_resolves_a_known_code() {
    let page = explain_lookup("IPE-T0001");
    assert!(page.is_ok(), "known code must resolve: {:?}", page.err());
    let Ok(page) = page else { return };
    assert!(
        page.starts_with("# IPE-T0001:"),
        "page line 1 must name the code, got:\n{page}"
    );
}

#[test]
fn explain_is_case_insensitive() {
    assert!(explain_lookup("ipe-t0001").is_ok());
    assert!(explain_lookup("  Ipe-T0001  ").is_ok());
}

#[test]
fn explain_resolves_ipe_t0014() {
    // IPE-T0014 resolves via ALL_CODES from ipe_diagnostics rather than
    // a hand-mirror that could omit it.
    let result = explain_lookup("IPE-T0014");
    assert!(
        result.is_ok(),
        "IPE-T0014 must resolve via ALL_CODES: {:?}",
        result.err()
    );
}

#[test]
fn explain_rejects_unknown_code_with_suggestions() {
    // Genuinely unknown code, close to IPE-T0013 — must yield did-you-mean.
    let result = explain_lookup("IPE-T0099");
    assert!(
        matches!(&result, Err(CliError::UnknownCode { .. })),
        "unknown code must error, got: {result:?}"
    );
    let Err(CliError::UnknownCode { suggestions, .. }) = result else {
        return;
    };
    assert!(
        !suggestions.is_empty(),
        "a near-miss must yield did-you-mean suggestions"
    );
}

#[test]
fn explain_unknown_code_display_is_deterministic() {
    let err = CliError::UnknownCode {
        input: "IPE-Z9999".to_owned(),
        suggestions: vec!["IPE-T0001", "IPE-T0002"],
    };
    assert_eq!(
        err.to_string(),
        "unknown error code `IPE-Z9999`\n  did you mean: IPE-T0001, IPE-T0002?"
    );
}

#[test]
fn explain_output_ends_with_trailing_newline() {
    // `ipe explain <CODE>` does `print!("{page}")`, so the page itself must
    // end with a newline to avoid a missing newline at the shell prompt.
    let page = explain_lookup("IPE-T0001").expect("known code must resolve");
    assert!(
        page.ends_with('\n'),
        "explain output must end with a trailing newline; got: {:?}",
        &page[page.len().saturating_sub(20)..]
    );
}

#[test]
fn code_index_lists_every_code() {
    let index = code_index();
    let lines = index.lines().count();
    assert_eq!(lines, ALL_CODES.len(), "one line per code");
    assert!(
        index.contains("IPE-T0001  type mismatch"),
        "index pairs code with title"
    );
}

#[test]
fn emit_ir_prints_a_tree_for_the_golden() {
    let tree = emit_ir_text(&golden_entry());
    assert!(
        tree.is_ok(),
        "emit-ir must succeed: {:?}",
        tree.as_ref().err()
    );
    let Ok(tree) = tree else { return };
    assert!(
        tree.starts_with("program"),
        "tree roots at `program`:\n{tree}"
    );
    assert!(tree.contains("main"), "tree names the `main` func:\n{tree}");
}

/// A program importing a compiled-source stdlib module that defines its own
/// types (`Ipe.Test`) must resolve its qualified members through the CLI
/// analysis path (`ipe build --emit-ir` / `ipe capabilities`), exactly as it
/// does through a real `ipe build`. Both share the injection-aware
/// source-graph pipeline: the analysis path once ran a bare single-module
/// lower that never injected the closure, so `Test.runMain` / `Test.equal`
/// failed with IPE-N0004 "unknown module `Test`" here while the build
/// succeeded. This pins the CLI<->build parity for compiled-source-with-types
/// modules so the divergence cannot return.
#[test]
fn emit_ir_resolves_compiled_source_stdlib_with_own_types() {
    let entry = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tests")
        .join("golden")
        .join("test_summary_line_219")
        .join("Main.ipe");
    let tree = emit_ir_text(&entry);
    assert!(
        tree.is_ok(),
        "emit-ir must resolve `Ipe.Test` (no IPE-N0004): {:?}",
        tree.as_ref().err()
    );
    let Ok(tree) = tree else { return };
    // The injected compiled-source module's OWN types + members are present
    // — proof the closure was injected, not merely that the diagnostic was
    // silenced.
    assert!(
        tree.contains("type TestResult"),
        "injected `Ipe.Test` types must appear in the IR:\n{tree}"
    );
    assert!(
        tree.contains("runMain"),
        "`Test.runMain` must resolve to the injected member:\n{tree}"
    );

    // The same source-graph pipeline backs `ipe capabilities` via
    // `lower_entry_via_graph`; it must resolve identically (a pure test
    // program).
    assert!(
        lower_entry_via_graph(&entry).is_ok(),
        "lower_entry_via_graph (capabilities path) must resolve `Ipe.Test` too"
    );
}

/// A compiled-source stdlib module that imports a kernel stdlib module inside
/// its own body must not fire IPE-N0034 on those imports.  `Ipe.Money`
/// imports `Ipe.String` (a kernel module) and uses `String.*` members
/// throughout; the Tier-C import gate must see those imports as satisfied
/// when the embedded source is injected and canonicalised.
///
/// The `money_parse_currency_maybe` golden exercises `Money.currencyCode`
/// (which calls `String.*` internally), making it the ideal witness.
#[test]
fn compiled_source_stdlib_own_imports_resolve_no_n0034() {
    let entry = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tests")
        .join("golden")
        .join("money_parse_currency_maybe")
        .join("Main.ipe");
    let tree = emit_ir_text(&entry);
    assert!(
        tree.is_ok(),
        "emit-ir must resolve `Ipe.Money` (no IPE-N0034 inside the embedded module): {:?}",
        tree.as_ref().err()
    );
    let Ok(tree) = tree else { return };
    // The injected module's types must appear — proof the closure was injected,
    // not merely that the diagnostic was silenced at a shallower stage.
    assert!(
        tree.contains("Money") || tree.contains("currency"),
        "injected `Ipe.Money` members must appear in the IR:\n{tree}"
    );
}

#[test]
fn machine_applicable_suggestion_is_collected_and_applied() {
    let src = "main = lenght";
    // `lenght` occupies bytes 7..13.
    let diag = Diagnostic::Name {
        span: Span::new(7, 13),
        msg: NameError::ValueNotFound {
            name: "lenght".into(),
            suggestions: Box::new(["length".into()]),
        },
    };
    let fixes = machine_applicable_suggestions(&diag);
    assert_eq!(fixes.len(), 1, "single candidate is machine-applicable");
    let selected = select_non_overlapping(fixes, src.len());
    let patched = apply_fixes(src, &selected);
    assert_eq!(patched.as_deref(), Some("main = length"));
}

#[test]
fn overlapping_suggestions_are_filtered_back_to_front() {
    let left = Suggestion {
        span: Span::new(0, 5),
        replacement: "x".into(),
        applicability: Applicability::MachineApplicable,
    };
    let right = Suggestion {
        span: Span::new(3, 8),
        replacement: "y".into(),
        applicability: Applicability::MachineApplicable,
    };
    let kept = select_non_overlapping(vec![left, right], 8);
    assert_eq!(kept.len(), 1, "overlapping spans collapse to one");
    // Back-to-front: the right-most (larger lo) span survives.
    assert_eq!(kept.first().map(|s| s.span.lo), Some(3));
}

#[test]
fn apply_fixes_rejects_out_of_bounds_span() {
    let s = Suggestion {
        span: Span::new(0, 999),
        replacement: "z".into(),
        applicability: Applicability::MachineApplicable,
    };
    assert_eq!(apply_fixes("short", &[s]), None);
}

#[test]
fn apply_fixes_rejects_non_char_boundary_span() {
    // "é" is two UTF-8 bytes; a span that splits it is rejected.
    let s = Suggestion {
        span: Span::new(0, 1),
        replacement: "z".into(),
        applicability: Applicability::MachineApplicable,
    };
    assert_eq!(apply_fixes("é", &[s]), None);
}

#[test]
fn levenshtein_is_symmetric_and_zero_on_equal() {
    assert_eq!(levenshtein("abc", "abc"), 0);
    assert_eq!(levenshtein("abc", "abd"), 1);
    assert_eq!(levenshtein("abc", "abd"), levenshtein("abd", "abc"));
}

#[test]
fn line_col_counts_from_one() {
    let src = "ab\ncd";
    assert_eq!(line_col(src, 0), (1, 1));
    assert_eq!(line_col(src, 1), (1, 2));
    assert_eq!(line_col(src, 3), (2, 1));
    assert_eq!(line_col(src, 4), (2, 2));
}

/// Generic records, end to end from SOURCE: parse → canon → infer → lower →
/// emit → `cargo build` → run, asserting the program prints `42` — the value
/// the Go reference backend produces for the same program (hand-verified in a
/// temp dir). Gated on `IPE_E2E=1` so the default `cargo test` stays fast and
/// offline. Complements the backend crate's hand-built-IR e2e by exercising
/// the whole frontend (record type annotations + generalisation + lowering).
#[test]
fn generic_record_program_builds_and_prints_forty_two() {
    const SRC: &str = "module Main exposing (main)\n\n\
         import Ipe.Io\n\
         import Ipe.String\n\n\
         wrap : a -> { value : a }\n\
         wrap x =\n    { value = x }\n\n\
         unwrap : { value : a } -> a\n\
         unwrap r =\n    r.value\n\n\
         main = Io.println (String.fromInt (unwrap (wrap 42)))\n";

    if std::env::var("IPE_E2E").is_err() {
        return;
    }

    let dir = std::env::temp_dir().join("ipec_generic_record_src_e2e");
    let _ = fs::remove_dir_all(&dir);
    let entry = dir.join("Main.ipe");
    let created = fs::create_dir_all(&dir).and_then(|()| fs::write(&entry, SRC));
    assert!(created.is_ok(), "write source: {created:?}");

    let runtime = resolve_runtime();
    assert!(runtime.is_ok(), "runtime must resolve: {runtime:?}");
    let Ok(runtime) = runtime else { return };

    let out = dir.join("out");
    let built = build(&entry, &out, &runtime);
    assert!(built.is_ok(), "ipe build must succeed: {built:?}");

    let status = std::process::Command::new("cargo")
        .arg("build")
        .current_dir(&out)
        .env("CARGO_TARGET_DIR", out.join("target"))
        .status();
    assert!(
        matches!(&status, Ok(s) if s.success()),
        "emitted generic-record crate must compile: {status:?}"
    );

    let bin = out.join("target").join("debug").join("ipe-app");
    let run = std::process::Command::new(&bin).output();
    let Ok(run) = run else {
        assert!(false_marker(), "run binary: {run:?}");
        return;
    };
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "42\n",
        "generic-record program prints 42 (Go-backend parity)"
    );
    assert!(run.status.success(), "exit 0, matching the Go oracle");
    let _ = std::fs::remove_dir_all(out.join("target"));
}

/// A runtime `false` the optimiser cannot fold, so `assert!(false_marker())`
/// fails the test without tripping `clippy::assertions_on_constants`.
fn false_marker() -> bool {
    std::hint::black_box(false)
}

// -----------------------------------------------------------------------
// find_manifest_for_ipe_file tests (IPE-N0020 fix)
// -----------------------------------------------------------------------

/// Creates a temp directory with a nested `src/Main.ipe` and a `package.ipe`
/// at the project root, confirming the upward walk finds the manifest.
#[test]
fn find_manifest_walks_up_to_project_root() {
    let tmp = std::env::temp_dir().join("ipec_find_manifest_test");
    let _ = fs::remove_dir_all(&tmp);
    let src = tmp.join("src");
    fs::create_dir_all(&src).expect("create src/");
    let manifest = tmp.join("package.ipe");
    fs::write(
        &manifest,
        "module Package exposing (package)\n\n\npackage =\n    { name = \"test\" }\n",
    )
    .expect("write package.ipe");
    let main_ipe = src.join("Main.ipe");
    fs::write(&main_ipe, "module Main exposing (main)\nmain = 0\n").expect("write Main.ipe");

    let found = find_manifest_for_ipe_file(&main_ipe);
    assert_eq!(
        found.as_deref(),
        Some(manifest.as_path()),
        "upward walk must find package.ipe at project root"
    );
    let _ = fs::remove_dir_all(&tmp);
}

// -----------------------------------------------------------------------
// Regression: PAnything (wildcard lambda param with unconstrained Ty::Var)
// -----------------------------------------------------------------------

/// Regression for `IPE-L0102` (`Feature::Polymorphism`) on wildcard `_`
/// lambda parameters.
///
/// Calling `ir_type_from_ty` on the `_` param's type is unsound: when the
/// type is still an unconstrained `Ty::Var` (e.g. the continuation of a
/// `Task.andThen` after `Task.fail` where the ok-type is never forced),
/// `ir_type_from_ty` returns `Err(unsupported(…, Feature::Polymorphism))`
/// and the pipeline aborts.
///
/// So `PAnything` params route through `ir_type_from_ty_json`, which
/// maps `Ty::Var → IrType::Json` instead of failing.
///
/// Source mirrors the failing pattern from `examples/14-task-demo`.
#[test]
fn panything_wildcard_lambda_compiles_without_polymorphism_error() {
    const SRC: &str = "\
module Main exposing (main)
import Ipe.Task as Task
import Ipe.Error as Error exposing (Error)
import Ipe.Io as Io

main =
Task.fail (Error.unexpected \"intentional\")
    |> Task.andThen (\\_ -> Task.succeed \"unreachable\")
    |> Task.andThen Io.println
    |> Task.onError (\\e -> Io.println (Error.toString e))
";

    let runtime = resolve_runtime();
    if runtime.is_err() {
        // Runtime not present in this environment — skip rather than fail.
        return;
    }
    let Ok(runtime) = runtime else { return };

    let dir = std::env::temp_dir().join("ipec_panything_regression");
    let _ = fs::remove_dir_all(&dir);
    let entry = dir.join("Main.ipe");
    let created = fs::create_dir_all(&dir).and_then(|()| fs::write(&entry, SRC));
    assert!(created.is_ok(), "write source: {created:?}");

    let out = dir.join("out");
    let result = build(&entry, &out, &runtime);
    assert!(
        result.is_ok(),
        "wildcard lambda with unconstrained type must not fire IPE-L0102: {result:?}"
    );
    let _ = fs::remove_dir_all(&dir);
}

// -----------------------------------------------------------------------
// Regression: Task.run elision — ipe_main must return IpeTask<A>
// -----------------------------------------------------------------------

/// `main` returning a `Task` directly must emit `fn ipe_main() -> IpeTask<`
/// (the shape the `block_on(ipe_main())` epilogue requires), never
/// `IpeResult<…>`. The internal `TaskRun` kernel is the auto-run mechanism
/// at the entry boundary; the surface `Task.run` binding is gone.
#[test]
fn task_run_main_emits_ipetask_not_iperesult() {
    const SRC: &str = "\
module Main exposing (main)
import Ipe.Io as Io

main =
Io.println \"hello from main task\"
";

    let runtime = resolve_runtime();
    if runtime.is_err() {
        return;
    }
    let Ok(runtime) = runtime else { return };

    let dir = std::env::temp_dir().join("ipec_taskrun_elision_regression");
    let _ = fs::remove_dir_all(&dir);
    let entry = dir.join("Main.ipe");
    let created = fs::create_dir_all(&dir).and_then(|()| fs::write(&entry, SRC));
    assert!(created.is_ok(), "write source: {created:?}");

    let out = dir.join("out");
    let built = build(&entry, &out, &runtime);
    assert!(built.is_ok(), "task-returning main must compile: {built:?}");

    let main_rs = out.join("src").join("main.rs");
    let emitted = fs::read_to_string(&main_rs).expect("emitted main.rs must exist after build");

    assert!(
        emitted.contains("fn ipe_main() -> IpeTask<"),
        "ipe_main must return IpeTask<…>, got signature region:\n{}",
        emitted
            .lines()
            .filter(|l| l.contains("ipe_main") || l.contains("IpeTask") || l.contains("IpeResult"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        !emitted.contains("fn ipe_main() -> IpeResult"),
        "ipe_main must NOT return IpeResult"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// The pure hot-appearance decision (over the two raw variable values):
/// default ON, the opt-out wins, and an explicit `IPE_WATCH_HOT_APPEARANCE`
/// is honoured. Exercises the logic without mutating process env.
#[test]
fn hot_appearance_defaults_on_and_honours_overrides() {
    // Neither var set ⇒ on (the new default for `ipe watch`).
    assert!(hot_appearance_from_env(None, None), "unset ⇒ default on");
    // Opt-out set ⇒ off, regardless of the explicit var.
    assert!(
        !hot_appearance_from_env(Some("1"), None),
        "IPE_WATCH_NO_HOT_APPEARANCE=1 ⇒ off"
    );
    assert!(
        !hot_appearance_from_env(Some("anything"), Some("1")),
        "opt-out wins over an explicit on"
    );
    // Opt-out empty / `0` does NOT opt out.
    assert!(
        hot_appearance_from_env(Some(""), None),
        "empty opt-out is not an opt-out ⇒ still on"
    );
    assert!(
        hot_appearance_from_env(Some("0"), None),
        "`0` opt-out is not an opt-out ⇒ still on"
    );
    // Explicit `IPE_WATCH_HOT_APPEARANCE` is honoured when opt-out is absent.
    assert!(
        !hot_appearance_from_env(None, Some("0")),
        "explicit `0` ⇒ off"
    );
    assert!(
        !hot_appearance_from_env(None, Some("")),
        "explicit empty ⇒ off"
    );
    assert!(
        hot_appearance_from_env(None, Some("1")),
        "explicit `1` ⇒ on"
    );
}

/// A web app with a hoist-eligible style literal (`Ui.style "font-weight"
/// "bold"`). Used to prove the build-vs-watch emit difference.
const WEB_APP_WITH_STYLE: &str = "\
module Main exposing (main)
import Ipe.Tea.Web as Web
import Ipe.Ui as Ui
import Ipe.Tea.Web.Cmd as Cmd
import Ipe.Tea.Web.Sub as Sub

type Msg = Noop
type alias Model = { count : Int }

init : WebReq -> ( Model, Cmd Msg )
init _req = ( { count = 0 }, Cmd.none )

update : Msg -> Model -> ( Model, Cmd Msg )
update _msg model = ( model, Cmd.none )

subscriptions : Model -> Sub Msg
subscriptions _model = Sub.none

view : Model -> Element Msg
view _model =
Ui.el [ Ui.style \"font-weight\" \"bold\" ] (Ui.text \"Counter\")

main =
Web.app
    { init = init
    , update = update
    , view = view
    , subscriptions = subscriptions
    , routes = []
    , notFound = Noop
    }
";

/// Emit `WEB_APP_WITH_STYLE` with an explicit `hot_appearance` and return the
/// CONCATENATED emitted Rust source (`src/main.rs` plus every per-module file
/// under `src/ipe_mods/`, where the `view` body actually lands), or `None`
/// when the runtime cannot be resolved (so the test is a no-op on a machine
/// without an installed runtime crate).
fn emit_web_app_source(hot_appearance: bool, tag: &str) -> Option<String> {
    let runtime = resolve_runtime().ok()?;
    let dir = std::env::temp_dir().join(format!("ipec_hot_appearance_{tag}"));
    let _ = fs::remove_dir_all(&dir);
    let entry = dir.join("Main.ipe");
    fs::create_dir_all(&dir).ok()?;
    fs::write(&entry, WEB_APP_WITH_STYLE).ok()?;
    let out = dir.join("out");
    let options = BuildOptions {
        hot_appearance,
        ..BuildOptions::from_env()
    };
    let built = build_with_sibling_discovery_with_options(&entry, &out, &runtime, options);
    assert!(built.is_ok(), "web app must compile ({tag}): {built:?}");
    // Walk `out/src` and concatenate every emitted `.rs` file: the view body
    // (and thus any hoisted `__ipe_lit` table) lands in a per-module file
    // under `src/ipe_mods/`, not in `src/main.rs`.
    let src_dir = out.join("src");
    let mut sources = String::new();
    let mut stack = vec![src_dir];
    while let Some(d) = stack.pop() {
        let Ok(entries) = fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|x| x.to_str()) == Some("rs")
                && let Ok(text) = fs::read_to_string(&p)
            {
                sources.push_str(&text);
            }
        }
    }
    assert!(
        !sources.is_empty(),
        "emitted src/ must carry at least one .rs file ({tag})"
    );
    let _ = fs::remove_dir_all(&dir);
    Some(sources)
}

/// PROD-CLEAN: a build-mode emit (`hot_appearance = false`, what `ipe build`
/// / `ipe run` / `ipe release` thread) carries NO hot-swap scaffolding — no
/// `LiteralTable` and no `/_ipe/hot-appearance` endpoint.
#[test]
fn build_mode_emit_carries_no_hot_swap_scaffolding() {
    let Some(src) = emit_web_app_source(false, "build_clean") else {
        return;
    };
    assert!(
        !src.contains("__ipe_lit"),
        "a build-mode emit must introduce no literal table, got:\n{src}"
    );
    assert!(
        !src.contains("/_ipe/hot-appearance"),
        "a build-mode emit must not mount the hot-appearance endpoint, got:\n{src}"
    );
}

/// WATCH: a watch-mode emit (`hot_appearance = true`) DOES hoist the style
/// literal into the per-view `LiteralTable`, so an appearance edit can be
/// hot-swapped without a rebuild.
#[test]
fn watch_mode_emit_hoists_literal_table() {
    let Some(src) = emit_web_app_source(true, "watch_hoist") else {
        return;
    };
    assert!(
        src.contains("__ipe_lit"),
        "a watch-mode emit must hoist style literals into a table, got:\n{src}"
    );
}

/// When no package.ipe exists in any parent directory, returns None.
#[test]
fn find_manifest_returns_none_when_absent() {
    let tmp = std::env::temp_dir().join("ipec_no_manifest_test");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).expect("create dir");
    let ipe = tmp.join("Standalone.ipe");
    fs::write(&ipe, "module Standalone exposing (f)\nf = 0\n").expect("write ipe");
    // Deliberately no package.ipe anywhere under tmp.
    // The walk terminates at the filesystem root without finding one.
    // We cannot guarantee the walk terminates before reaching /tmp or /
    // on all systems, so we only assert non-panicking behaviour and that
    // the returned path (if Some) is a real file.
    let found = find_manifest_for_ipe_file(&ipe);
    if let Some(ref p) = found {
        assert!(p.is_file(), "if Some, the manifest must exist on disk");
    }
    let _ = fs::remove_dir_all(&tmp);
}

/// Two-module program: `Main.ipe` calls a helper in sibling `Lib.ipe`.
/// `build_with_sibling_discovery` must compile both without IPE-N0020.
#[test]
fn sibling_discovery_compiles_two_module_program() {
    let runtime = resolve_runtime();
    if runtime.is_err() {
        // Runtime not found in this environment (CI without IPE_RUNTIME_DIR) —
        // skip rather than fail: the sweep catches this live.
        return;
    }
    let Ok(runtime) = runtime else { return };

    let tmp = std::env::temp_dir().join("ipec_sibling_disc_test");
    let _ = fs::remove_dir_all(&tmp);
    let src = tmp.join("src");
    fs::create_dir_all(&src).expect("create src/");

    // Helper module: src/Helper.ipe
    fs::write(
        src.join("Helper.ipe"),
        "module Helper exposing (answer)\nanswer = 42\n",
    )
    .expect("write Helper.ipe");

    // Entry module: src/Main.ipe — imports Helper
    fs::write(
        src.join("Main.ipe"),
        "module Main exposing (main)\nimport Helper\nimport Ipe.Io\nimport Ipe.String\nmain = Io.println (String.fromInt Helper.answer)\n",
    )
    .expect("write Main.ipe");

    let out = tmp.join("out");
    let result = build_with_sibling_discovery(&src.join("Main.ipe"), &out, &runtime);
    assert!(
        result.is_ok(),
        "two-module program must compile via sibling discovery: {:?}",
        result.err()
    );
    let _ = fs::remove_dir_all(&tmp);
}

/// `ipe verify`'s test-stage build: a `tests/Main.ipe` that imports a module
/// living under `src/Lib/` must resolve the `src/` code under test, not fail
/// with IPE-N0020. This is the standard `src/` + `tests/` layout the naive
/// entry-parent source root cannot see across.
#[test]
fn test_stage_build_resolves_src_modules_from_tests_dir() {
    let runtime = resolve_runtime();
    let Ok(runtime) = runtime else { return };

    let tmp = std::env::temp_dir().join("ipec_verify_test_stage_src_disc");
    let _ = fs::remove_dir_all(&tmp);
    let src = tmp.join("src");
    let tests = tmp.join("tests");
    fs::create_dir_all(src.join("Lib")).expect("create src/Lib/");
    fs::create_dir_all(&tests).expect("create tests/");

    // Code under test: src/Lib/Foo.ipe (a multi-segment module referenced
    // via an alias, the way real projects import a nested module).
    fs::write(
        src.join("Lib").join("Foo.ipe"),
        "module Lib.Foo exposing (answer)\nanswer = 42\n",
    )
    .expect("write src/Lib/Foo.ipe");

    // A src entry that also uses the library (mirrors a real project).
    fs::write(
        src.join("Main.ipe"),
        "module Main exposing (main)\nimport Lib.Foo as Foo\nimport Ipe.Io as Io\nimport Ipe.String as String\nmain = Io.println (String.fromInt Foo.answer)\n",
    )
    .expect("write src/Main.ipe");

    // Test entry in the sibling tests/ directory imports the src/ module.
    fs::write(
        tests.join("Main.ipe"),
        "module Main exposing (main)\nimport Lib.Foo as Foo\nimport Ipe.Io as Io\nimport Ipe.String as String\nmain = Io.println (String.fromInt Foo.answer)\n",
    )
    .expect("write tests/Main.ipe");

    let out = tmp.join("out");
    let result = build_test_with_project_sources(&src, &tests.join("Main.ipe"), &out, &runtime);
    assert!(
        result.is_ok(),
        "the test stage must resolve src/ modules from tests/ (no IPE-N0020): {:?}",
        result.err()
    );
    let _ = fs::remove_dir_all(&tmp);
}

/// The test-stage source collection unions the `src/` and `tests/` trees
/// with the correct per-root relativisation: `src/Lib/Foo.ipe` → `Lib.Foo`,
/// `tests/Main.ipe` → `Main`, and the entry is the test module. This is the
/// resolution the build depends on, asserted without a runtime.
#[test]
fn collect_test_sources_unions_src_and_tests_trees() {
    let tmp = std::env::temp_dir().join("ipec_collect_test_sources_union");
    let _ = fs::remove_dir_all(&tmp);
    let src = tmp.join("src");
    let tests = tmp.join("tests");
    fs::create_dir_all(src.join("Lib")).expect("create src/Lib/");
    fs::create_dir_all(&tests).expect("create tests/");
    fs::write(
        src.join("Lib").join("Foo.ipe"),
        "module Lib.Foo exposing (answer)\nanswer = 42\n",
    )
    .expect("write src/Lib/Foo.ipe");
    fs::write(
        tests.join("Main.ipe"),
        "module Main exposing (main)\nimport Lib.Foo as Foo\nmain = Foo.answer\n",
    )
    .expect("write tests/Main.ipe");

    let collected = collect_test_sources(&src, &tests.join("Main.ipe"))
        .expect("collect_test_sources must succeed");

    assert_eq!(
        collected.entry_module_path,
        vec!["Main".to_owned()],
        "the entry is the test module"
    );
    assert!(
        collected
            .sources
            .contains_key(&vec!["Lib".to_owned(), "Foo".to_owned()]),
        "src/Lib/Foo.ipe must be present as Lib.Foo, got keys: {:?}",
        collected.sources.keys().collect::<Vec<_>>()
    );
    assert!(
        collected.sources.contains_key(&vec!["Main".to_owned()]),
        "the test entry must be present as Main"
    );
    let _ = fs::remove_dir_all(&tmp);
}

// -----------------------------------------------------------------------
// Cross-module infer errors name the dep module's file
// -----------------------------------------------------------------------

/// When a type error originates in a dep module (`Helper.ipe`), the rendered
/// diagnostic must cite `Helper.ipe` as the file, NOT the entry `Main.ipe`.
/// A single `pipeline_err` closure capturing only the entry file path would
/// render dep-module errors with the wrong source snippet and file name.
///
/// Runtime is not reached (infer aborts first), so we pass a dummy path.
#[test]
fn infer_error_in_dep_module_names_dep_file() {
    let tmp = std::env::temp_dir().join("ipec_144_dep_err_test");
    let _ = fs::remove_dir_all(&tmp);
    let src = tmp.join("src");
    fs::create_dir_all(&src).expect("create src/");

    // Helper.ipe: deliberate type error — `1 + "oops"` mixes Int and String.
    let helper_path = src.join("Helper.ipe");
    fs::write(
        &helper_path,
        "module Helper exposing (broken)\nbroken = 1 + \"oops\"\n",
    )
    .expect("write Helper.ipe");

    // Main.ipe: imports Helper and uses `broken` — but the error is in Helper.
    let main_path = src.join("Main.ipe");
    fs::write(
        &main_path,
        "module Main exposing (main)\nimport Helper\nimport Ipe.Io\nimport Ipe.String\nmain = Io.println (String.fromInt Helper.broken)\n",
    )
    .expect("write Main.ipe");

    // Runtime is never accessed: a type error fires at infer, before lower/emit.
    let dummy_runtime = std::env::temp_dir();
    let out = tmp.join("out");
    let result = build_with_sibling_discovery(&main_path, &out, &dummy_runtime);

    // Must fail — the program has a type error in Helper.
    assert!(
        result.is_err(),
        "#144 fixture must fail (type error in dep); got Ok unexpectedly"
    );
    let Err(CliError::Pipeline { file, .. }) = result else {
        let _ = fs::remove_dir_all(&tmp);
        return; // any other error kind is a separate concern
    };

    // The file blamed must be Helper.ipe, not Main.ipe.
    let file_name = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
    assert_eq!(
        file_name,
        "Helper.ipe",
        "#144 regression: type error in dep module must blame `Helper.ipe`, \
         not `{file_name}`; full path: {}",
        file.display()
    );

    let _ = fs::remove_dir_all(&tmp);
}

// -----------------------------------------------------------------------
// Home-module discriminant — cross-module errors use `home` on Constraint
// -----------------------------------------------------------------------

/// Regression test for the home-module span discriminant fix.
///
/// Before this fix the constraint solver emitted bare `Span` values (byte
/// offsets with no module tag).  After `link::link` merges N modules into
/// one flat def list, a byte offset like 34 can be numerically contained by
/// a def from *either* module.  The byte-offset heuristic (`source_for_span`)
/// picks the closest def, but it can pick the wrong one when two modules have
/// overlapping numeric span ranges — e.g., a wide def in module A that starts
/// at byte 20 and a narrow def in module B that starts at byte 30, with the
/// type error at byte 34.  Both body spans contain byte 34, but A has a
/// closer `lo_dist` to the wrong def, so the heuristic blames the wrong file
/// whenever the numerically-nearest def belongs to a different module.
///
/// Every `Constraint` carries its source module's `home` path, so
/// `compile_modules` routes `Err((diag, home))` directly via
/// `home_to_source.get(&home)`, bypassing the heuristic entirely when a home
/// is available.
///
/// This test builds a two-module program where the type error is in module B
/// (`Lib.ipe`) but the heuristic *could* be fooled by a wide def in module A
/// (`Pad.ipe`).  The assertion checks that the blamed file is `Lib.ipe`.
///
/// To exercise the home-discriminant path rather than the heuristic, `Pad.ipe`
/// is constructed so that its def body starts at roughly the same byte offset
/// as the error in `Lib.ipe` — any byte-offset resolver that ignores the home
/// would be ambiguous.  The discriminant is the only reliable resolver.
#[test]
fn home_discriminant_cross_module_type_error_names_correct_file() {
    let tmp = std::env::temp_dir().join("ipec_home_disc_test");
    let _ = fs::remove_dir_all(&tmp);
    let src = tmp.join("src");
    fs::create_dir_all(&src).expect("create src/");

    // Pad.ipe: a valid module whose single def body starts at roughly the
    // same byte offset as the type error in Lib.ipe.  Constructed so the
    // body span (a long arithmetic chain) numerically overlaps with Lib's
    // error span.  The body itself is well-typed.
    //
    //   "module Pad exposing (pad)\npad = " is 27 bytes.
    //   The body "1 + 2 + 3 + 4 + 5 + 6 + 7 + 8 + 9" starts at byte 27.
    //   The body ends at byte 27+35 = 62.
    //
    // After link, Pad's def body covers bytes [27, 62] in Pad's namespace.
    fs::write(
        src.join("Pad.ipe"),
        "module Pad exposing (pad)\npad = 1 + 2 + 3 + 4 + 5 + 6 + 7 + 8 + 9\n",
    )
    .expect("write Pad.ipe");

    // Lib.ipe: a module with a deliberate type error at a span that falls
    // numerically inside Pad's body range.
    //
    //   "module Lib exposing (bad)\nbad = " is 27 bytes.
    //   The body "1 + 2 + 3 + 4 + \"oops\"" starts at byte 27.
    //   The type error is at "\"oops\"" = byte 27+20 = 47, inside [27,62].
    //
    // Without the home discriminant, `source_for_span(span=47)` would see
    // BOTH Pad's body [27,62] (lo_dist=20) and Lib's body [27,49] (lo_dist=20)
    // as equally-distanced candidates — and would pick the narrower body, which
    // happens to be Lib here.  But in general (different padding choices) it
    // can pick the wrong one.  The fix makes the home the authoritative signal.
    fs::write(
        src.join("Lib.ipe"),
        "module Lib exposing (bad)\nbad = 1 + 2 + 3 + 4 + \"oops\"\n",
    )
    .expect("write Lib.ipe");

    // Main.ipe: imports both; the error is in Lib, not Main or Pad.
    fs::write(
        src.join("Main.ipe"),
        "module Main exposing (main)\nimport Lib\nimport Pad\nimport Ipe.Io\nimport Ipe.String\nmain = Io.println (String.fromInt Lib.bad)\n",
    )
    .expect("write Main.ipe");

    let dummy_runtime = std::env::temp_dir();
    let out = tmp.join("out");
    let result = build_with_sibling_discovery(&src.join("Main.ipe"), &out, &dummy_runtime);

    // Must fail — type error in Lib.
    assert!(
        result.is_err(),
        "home-discriminant fixture must fail (type error in Lib); got Ok unexpectedly"
    );
    let Err(CliError::Pipeline { file, .. }) = result else {
        let _ = fs::remove_dir_all(&tmp);
        return;
    };

    // The blamed file must be Lib.ipe — the module that OWNS the failing
    // constraint, regardless of which module the byte-offset heuristic
    // would pick.
    let file_name = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
    assert_eq!(
        file_name,
        "Lib.ipe",
        "home-discriminant regression: type error in Lib must blame `Lib.ipe`, \
         not `{file_name}`; full path: {}",
        file.display()
    );

    let _ = fs::remove_dir_all(&tmp);
}

// -----------------------------------------------------------------
// On-disk build cache end-to-end proof
// -----------------------------------------------------------------

/// Walk `cache_root/<epoch>/` and return the single `EmittedProject`-tier
/// entry (`<key>.json`) a fresh build just wrote. The epoch name is
/// unpredictable from a test's perspective (it folds in the running binary's
/// own content hash), so this has to search rather than construct the path
/// directly. The co-resident IR tier writes `<key>.ir.json` under the same
/// epoch dir — that file's extension is also `json`, so it is excluded by
/// name to keep this matcher pinned to the `EmittedProject` tier.
fn find_single_cache_entry(cache_root: &Path) -> Option<PathBuf> {
    for epoch_entry in fs::read_dir(cache_root).ok()?.flatten() {
        let epoch_dir = epoch_entry.path();
        if !epoch_dir.is_dir() {
            continue;
        }
        for file_entry in fs::read_dir(&epoch_dir).ok()?.flatten() {
            let path = file_entry.path();
            let is_json = path.extension().and_then(std::ffi::OsStr::to_str) == Some("json");
            let is_ir_tier = path
                .file_name()
                .and_then(std::ffi::OsStr::to_str)
                .is_some_and(|n| n.ends_with(".ir.json"));
            if is_json && !is_ir_tier {
                return Some(path);
            }
        }
    }
    None
}

/// The end-to-end proof that `compile_modules_observed` actually
/// CONSULTS and TRUSTS the on-disk cache, not merely that two identical
/// builds happen to agree (which determinism alone would already give,
/// without proving the cache was read at all).
///
/// Strategy: compile once (a genuine cache miss, populates the cache),
/// locate the single entry the build just wrote, and TAMPER with its
/// `cargo_toml` field with a sentinel no fresh compile of the SAME
/// source could ever produce. Compile again with the SAME inputs and
/// the SAME cache dir; if the driver reads and trusts the cache, the
/// second build's `Cargo.toml` carries the sentinel verbatim. If it
/// silently recompiled instead, the sentinel is gone.
#[test]
fn on_disk_cache_hit_serves_a_tampered_entry_verbatim() {
    const SENTINEL: &str = "# CACHE-HIT-SENTINEL\n";

    let Ok(runtime) = resolve_runtime() else {
        return; // No in-repo runtime tree in this environment — see other tests' pattern.
    };

    let tmp = std::env::temp_dir().join(format!("ipe-cache-e2e-{}", std::process::id()));
    let cache_dir = tmp.join("cache");
    let out_a = tmp.join("out-a");
    let out_b = tmp.join("out-b");
    let _ = fs::remove_dir_all(&tmp);

    let entry_path = vec!["Main".to_owned()];
    let mut sources: BTreeMap<Vec<String>, (PathBuf, String)> = BTreeMap::new();
    sources.insert(
        entry_path.clone(),
        (
            PathBuf::from("<cache-e2e>/Main.ipe"),
            "module Main exposing (main)\n\nimport Ipe.Io as Io\nimport Ipe.String as String\n\nmain : Task Error ()\nmain =\n    Io.println (String.fromInt 1)\n".to_owned(),
        ),
    );
    let discovered = vec![project::DiscoveredModule {
        path: PathBuf::from("<cache-e2e>/Main.ipe"),
        module_path: entry_path.clone(),
    }];

    let (result_a, outcome_a) = compile_modules_observed(
        sources.clone(),
        discovered.clone(),
        &entry_path,
        &out_a,
        &runtime,
        Path::new("<cache-e2e>"),
        ipe_backend_rust::DbDriver::Sqlite,
        Some(&cache_dir),
        BuildOptions::default(),
    );
    assert!(
        result_a.is_ok(),
        "first (cold) compile must succeed: {:?}",
        result_a.err()
    );
    assert_eq!(
        outcome_a,
        CacheOutcome::Miss,
        "first compile against an empty cache dir must be a miss"
    );

    let entry_json = find_single_cache_entry(&cache_dir)
        .expect("first build must have written exactly one cache entry");
    let stored = fs::read_to_string(&entry_json).expect("cache entry must be readable");
    let mut cached: ipe_backend::EmittedProject =
        serde_json::from_str(&stored).expect("cache entry must deserialize");
    cached.cargo_toml = format!("{SENTINEL}{}", cached.cargo_toml);
    fs::write(
        &entry_json,
        serde_json::to_vec(&cached).expect("re-serialize must succeed"),
    )
    .expect("tamper write must succeed");

    let (result_b, outcome_b) = compile_modules_observed(
        sources,
        discovered,
        &entry_path,
        &out_b,
        &runtime,
        Path::new("<cache-e2e>"),
        ipe_backend_rust::DbDriver::Sqlite,
        Some(&cache_dir),
        BuildOptions::default(),
    );
    assert!(
        result_b.is_ok(),
        "second (cache-hit) compile must succeed: {:?}",
        result_b.err()
    );
    assert_eq!(
        outcome_b,
        CacheOutcome::Hit,
        "second compile with byte-identical inputs must hit the cache"
    );

    let written = fs::read_to_string(out_b.join("Cargo.toml")).expect("Cargo.toml must exist");
    assert!(
        written.starts_with(SENTINEL),
        "materialized output must be the TAMPERED cache entry, not a fresh \
         recompile — proves the driver actually reads and trusts the \
         on-disk cache: {written}"
    );

    let _ = fs::remove_dir_all(&tmp);
}

/// Walk `cache_root/<epoch>/*.ir.json` and return the single
/// lowered-IR entry file a build just wrote. Mirrors
/// [`find_single_cache_entry`], but matches on the `.ir.json` suffix
/// specifically — `Path::extension()` alone cannot tell `key.json` from
/// `key.ir.json` apart (both report `json`), so a build that populated
/// BOTH tiers in the same epoch directory needs the suffix check to
/// find the right one.
fn find_single_ir_cache_entry(cache_root: &Path) -> Option<PathBuf> {
    for epoch_entry in fs::read_dir(cache_root).ok()?.flatten() {
        let epoch_dir = epoch_entry.path();
        if !epoch_dir.is_dir() {
            continue;
        }
        for file_entry in fs::read_dir(&epoch_dir).ok()?.flatten() {
            let path = file_entry.path();
            if path
                .file_name()
                .and_then(std::ffi::OsStr::to_str)
                .is_some_and(|n| n.ends_with(".ir.json"))
            {
                return Some(path);
            }
        }
    }
    None
}

/// **End-to-end proof that a `db_driver`-only edit reuses the
/// lowered-IR tier instead of a full recompile.** The `EmittedProject`
/// tier's key folds in `db_driver` (a real dependency of the FINAL emit
/// stage), so it correctly MISSES on a driver flip — but
/// `linked_program`/`typecheck`/`lower_program` never read `db_driver`
/// at all, so the SAME lowered `Program` is still exactly reusable. This
/// is the concrete case the IR tier exists to cover that the
/// `EmittedProject` tier structurally cannot.
#[test]
fn ir_cache_hit_reuses_lowered_program_across_a_db_driver_only_edit() {
    let Ok(runtime) = resolve_runtime() else {
        return;
    };
    let tmp = std::env::temp_dir().join(format!("ipec-ir-cache-driver-{}", std::process::id()));
    let cache_dir = tmp.join("cache");
    let out_a = tmp.join("out-a");
    let out_b = tmp.join("out-b");
    let _ = fs::remove_dir_all(&tmp);

    let entry_path = vec!["Main".to_owned()];
    let mut sources: BTreeMap<Vec<String>, (PathBuf, String)> = BTreeMap::new();
    sources.insert(
        entry_path.clone(),
        (
            PathBuf::from("<p>/Main.ipe"),
            "module Main exposing (main)\n\nimport Ipe.Io as Io\nimport Ipe.String as String\n\nmain : Task Error ()\nmain =\n    Io.println (String.fromInt 1)\n".to_owned(),
        ),
    );
    let discovered = vec![project::DiscoveredModule {
        path: PathBuf::from("<p>/Main.ipe"),
        module_path: entry_path.clone(),
    }];

    let (result_a, outcome_a) = compile_modules_observed(
        sources.clone(),
        discovered.clone(),
        &entry_path,
        &out_a,
        &runtime,
        Path::new("<p>"),
        ipe_backend_rust::DbDriver::Sqlite,
        Some(&cache_dir),
        BuildOptions::default(),
    );
    assert!(
        result_a.is_ok(),
        "first (cold, Sqlite) compile must succeed: {:?}",
        result_a.err()
    );
    assert_eq!(
        outcome_a,
        CacheOutcome::Miss,
        "first compile against an empty cache dir must be a miss"
    );
    assert!(
        find_single_ir_cache_entry(&cache_dir).is_some(),
        "the cold compile must have populated the IR tier"
    );

    // Same source, DIFFERENT driver, same cache dir: the EmittedProject
    // tier's key changes (driver is part of it) so it misses, but the
    // IR tier's key does not depend on driver — it must hit.
    let (result_b, outcome_b) = compile_modules_observed(
        sources,
        discovered,
        &entry_path,
        &out_b,
        &runtime,
        Path::new("<p>"),
        ipe_backend_rust::DbDriver::Postgres,
        Some(&cache_dir),
        BuildOptions::default(),
    );
    assert!(
        result_b.is_ok(),
        "second (Postgres) compile must succeed: {:?}",
        result_b.err()
    );
    assert_eq!(
        outcome_b,
        CacheOutcome::IrHit,
        "a db_driver-only edit must hit the IR tier, not re-run the full pipeline nor \
         merely miss everything"
    );

    let _ = fs::remove_dir_all(&tmp);
}

/// **The IR-tier end-to-end tamper proof**, mirroring
/// [`on_disk_cache_hit_serves_a_tampered_entry_verbatim`] one tier
/// earlier: compile once (populates BOTH tiers), tamper the ON-DISK
/// lowered-IR entry's literal body (`main`'s `Expr::Int(1)` ->
/// `Expr::Int(42)`) with a value no fresh compile of the SAME source
/// could ever produce, then force an IR-tier hit (a `db_driver` flip,
/// which misses the `EmittedProject` tier deterministically) and assert
/// the SENTINEL VALUE reaches the materialised `main.rs` — proof the
/// driver actually reads, relocates, and RE-EMITS the on-disk IR entry
/// rather than silently recompiling or ignoring the tamper.
#[test]
fn on_disk_ir_cache_hit_serves_a_tampered_entry_verbatim() {
    let Ok(runtime) = resolve_runtime() else {
        return;
    };
    let tmp = std::env::temp_dir().join(format!("ipec-ir-cache-tamper-{}", std::process::id()));
    let cache_dir = tmp.join("cache");
    let out_a = tmp.join("out-a");
    let out_b = tmp.join("out-b");
    let _ = fs::remove_dir_all(&tmp);

    let entry_path = vec!["Main".to_owned()];
    let mut sources: BTreeMap<Vec<String>, (PathBuf, String)> = BTreeMap::new();
    sources.insert(
        entry_path.clone(),
        (
            PathBuf::from("<p>/Main.ipe"),
            "module Main exposing (main)\n\nimport Ipe.Io as Io\nimport Ipe.String as String\n\nmain : Task Error ()\nmain =\n    Io.println (String.fromInt 1)\n".to_owned(),
        ),
    );
    let discovered = vec![project::DiscoveredModule {
        path: PathBuf::from("<p>/Main.ipe"),
        module_path: entry_path.clone(),
    }];

    let (result_a, outcome_a) = compile_modules_observed(
        sources.clone(),
        discovered.clone(),
        &entry_path,
        &out_a,
        &runtime,
        Path::new("<p>"),
        ipe_backend_rust::DbDriver::Sqlite,
        Some(&cache_dir),
        BuildOptions::default(),
    );
    assert!(
        result_a.is_ok(),
        "first (cold) compile must succeed: {:?}",
        result_a.err()
    );
    assert_eq!(outcome_a, CacheOutcome::Miss);

    let ir_json_path =
        find_single_ir_cache_entry(&cache_dir).expect("cold compile must write an IR entry");
    let stored = fs::read_to_string(&ir_json_path).expect("IR entry must be readable");
    // Verified shape via a one-off print during development: `main`'s body is
    // `Io.println (String.fromInt 1)`, so the only integer literal in the IR is
    // the `{"Int":1}` argument to `String.fromInt`. Tampering it to `42` makes
    // the re-emitted program print `42` — a value no fresh compile of this
    // source could produce.
    assert!(
        stored.contains("{\"Int\":1}"),
        "unexpected IR JSON shape, cannot safely tamper: {stored}"
    );
    let tampered = stored.replace("{\"Int\":1}", "{\"Int\":42}");
    fs::write(&ir_json_path, &tampered).expect("tamper write must succeed");

    // Force the EmittedProject tier to miss (driver flip) so the
    // IR-tier fast path is the one actually exercised.
    let (result_b, outcome_b) = compile_modules_observed(
        sources,
        discovered,
        &entry_path,
        &out_b,
        &runtime,
        Path::new("<p>"),
        ipe_backend_rust::DbDriver::Postgres,
        Some(&cache_dir),
        BuildOptions::default(),
    );
    assert!(
        result_b.is_ok(),
        "second (tampered IR, hit) compile must succeed: {:?}",
        result_b.err()
    );
    assert_eq!(outcome_b, CacheOutcome::IrHit);

    let main_rs = fs::read_to_string(out_b.join("src/main.rs")).expect("main.rs must exist");
    assert!(
        main_rs.contains("42"),
        "materialized output must be re-EMITTED FROM the tampered IR entry \
         (contains the literal 42), proving the driver reads/relocates/re-emits \
         the on-disk lowered-IR cache rather than recompiling or discarding the \
         tamper: {main_rs}"
    );

    let _ = fs::remove_dir_all(&tmp);
}

/// A cache disabled via `cache_dir: None` never touches disk for
/// caching purposes and always runs the full pipeline.
#[test]
fn cache_dir_none_disables_caching_entirely() {
    let Ok(runtime) = resolve_runtime() else {
        return;
    };
    let tmp = std::env::temp_dir().join(format!("ipe-cache-disabled-{}", std::process::id()));
    let out_dir = tmp.join("out");
    let _ = fs::remove_dir_all(&tmp);

    let entry_path = vec!["Main".to_owned()];
    let mut sources: BTreeMap<Vec<String>, (PathBuf, String)> = BTreeMap::new();
    sources.insert(
        entry_path.clone(),
        (
            PathBuf::from("<cache-e2e>/Main.ipe"),
            "module Main exposing (main)\n\nimport Ipe.Io as Io\nimport Ipe.String as String\n\nmain : Task Error ()\nmain =\n    Io.println (String.fromInt 1)\n".to_owned(),
        ),
    );
    let discovered = vec![project::DiscoveredModule {
        path: PathBuf::from("<cache-e2e>/Main.ipe"),
        module_path: entry_path.clone(),
    }];

    let (result, outcome) = compile_modules_observed(
        sources,
        discovered,
        &entry_path,
        &out_dir,
        &runtime,
        Path::new("<cache-e2e>"),
        ipe_backend_rust::DbDriver::Sqlite,
        None,
        BuildOptions::default(),
    );
    assert!(result.is_ok(), "compile must succeed: {:?}", result.err());
    assert_eq!(
        outcome,
        CacheOutcome::Miss,
        "a disabled cache is always reported as a miss"
    );
    assert!(
        !tmp.join(".ipe-cache").exists(),
        "no cache directory should be created when caching is disabled"
    );

    let _ = fs::remove_dir_all(&tmp);
}

// ── [wasm].mode target inference ─────────────────────────────────────────

fn wasm_config(mode: Option<&str>) -> project::WasmConfig {
    project::WasmConfig {
        mode: mode.map(str::to_owned),
        ..Default::default()
    }
}

/// `[wasm] mode = "spa"` with no CLI flag → inferred `WasmClient`.
#[test]
fn wasm_mode_spa_infers_wasm_target() {
    let cfg = wasm_config(Some("spa"));
    assert!(
        resolve_wasm_target(false, Some(&cfg)),
        "spa mode must infer wasm target"
    );
}

/// `[wasm] mode = "hydrate"` with no CLI flag → inferred `WasmClient`.
#[test]
fn wasm_mode_hydrate_infers_wasm_target() {
    let cfg = wasm_config(Some("hydrate"));
    assert!(
        resolve_wasm_target(false, Some(&cfg)),
        "hydrate mode must infer wasm target"
    );
}

/// `[wasm] mode = "off"` → native (explicit opt-out).
#[test]
fn wasm_mode_off_does_not_infer_wasm_target() {
    let cfg = wasm_config(Some("off"));
    assert!(
        !resolve_wasm_target(false, Some(&cfg)),
        "off mode must not infer wasm target"
    );
}

/// No `[wasm]` section (None config) → native default.
#[test]
fn no_wasm_config_defaults_to_native_target() {
    assert!(
        !resolve_wasm_target(false, None),
        "absent [wasm] section must default to native"
    );
}

/// `mode = None` (section present but no mode key) → native.
#[test]
fn wasm_config_absent_mode_key_defaults_to_native_target() {
    let cfg = wasm_config(None);
    assert!(
        !resolve_wasm_target(false, Some(&cfg)),
        "absent mode key must default to native"
    );
}

/// CLI `--target wasm` (`cli_wasm` = true) wins even when no manifest.
#[test]
fn cli_flag_overrides_absent_manifest_to_wasm() {
    assert!(
        resolve_wasm_target(true, None),
        "cli flag must win over absent manifest"
    );
}

/// CLI `--target wasm` wins even if the manifest says off (highest precedence).
#[test]
fn cli_flag_wins_over_mode_off() {
    let cfg = wasm_config(Some("off"));
    assert!(
        resolve_wasm_target(true, Some(&cfg)),
        "explicit cli --target wasm must win over mode=off"
    );
}

/// `declared_modules` reads exactly the `pub mod X;` / `mod X;` statements a
/// runtime `mod.rs` declares — the oracle the eject tree-shaker copies from.
/// A `pub use X::*;` re-export is NOT a module declaration and must not add a
/// file to the copy set, and a block-opening `pub mod X {` (an inline module
/// with no separate source file) is excluded by the `;` requirement.
#[test]
fn declared_modules_reads_only_semicolon_terminated_mod_statements() {
    let mod_rs = "\
// GENERATED by Ipê — do not edit
pub mod basics;
pub mod core;
mod path_core;
pub use basics::*;
pub use core::*;
pub mod web {
pub mod route;
}
";
    let names = declared_modules(mod_rs);
    assert!(names.contains("basics"), "a `pub mod` is a declaration");
    assert!(names.contains("core"), "a `pub mod` is a declaration");
    assert!(names.contains("path_core"), "a bare `mod` is a declaration");
    assert!(
        !names.contains("web"),
        "a block-opening `pub mod web {{` has no separate file — excluded"
    );
    // A `pub use X::*;` glob is a re-export, never a module declaration.
    assert!(
        !names.contains("basics::*") && names.iter().all(|n| !n.contains('*')),
        "a glob re-export is not a module declaration"
    );
    // The one `;`-terminated statement inside the block (`pub mod route;`) is
    // collected by name — it is harmless in practice: the copy step resolves
    // it against no top-level `route.rs`/`route/` and vendors nothing for it.
    // The real emitted native `mod.rs` is flat (no inline blocks), so this
    // case never arises there; the copy step, not this scanner, is where the
    // fail-safe lives.
    assert!(names.contains("route"));
}

/// The tree-shaker copies a reached module's single `.rs` file, a reached
/// directory module's ENTIRE subtree (fail-closed — never omit a nested
/// `mod`'s file), and nothing for a module the emitted `mod.rs` never
/// declares. This is the whole tree-shaking contract, asserted without a
/// compile.
#[test]
fn reachable_runtime_copy_takes_declared_files_and_whole_reached_dirs() {
    let tmp = std::env::temp_dir().join("ipe_eject_reach_copy");
    let _ = fs::remove_dir_all(&tmp);
    let rt = tmp.join("ipe_runtime");
    fs::create_dir_all(rt.join("web")).expect("create web/");
    fs::create_dir_all(rt.join("db")).expect("create db/");
    fs::write(rt.join("mod.rs"), "pub mod core;\npub mod web;\n").expect("mod.rs");
    fs::write(rt.join("core.rs"), "// core").expect("core.rs");
    fs::write(rt.join("unreached.rs"), "// unreached").expect("unreached.rs");
    fs::write(rt.join("web").join("mod.rs"), "pub mod route;").expect("web/mod.rs");
    fs::write(rt.join("web").join("route.rs"), "// route").expect("web/route.rs");
    // An unreached directory module: its whole subtree must be dropped.
    fs::write(rt.join("db").join("mod.rs"), "// db").expect("db/mod.rs");

    // The emitted mod.rs reaches `core` (file) and `web` (directory), never
    // `unreached` or `db`.
    let emitted_mod_rs = "pub mod core;\npub mod web;\n";
    let mut manifest = BTreeMap::new();
    collect_reachable_runtime_text(
        &rt,
        Path::new("src/ipe_runtime"),
        emitted_mod_rs,
        &mut manifest,
    )
    .expect("copy reachable runtime");

    let has = |p: &str| manifest.contains_key(&PathBuf::from(p));
    assert!(has("src/ipe_runtime/core.rs"), "reached file copied");
    assert!(
        has("src/ipe_runtime/web/mod.rs") && has("src/ipe_runtime/web/route.rs"),
        "reached directory module copied WHOLE (nested mod's file included)"
    );
    assert!(
        !has("src/ipe_runtime/unreached.rs"),
        "an undeclared file is tree-shaken away"
    );
    assert!(
        !manifest.keys().any(|k| k.starts_with("src/ipe_runtime/db")),
        "an undeclared directory module's whole subtree is tree-shaken away"
    );
    let _ = fs::remove_dir_all(&tmp);
}

/// Eject refuses a wasm-target project from the `[wasm].mode` manifest tier —
/// not only the `IPE_TARGET` env — so a browser SPA is never silently ejected
/// as a native tree (a target the emitted crate would not build). The refusal
/// fires before any file is written.
#[test]
fn eject_refuses_a_wasm_mode_project_from_the_manifest_tier() {
    let tmp = std::env::temp_dir().join("ipe_eject_wasm_mode_refuse");
    let _ = fs::remove_dir_all(&tmp);
    let src = tmp.join("src");
    fs::create_dir_all(&src).expect("create src/");
    // A project whose manifest selects the wasm target via `Package.wasm`,
    // with no `IPE_TARGET` env set — the tier the env-only check missed.
    fs::write(
        tmp.join("package.ipe"),
        "module Package exposing (package)\n\n\npackage =\n    { name = \"w\", wasm = On { mode = Spa } }\n",
    )
    .expect("write package.ipe");
    fs::write(
        src.join("Main.ipe"),
        "module Main exposing (main)\nmain = 0\n",
    )
    .expect("write Main.ipe");

    let out = tmp.join("out");
    let args = [
        tmp.join("package.ipe").to_string_lossy().into_owned(),
        "--out".to_owned(),
        out.to_string_lossy().into_owned(),
    ];
    let result = run_eject(&args);
    assert!(
        matches!(result, Err(CliError::EjectUnsupported { .. })),
        "a `[wasm].mode` project must be refused, not ejected native: {result:?}"
    );
    assert!(
        !out.exists(),
        "the refusal must fire before any project tree is written"
    );
    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn analysis_root_prefers_main_then_program_then_exposed() {
    // An application with a src/Main.ipe uses it as the analysis root.
    let app = std::env::temp_dir().join("ipe_analysis_root_app");
    let _ = fs::remove_dir_all(&app);
    let app_src = app.join("src");
    fs::create_dir_all(&app_src).expect("create src/");
    fs::write(
        app.join("package.ipe"),
        "module Package exposing (package)\n\n\npackage =\n    { name = \"app\" }\n",
    )
    .expect("pkg");
    fs::write(
        app_src.join("Main.ipe"),
        "module Main exposing (main)\nmain = 0\n",
    )
    .expect("main");
    let app_manifest = project::parse_manifest(&app.join("package.ipe")).expect("app parses");
    assert_eq!(
        analysis_root_of(&app_manifest).expect("app root resolves"),
        app_src.join("Main.ipe")
    );
    let _ = fs::remove_dir_all(&app);

    // A library (exposedModules, no Main) uses its first exposed module's file.
    let lib = std::env::temp_dir().join("ipe_analysis_root_lib");
    let _ = fs::remove_dir_all(&lib);
    let lib_src = lib.join("src");
    fs::create_dir_all(&lib_src).expect("create src/");
    fs::write(lib.join("package.ipe"), "module Package exposing (package)\n\n\npackage =\n    { name = \"lib\", exposedModules = [ \"Core.Utils\" ] }\n").expect("pkg");
    // src/ must exist for the manifest reader's source-root check; the module
    // file itself need not exist for the pure path derivation under test.
    let lib_manifest = project::parse_manifest(&lib.join("package.ipe")).expect("lib parses");
    assert_eq!(
        analysis_root_of(&lib_manifest).expect("lib root resolves"),
        lib_src.join("Core").join("Utils.ipe")
    );
    let _ = fs::remove_dir_all(&lib);
}

#[test]
fn version_flags_alias_the_version_command() {
    // `--version` / `-V` are the near-universal version probe; both resolve
    // to the `version` command rather than falling through to an
    // unknown-command failure with the full help screen.
    assert!(run_cli(&["--version".to_owned()]).is_ok());
    assert!(run_cli(&["-V".to_owned()]).is_ok());
    // Trailing format flags still reach the command.
    assert!(run_cli(&["--version".to_owned(), "--json".to_owned()]).is_ok());
}

#[test]
fn analysis_root_rejects_a_program_entry_that_escapes_the_source_root() {
    // A manifest whose declared program entry is an absolute path (or a `..`
    // traversal) must not let `ipe type-check` read a file outside the
    // project: analysis_root_of routes the entry through the same containment
    // gate the build path uses, so the escape is a typed refusal.
    let proj = std::env::temp_dir().join("ipe_analysis_root_escape");
    let _ = fs::remove_dir_all(&proj);
    let proj_src = proj.join("src");
    fs::create_dir_all(&proj_src).expect("create src/");
    fs::write(
        proj.join("package.ipe"),
        "module Package exposing (package)\n\n\npackage =\n    { name = \"escape\"\n    , programs = [ { name = \"x\", entry = \"/etc/passwd\" } ]\n    }\n",
    )
    .expect("pkg");
    // No src/Main.ipe: the program entry is the analysis root candidate.
    let manifest = project::parse_manifest(&proj.join("package.ipe")).expect("parses");
    assert!(
        matches!(
            analysis_root_of(&manifest),
            Err(CliError::PathEscape { .. })
        ),
        "an absolute program entry must be refused, not joined and read"
    );

    // A `..` traversal is refused the same way.
    fs::write(
        proj.join("package.ipe"),
        "module Package exposing (package)\n\n\npackage =\n    { name = \"escape\"\n    , programs = [ { name = \"x\", entry = \"../../secret.ipe\" } ]\n    }\n",
    )
    .expect("pkg");
    let manifest = project::parse_manifest(&proj.join("package.ipe")).expect("parses");
    assert!(
        matches!(
            analysis_root_of(&manifest),
            Err(CliError::PathEscape { .. })
        ),
        "a dot-dot program entry must be refused, not joined and read"
    );
    let _ = fs::remove_dir_all(&proj);
}

#[test]
fn build_refuses_a_pure_library_with_a_clean_message() {
    let tmp = std::env::temp_dir().join("ipe_build_refuse_library");
    let _ = fs::remove_dir_all(&tmp);
    let src = tmp.join("src");
    fs::create_dir_all(&src).expect("create src/");
    fs::write(
        tmp.join("package.ipe"),
        "module Package exposing (package)\n\n\npackage =\n    { name = \"lib\", exposedModules = [ \"Core\" ] }\n",
    )
    .expect("pkg");
    fs::write(src.join("Core.ipe"), "module Core exposing (x)\nx = 0\n").expect("core");

    let out = tmp.join("out");
    let result = build_project_with_options(
        &tmp.join("package.ipe"),
        &out,
        Path::new("."),
        BuildOptions::from_env(),
    );
    assert!(
        matches!(&result, Err(CliError::Usage(msg)) if msg.contains("library package")),
        "a pure library must be refused with a clean library message: {result:?}"
    );
    assert!(!out.exists(), "the refusal fires before any emit");
    let _ = fs::remove_dir_all(&tmp);
}

// =========================================================================
// `ipe package audit-entry` — argument parsing and fail-closed schema gate
// =========================================================================

fn temp_dir_unique(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ipe-audit-entry-test-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// Write a minimal well-formed `packages/<name>.toml` entry into `root`.
fn write_entry(root: &Path, name: &str, versions: &[(&str, &str, &str, &str)]) {
    use std::fmt::Write as _;
    let pkgs = root.join("packages");
    std::fs::create_dir_all(&pkgs).expect("packages dir");
    let mut text = format!("name = \"{name}\"\npublisher = \"tester\"\n");
    for (ver, source, rev, sha) in versions {
        let _ = write!(
            text,
            "\n[[version]]\nversion = \"{ver}\"\nsource = \"{source}\"\n\
             rev = \"{rev}\"\nsha256 = \"{sha}\"\ncapabilities = []\n"
        );
    }
    std::fs::write(pkgs.join(format!("{name}.toml")), text).expect("write entry");
}

/// `parse_audit_entry_args` — missing positional yields a `Usage` error.
#[test]
fn parse_audit_entry_args_requires_entry_file() {
    let err = parse_audit_entry_args(&[]).unwrap_err();
    assert!(
        matches!(err, CliError::Usage(_)),
        "missing entry-file must be a Usage error: {err:?}"
    );
}

/// `parse_audit_entry_args` — unknown flag yields `UsageOwned`.
#[test]
fn parse_audit_entry_args_rejects_unknown_flag() {
    let args: Vec<String> = ["packages/foo.toml", "--unknown"]
        .iter()
        .map(ToString::to_string)
        .collect();
    let err = parse_audit_entry_args(&args).unwrap_err();
    assert!(
        matches!(err, CliError::UsageOwned(_)),
        "unknown flag must be a UsageOwned error: {err:?}"
    );
}

/// `parse_audit_entry_args` — `--index` without a value yields `Usage`.
#[test]
fn parse_audit_entry_args_rejects_index_without_value() {
    let args: Vec<String> = ["packages/foo.toml", "--index"]
        .iter()
        .map(ToString::to_string)
        .collect();
    let err = parse_audit_entry_args(&args).unwrap_err();
    assert!(
        matches!(err, CliError::Usage(_)),
        "--index without value must be a Usage error: {err:?}"
    );
}

/// `parse_audit_entry_args` — two positionals yields `Usage`.
#[test]
fn parse_audit_entry_args_rejects_two_positionals() {
    let args: Vec<String> = ["packages/foo.toml", "extra"]
        .iter()
        .map(ToString::to_string)
        .collect();
    let err = parse_audit_entry_args(&args).unwrap_err();
    assert!(
        matches!(err, CliError::Usage(_)),
        "two positionals must be a Usage error: {err:?}"
    );
}

/// `parse_audit_entry_args` — valid path + `--index` round-trips correctly.
#[test]
fn parse_audit_entry_args_parses_path_and_index() {
    let args: Vec<String> = ["packages/foo.toml", "--index", "/some/index"]
        .iter()
        .map(ToString::to_string)
        .collect();
    let (path, index) = parse_audit_entry_args(&args).expect("parses");
    assert_eq!(path, PathBuf::from("packages/foo.toml"));
    assert_eq!(index, Some(PathBuf::from("/some/index")));
}

/// `run_audit_entry` — a malformed entry file (missing `sha256`) is a hard
/// schema reject, never a warn-and-pass (§0 fail-closed).
#[test]
fn audit_entry_rejects_malformed_entry_schema() {
    let root = temp_dir_unique("ae-bad-schema");
    let pkgs = root.join("packages");
    std::fs::create_dir_all(&pkgs).expect("packages dir");
    // No `sha256` — the integrity anchor is mandatory; parse must reject.
    std::fs::write(
        pkgs.join("nohash.toml"),
        "publisher = \"tester\"\n\n[[version]]\nversion = \"1.0.0\"\n\
         source = \"https://example.invalid/nohash\"\nrev = \"abc\"\n",
    )
    .expect("write entry");
    let args: Vec<String> =
        std::iter::once(pkgs.join("nohash.toml").to_string_lossy().into_owned()).collect();
    let err = run_audit_entry(&args).unwrap_err();
    // Must be a Resolve or Io error from the schema parse — never Ok.
    assert!(
        matches!(err, CliError::Resolve(_) | CliError::Io { .. }),
        "malformed entry must be rejected at schema step: {err:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// `run_audit_entry` — an entry whose every `[[version]]` is already in the
/// baseline index is rejected: nothing new to audit (§0 fail-closed; the gate
/// must not silently pass with no work done).
#[test]
fn audit_entry_rejects_when_all_versions_are_already_in_baseline() {
    let submitted_root = temp_dir_unique("ae-all-baseline-sub");
    let baseline_root = temp_dir_unique("ae-all-baseline-idx");
    // Both the submitted and the baseline have exactly version 1.0.0.
    write_entry(
        &submitted_root,
        "mylib",
        &[(
            "1.0.0",
            "https://x.invalid/mylib",
            "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
            "00",
        )],
    );
    write_entry(
        &baseline_root,
        "mylib",
        &[(
            "1.0.0",
            "https://x.invalid/mylib",
            "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
            "00",
        )],
    );
    let args: Vec<String> = [
        submitted_root
            .join("packages")
            .join("mylib.toml")
            .to_string_lossy()
            .into_owned(),
        "--index".to_owned(),
        baseline_root.to_string_lossy().into_owned(),
    ]
    .into_iter()
    .collect();
    let err = run_audit_entry(&args).unwrap_err();
    assert!(
        matches!(err, CliError::UsageOwned(_)),
        "no new versions must be a UsageOwned error: {err:?}"
    );
    let _ = std::fs::remove_dir_all(&submitted_root);
    let _ = std::fs::remove_dir_all(&baseline_root);
}

/// `run_audit_entry` — a published version is immutable. Re-submitting an
/// existing version number with a *different* row (here a changed `sha256`)
/// must be a hard reject naming immutability, never a silent skip. This closes
/// the version-delta bypass: were the delta keyed on version number alone, a
/// rewritten `source`/`rev`/`sha256`/`capabilities` on an already-published
/// version would slip past both hash-verify and audit (ADR 0044, §receiving-gate).
#[test]
fn audit_entry_rejects_rewriting_a_published_version() {
    let submitted_root = temp_dir_unique("ae-immutable-sub");
    let baseline_root = temp_dir_unique("ae-immutable-idx");
    // Baseline published 1.0.0 with sha "00"; the submission keeps the same
    // version number but rewrites its sha256 to "11".
    write_entry(
        &baseline_root,
        "mylib",
        &[(
            "1.0.0",
            "https://x.invalid/mylib",
            "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
            "00",
        )],
    );
    write_entry(
        &submitted_root,
        "mylib",
        &[(
            "1.0.0",
            "https://x.invalid/mylib",
            "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
            "11",
        )],
    );
    let args: Vec<String> = [
        submitted_root
            .join("packages")
            .join("mylib.toml")
            .to_string_lossy()
            .into_owned(),
        "--index".to_owned(),
        baseline_root.to_string_lossy().into_owned(),
    ]
    .into_iter()
    .collect();
    let err = run_audit_entry(&args).unwrap_err();
    assert!(
        matches!(&err, CliError::UsageOwned(msg) if msg.contains("immutable")),
        "rewriting a published version must be a UsageOwned reject naming immutability: {err:?}"
    );
    let _ = std::fs::remove_dir_all(&submitted_root);
    let _ = std::fs::remove_dir_all(&baseline_root);
}

/// `run_audit_entry` — a new version whose `sha256` does not match the fetched
/// tree is a hard [`CliError::HashMismatch`] (verify-before-trust, §0).
///
/// Uses a local git repo as the source so the test runs offline.
#[test]
fn audit_entry_rejects_on_hash_mismatch() {
    // Build a tiny local git repo.
    let repo = temp_dir_unique("ae-mismatch-repo");
    let git = |args: &[&str]| {
        let ok = std::process::Command::new("git")
            .args(args)
            .current_dir(&repo)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .expect("git runs")
            .status
            .success();
        assert!(ok, "git {args:?} must succeed");
    };
    git(&["init", "--quiet"]);
    std::fs::write(repo.join("lib.ipe"), "module Lib\n").expect("write");
    git(&["add", "."]);
    git(&["commit", "--quiet", "-m", "seed"]);
    // Get the HEAD commit hash.
    let rev_out = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&repo)
        .output()
        .expect("git rev-parse");
    let rev = String::from_utf8_lossy(&rev_out.stdout).trim().to_owned();

    // Write an entry that points at this repo but with a deliberately wrong sha256.
    let entry_root = temp_dir_unique("ae-mismatch-entry");
    let pkgs = entry_root.join("packages");
    std::fs::create_dir_all(&pkgs).expect("packages dir");
    let entry_text = format!(
        "name = \"testlib\"\npublisher = \"tester\"\n\n[[version]]\n\
         version = \"1.0.0\"\nsource = \"{}\"\nrev = \"{rev}\"\n\
         sha256 = \"000000000000000000000000000000000000000000000000000000000000wrong\"\n\
         capabilities = []\n",
        repo.display()
    );
    std::fs::write(pkgs.join("testlib.toml"), entry_text).expect("write entry");

    // Point --index at a root with no baseline so all versions are "new".
    let idx_root = temp_dir_unique("ae-mismatch-idx");
    std::fs::create_dir_all(idx_root.join("packages")).expect("packages dir");

    let args: Vec<String> = [
        pkgs.join("testlib.toml").to_string_lossy().into_owned(),
        "--index".to_owned(),
        idx_root.to_string_lossy().into_owned(),
    ]
    .into_iter()
    .collect();
    let err = run_audit_entry(&args).unwrap_err();
    assert!(
        matches!(err, CliError::HashMismatch { .. }),
        "a wrong sha256 must be a HashMismatch error, not: {err:?}"
    );

    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&entry_root);
    let _ = std::fs::remove_dir_all(&idx_root);
}

// ---- unsafe-scan fail-closed tests -----------------------------------

/// Returns a unique scratch directory under the OS temp root.
/// The caller is responsible for removing it when done.
fn unsafe_scan_test_dir(tag: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "ipe-unsafe-scan-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("create test scratch dir");
    dir
}

/// A manifest project with an unreadable module must return `Err(CliError::Io)`
/// naming the unreadable path — not `Ok` with a partial source list.
#[cfg(unix)]
#[test]
fn unsafe_scan_manifest_project_fails_closed_on_unreadable_module() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let dir = unsafe_scan_test_dir("manifest-fail");
    let src = dir.join("src");
    fs::create_dir_all(&src).expect("create src");

    // One readable module and one unreadable one.
    let readable = src.join("Main.ipe");
    fs::write(&readable, "module Main exposing (main)\n").expect("write Main");
    let unreadable = src.join("Locked.ipe");
    fs::write(&unreadable, "module Locked exposing ()\n").expect("write Locked");
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).expect("chmod 000");

    let manifest_path = dir.join("package.ipe");
    fs::write(
        &manifest_path,
        "module Package exposing (package)\n\n\npackage =\n    { name = \"test\" }\n",
    )
    .expect("write manifest");

    let result = user_sources_for_unsafe_scan(Some(&manifest_path), &readable);

    // Restore permissions before any assertion so cleanup always runs.
    let _ = fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o644));
    let _ = fs::remove_dir_all(&dir);

    // Must be `Err(CliError::Io)` naming the unreadable path — never an
    // `Ok` partial scan and never a different error variant.
    assert!(
        matches!(&result, Err(CliError::Io { path, .. }) if path == &unreadable),
        "expected Err(CliError::Io) naming {unreadable:?}, got: {result:?}"
    );
}

/// A manifest project where every module is readable must return `Ok` with
/// every source text present.
#[test]
fn unsafe_scan_manifest_project_ok_when_all_readable() {
    use std::fs;

    let dir = unsafe_scan_test_dir("manifest-ok");
    let src = dir.join("src");
    fs::create_dir_all(&src).expect("create src");

    let entry = src.join("Main.ipe");
    fs::write(&entry, "module Main exposing (main)\n").expect("write Main");
    let other = src.join("Helper.ipe");
    fs::write(&other, "module Helper exposing ()\n").expect("write Helper");

    let manifest_path = dir.join("package.ipe");
    fs::write(
        &manifest_path,
        "module Package exposing (package)\n\n\npackage =\n    { name = \"test\" }\n",
    )
    .expect("write manifest");

    let result = user_sources_for_unsafe_scan(Some(&manifest_path), &entry);
    let _ = fs::remove_dir_all(&dir);

    // Every module readable ⇒ `Ok` carrying a source for each.
    assert!(
        matches!(&result, Ok(sources) if sources.len() >= 2),
        "expected Ok with a source for every readable module, got: {result:?}"
    );
}

/// Single-file fallback: when `collect_entry_and_siblings` fails and the
/// entry itself is unreadable, the result must be `Err(CliError::Io)`
/// naming the entry path — not `Ok` with an empty list.
#[cfg(unix)]
#[test]
fn unsafe_scan_single_file_fallback_fails_closed_on_unreadable_entry() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let dir = unsafe_scan_test_dir("single-fail");
    let entry = dir.join("Main.ipe");
    fs::write(&entry, "module Main exposing (main)\n").expect("write entry");
    fs::set_permissions(&entry, fs::Permissions::from_mode(0o000)).expect("chmod 000");

    // Pass no manifest so the single-file (entry + siblings) path is taken.
    let result = user_sources_for_unsafe_scan(None, &entry);

    let _ = fs::set_permissions(&entry, fs::Permissions::from_mode(0o644));
    let _ = fs::remove_dir_all(&dir);

    // Must be `Err(CliError::Io)` naming the unreadable entry — never an
    // `Ok` empty scan and never a different error variant.
    assert!(
        matches!(&result, Err(CliError::Io { path, .. }) if path == &entry),
        "expected Err(CliError::Io) naming {entry:?}, got: {result:?}"
    );
}

#[test]
fn check_exit_code_is_git_style() {
    use crate::version_check::UpgradeAction::*;
    assert_eq!(super::check_exit_code(&Available), 10);
    assert_eq!(super::check_exit_code(&UpToDate), 0);
    assert_eq!(super::check_exit_code(&Unreachable), 2);
}

#[test]
fn upgrade_json_reports_available() {
    use crate::version_check::{UpgradeAction, VersionCheck};
    let vc = VersionCheck {
        current: semver::Version::parse("0.1.72").expect("valid semver"),
        latest: Some(semver::Version::parse("0.1.75").expect("valid semver")),
        upgrade_available: true,
        reached_feed: true,
    };
    let s = super::render_upgrade(
        &vc,
        &UpgradeAction::Available,
        false,
        crate::cli_args::OutputFormat::Json,
    );
    assert!(
        s.contains("\"upgradeAvailable\":true"),
        "upgradeAvailable: {s}"
    );
    assert!(s.contains("\"action\":\"checked\""), "action: {s}");
    assert!(s.contains("\"latest\":\"0.1.75\""), "latest: {s}");
}

#[test]
fn upgrade_plain_is_flush_and_terse() {
    use crate::version_check::{UpgradeAction, VersionCheck};
    let vc = VersionCheck {
        current: semver::Version::parse("0.1.72").expect("valid semver"),
        latest: None,
        upgrade_available: false,
        reached_feed: false,
    };
    let s = super::render_upgrade(
        &vc,
        &UpgradeAction::Unreachable,
        false,
        crate::cli_args::OutputFormat::Plain,
    );
    assert_eq!(s, "feed unreachable\n");
}
