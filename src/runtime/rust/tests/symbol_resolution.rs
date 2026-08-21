//! Regression test: every kernel's emit symbol must resolve to a real `pub fn`
//! in the runtime source (`src/**/*.rs`), OR appear in the explicit
//! `KNOWN_DEAD_OR_EPILOGUE` allowlist.
//!
//! **The emit symbol's source.** A kernel's emitted runtime function name is
//! defined once, in `ipe_kernels::StdlibKernel::def().runtime_fn` — the
//! emit-symbol SSOT. The backend's `naming::kernel_name(k)` is a zero-cost
//! projection of that field, so iterating `StdlibKernel::ALL` and reading
//! `def().runtime_fn` yields exactly the strings the backend emits.
//!
//! **Why this matters.** `callee_name()` in `emit_expr.rs` emits that symbol as a
//! bare Rust identifier in generated code.  A wrong name compiles fine in the Ipê
//! backend (it's just a string) but produces an `undefined` error when `cargo
//! build` runs on the generated project.  This test makes that class of bug a
//! failure of the test suite rather than a user-facing "cargo build failed"
//! surprise.
//!
//! **Allowlist rationale.**  Some `kernel_name()` entries are never reached by
//! the generic `callee_name()` path because dedicated emit functions intercept
//! those `KernelFn` variants first.  Their names in `naming.rs` are therefore
//! dead for the emit path; we keep them allowlisted rather than deleting them
//! so that future visitors understand the dispatch structure.  The epilogue entry
//! (`list_map_consume`) is defined inline in the generated-code preamble, not in
//! the runtime library.

use std::collections::HashSet;
use std::path::PathBuf;

/// Emit symbols never reached via the generic `callee_name()` path (a dedicated
/// emit function intercepts the variant first), OR defined in the generated-code
/// epilogue rather than the runtime.  See per-entry rationale below.
const KNOWN_DEAD_OR_EPILOGUE: &[&str] = &[
    // ── Build-time: env_public embeds the whitelisted public environment
    //         values into the emitted binary at compile time; there is no
    //         runtime fn to resolve — the value is a baked constant. ──────────
    "env_public",
    // ── Dead: emit_task_retry_call constructs RetryPolicy / ShouldRetry
    //         values inline for the builder variants; only task_retry_with has
    //         a real runtime fn. These name strings are never emitted. ────────
    "task_default_retry_policy",
    "task_exponential_backoff",
    "task_linear_backoff",
    "task_retry_on",
    "task_with_base_ms",
    "task_with_jitter",
    "task_with_kind",
    "task_with_max_attempts",
    "task_with_retry_on",
    // ── Dead: emit_http_builder_call constructs an HttpRequest struct inline
    //         for these variants; the name string is never used. ─────────────
    "http_default_request",
    "http_with_method",
    "http_with_body",
    "http_with_header",
    "http_with_timeout",
    // Go-parity builders — same inline clone-and-reassign emission.
    "http_with_url",
    "http_with_follow_redirects",
    "http_with_max_redirects",
    // ── Dead: emit_expr's DbDefaultMigration arm emits the `Migration`
    //         record struct literal inline; this name string is never emitted.
    "db_default_migration",
    // ── Dead: emit_web_route generates a closure expression, not a function
    //         call. ──────────────────────────────────────────────────────────
    "web_route",
    // ── Dead: emit_console_call synthesises the CLI entry-point block inline. ───
    "ipe_console_app_",
    // ── Dead: emit_ui_call emits ipe_runtime_rust::ui::render::ui_layout_with_vecs
    //         for UiLayoutWith; the bare "ui_layout_with" name is not used.
    //         Note: ui_layout_with_vecs IS in the runtime; this entry is for
    //         the stub "ui_layout_with" name that never reaches callee_name(). ──
    "ui_layout_with",
    // ── Epilogue: defined in the generated-code preamble (preamble.rs), not
    //         shipped as part of the runtime library. ─────────────────────────
    "list_map_consume",
    // ── Dead: `PubSub.topic : String -> Topic a` erases to the identity over
    //         the topic-name String; emit_expr emits the argument directly, so
    //         this name string never reaches a runtime call. ──────────────────
    "pubsub_topic",
    // ── Dead: `Store.eq` / `Store.eqBy` are the typed accessor query leaves;
    //         the lowering intercept reads the `.field` accessor and synthesises
    //         a `Compare` `Cond` (for `eq`) or a call to the `eqByNamed` helper
    //         (for `eqBy`) directly, so these name strings never reach a runtime
    //         call. ──────────────────────────────────────────────────────────
    "store_eq_col",
    "store_eq_by",
];

fn walk(dir: &std::path::Path, fn_re: &regex::Regex, out: &mut HashSet<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, fn_re, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs")
            && let Ok(content) = std::fs::read_to_string(&path)
        {
            for cap in fn_re.captures_iter(&content) {
                out.insert(cap[1].to_string());
            }
        }
    }
}

#[test]
fn every_kernel_name_resolves_to_runtime_fn() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    // ── 1. Collect every kernel's emit symbol from the SSOT ──────────────────
    // `StdlibKernel::def().runtime_fn` is the single source the backend's
    // `naming::kernel_name` projects; iterating `ALL` yields exactly the symbols
    // the emitted code names.
    let naming_symbols: HashSet<String> = ipe_kernels::StdlibKernel::ALL
        .iter()
        .map(|k| k.def().runtime_fn.to_string())
        .collect();
    assert!(
        !naming_symbols.is_empty(),
        "StdlibKernel::ALL is empty — the emit-symbol SSOT is broken"
    );

    // ── 2. Walk src/runtime/rust/src/**/*.rs: collect all `pub fn` names ─
    let runtime_src_dir = root.join("src");
    let mut runtime_fns: HashSet<String> = HashSet::new();
    let fn_re = regex::Regex::new(r"pub fn ([a-z_][a-z0-9_]*)").expect("fn_re");

    walk(&runtime_src_dir, &fn_re, &mut runtime_fns);
    assert!(
        !runtime_fns.is_empty(),
        "runtime pub-fn walk found zero functions — the runtime src path is broken"
    );

    // ── 3. Build the allowlist set ────────────────────────────────────────────
    let allowlist: HashSet<&str> = KNOWN_DEAD_OR_EPILOGUE.iter().copied().collect();

    // ── 4. Assert every kernel emit symbol is reachable ──────────────────────
    let mut unresolved: Vec<String> = naming_symbols
        .iter()
        .filter(|sym| !runtime_fns.contains(*sym) && !allowlist.contains(sym.as_str()))
        .cloned()
        .collect();
    unresolved.sort();

    assert_eq!(
        unresolved,
        Vec::<String>::new(),
        "kernel emit symbol(s) don't exist as `pub fn` in the runtime \
         AND aren't in KNOWN_DEAD_OR_EPILOGUE.\n\
         Fix: either (a) add/rename the runtime function, (b) fix the \
         `runtime_fn` in the kernel's `def()`, \
         or (c) add the symbol to KNOWN_DEAD_OR_EPILOGUE with a comment explaining why \
         the generic callee_name() path never reaches it.\n\
         Unresolved: {unresolved:?}"
    );
}
