//! Regression for a `24-tui-kitchen-sink` SEAL violation: `ipe build` exits 0
//! on the Ipe.Tui `argv`-dispatch entry-point idiom (`main = case List.head
//! argsList of Just "live" -> Web.app {...} |> Task.run; _ -> Tui.app {...} |>
//! Task.run`) while the emitted crate fails `cargo build` with two INDEPENDENT
//! E0308 errors.
//! The `tui_entry_case_taskrun` fixture minimises both defects down to plain
//! `println`/`Task` calls so this test needs no Ipe.Tui / Ipe.Web
//! dependency.
//!
//! Defect 1: a string-literal ctor-payload sub-pattern (`Just "live"`)
//! lowered straight to a bare `Pat::Str`, emitted as
//! `IpeMaybe::Just("live")` — Rust rejects a `&str` literal pattern against
//! an owned `String` ctor field. Fixed by desugaring a direct `PStr` ctor-arg
//! into a fresh `String` binder plus an arm guard (`binder == "live"`),
//! mirroring the existingthe C2 nested-cons-in-ctor-payload desugaring.
//!
//! Defect 2: the entry-point `Task.run` elision in `emit_func`
//! (`ipe_backend_rust::emit_expr`) only recognised a FLAT `main = task |>
//! Task.run` body (`func.body` itself a `Call(TaskRun, [inner])`). A
//! `case`-branched body where EVERY arm tail-calls `Task.run` left
//! `ipe_main` returning `IpeResult<E, A>` while the `fn main` epilogue's
//! `block_on(ipe_main())` requires `IpeTask<A>`. Fixed by recursing the
//! elision through `Match` / `If` / `Let` / `Destructure` tail positions —
//! elision only fires when EVERY leaf in tail position is a `Task.run` /
//! `Task.perform` call, so a partially-elided (mismatched-type) body can
//! never be produced.
//!
//! ## Why the emit-only assertions run in the DEFAULT gate
//!
//! `IPE_E2E`-gated tests do not run in the default `cargo nextest` gate —
//! documented BACKLOG blind spot ("Gate blind spot" row). This file's first
//! two tests inspect the emitted `src/main.rs` text (no cargo build) so they
//! run in the DEFAULT gate and pin the regression even when `IPE_E2E` is
//! unset; the third test is the `IPE_E2E`-gated cargo-build-and-run proof
//! that the emitted crate actually compiles AND prints the right thing.

use std::path::{Path, PathBuf};

mod support;

fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn fixture_entry(root: &Path) -> PathBuf {
    root.join("tests")
        .join("golden")
        .join("tui_entry_case_taskrun")
        .join("Main.ipe")
}

/// Build the fixture and return the emitted `src/main.rs` text. `None` when
/// the runtime resolver is unavailable in this environment (mirrors the
/// resolve-skip convention every other golden test in this suite uses) or
/// when the build itself fails (the caller's `assert!` reports the diag).
fn built_main_rs(root: &Path, out: &Path) -> (Result<(), ipe::CliError>, Option<String>) {
    let entry = fixture_entry(root);
    let _ = std::fs::remove_dir_all(out);
    let Ok(runtime) = ipe::resolve_runtime() else {
        return (Ok(()), None);
    };
    let built = ipe::build(&entry, out, &runtime);
    let main_rs = if built.is_ok() {
        std::fs::read_to_string(out.join("src").join("main.rs")).ok()
    } else {
        None
    };
    (built, main_rs)
}

/// Defect 1 pin: the fixture must be ACCEPTED (ipe exit 0) and the emitted
/// `IpeMaybe::Just` arm must bind a fresh variable guarded by `== "live"`,
/// never re-emit the bare `&str` literal pattern `IpeMaybe::Just("live")`
/// that `cargo` rejects against the owned `String` payload.
#[test]
fn nested_str_literal_ctor_payload_does_not_emit_bare_str_pattern() {
    let root = repo_root();
    let out = std::env::temp_dir().join("ipec_tui_entry_case_taskrun_strlit");
    let (built, main_rs) = built_main_rs(&root, &out);
    assert!(
        built.is_ok(),
        "tui_entry_case_taskrun: must be accepted, got: {built:?}"
    );
    let Some(main_rs) = main_rs else {
        return; // resolver unavailable — skip, matches the other goldens
    };

    assert!(
        !main_rs.contains("IpeMaybe::Just(\"live\")"),
        "a bare `&str` literal pattern against an owned `String` ctor field \
         is a cargo-reject (E0308: expected String, found &str) — the nested \
         `PStr` ctor-arg must desugar to a fresh binder + guard.\n\
         --- match arm lines ---\n{}",
        main_rs
            .lines()
            .filter(|l| l.contains("IpeMaybe::Just"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        main_rs.contains("== \"live\".to_string()") || main_rs.contains("== \"live\""),
        "expected a synthesised `== \"live\"` arm guard recovering the \
         desugared string-literal ctor payload; got:\n{}",
        main_rs
            .lines()
            .filter(|l| l.contains("IpeMaybe::Just") || l.contains("\"live\""))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Defect 2 pin: `ipe_main` must return `IpeTask<…>` (the elided shape the
/// `block_on(ipe_main())` epilogue requires), never `IpeResult<…>` — even
/// though the body is a `case`, not a flat `task |> Task.run` call.
#[test]
fn case_branched_entry_point_elides_task_run_to_ipetask() {
    let root = repo_root();
    let out = std::env::temp_dir().join("ipec_tui_entry_case_taskrun_elision");
    let (built, main_rs) = built_main_rs(&root, &out);
    assert!(
        built.is_ok(),
        "tui_entry_case_taskrun: must be accepted, got: {built:?}"
    );
    let Some(main_rs) = main_rs else {
        return;
    };

    assert!(
        main_rs.contains("fn ipe_main() -> IpeTask<"),
        "ipe_main must return IpeTask<…> even when its body is a `case` \
         whose every arm tail-calls Task.run — the block_on(ipe_main()) \
         epilogue requires a Task, not a Result. Got signature region:\n{}",
        main_rs
            .lines()
            .filter(|l| l.contains("ipe_main") || l.contains("IpeTask") || l.contains("IpeResult"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        !main_rs.contains("fn ipe_main() -> IpeResult<"),
        "ipe_main must not return IpeResult<…> — that is the un-elided shape \
         that mismatches block_on's IpeTask<…> parameter.\n{}",
        main_rs
            .lines()
            .filter(|l| l.contains("ipe_main"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// The load-bearing SEAL proof: under `IPE_E2E=1`, actually `cargo build` the
/// emitted crate and run it, asserting BOTH defects are closed together (a
/// fix for one that reintroduces the other still fails this test) and that
/// the runtime picks the `Just "live"` branch (`mode = Just "live"` in the
/// fixture).
#[test]
fn tui_entry_case_taskrun_builds_and_runs() {
    let root = repo_root();
    let out = std::env::temp_dir().join("ipec_tui_entry_case_taskrun_e2e");
    let Ok(runtime) = ipe::resolve_runtime() else {
        return;
    };
    let entry = fixture_entry(&root);
    let _ = std::fs::remove_dir_all(&out);
    let built = ipe::build(&entry, &out, &runtime);
    assert!(
        built.is_ok(),
        "tui_entry_case_taskrun: must be accepted, got: {built:?}"
    );

    if std::env::var("IPE_E2E").is_err() {
        return;
    }
    let outcome = support::build_and_run_emitted("tui_entry_case_taskrun", &out);
    assert_eq!(
        outcome.exit_code,
        Some(0),
        "tui_entry_case_taskrun: emitted crate must build and exit 0; stdout:\n{}",
        outcome.stdout
    );
    assert_eq!(outcome.stdout.trim(), "live mode", "wrong runtime output");
}
