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
///
/// The `Store.*` accessor-intercept placeholder symbols are NOT listed here —
/// they are derived at test time from
/// [`ipe_kernels::StdlibKernel::ACCESSOR_INTERCEPT_PLACEHOLDERS`], the SSOT.
/// See the `every_kernel_name_resolves_to_runtime_fn` test body below.
const KNOWN_DEAD_OR_EPILOGUE: &[&str] = &[
    // ── Build-time: env_public embeds the whitelisted public environment
    //         values into the emitted binary at compile time; there is no
    //         runtime fn to resolve — the value is a baked constant. ──────────
    "env_public",
    // ── Dead: emit_task_retry_call constructs RetryPolicy / BackoffStrategy
    //         values inline for the builder variants; only task_retry_with has
    //         a real runtime fn. These name strings are never emitted. ────────
    "task_default_retry_policy",
    "task_exponential_backoff",
    "task_linear_backoff",
    "task_retry_on",
    "task_with_base_ms",
    "task_with_jitter",
    "task_with_max_attempts",
    "task_with_retry_on",
    "backoff_linear",
    "backoff_linear_with_jitter",
    "backoff_exponential",
    "backoff_exponential_with_jitter",
    // ── Dead: emit_http_builder_call constructs an HttpRequest struct inline
    //         for these variants; the name string is never used. ─────────────
    "http_default_request",
    "http_with_method",
    "http_with_body",
    "http_with_header",
    "http_with_timeout",
    // parity builders — same inline clone-and-reassign emission.
    "http_with_url",
    "http_with_redirects",
    // ── Dead: emit_expr's DbDefaultMigration arm emits the `Migration`
    //         record struct literal inline; this name string is never emitted.
    "db_default_migration",
    // ── Dead: emit_web_route generates a closure expression, not a function
    //         call. ──────────────────────────────────────────────────────────
    "web_route",
    // ── Dead: emit_web inlines install_web plus the app body, so the
    //         web_app_with descriptor name never reaches a runtime call. ──────
    "web_app_with",
    // ── Dead: emit_console_call synthesises the CLI entry-point block inline. ───
    "ipe_console_app_",
    // ── Dead: emit_worker_call synthesises the worker entry-point block inline. ─
    "ipe_worker_app_",
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
    // (`js_send` / `js_subscribe` — the Ipe.Ffi.Js port transport — are REAL runtime
    // fns in `js_port.rs`, so they resolve and are deliberately NOT allowlisted.)
    // ── Dead: the config-tag ADT constructors (`Host.loopback` / `Level.warn`
    //         / `Web.strict` / …) are emitted inline as their raw `Int` tag by
    //         emit_config_ctor_call; these name strings never reach a runtime
    //         call. The setting builders they feed (`ipe_setting_host_bind` / …)
    //         ARE real runtime fns. ─────────────────────────────────────────────
    "config_host_mode_loopback",
    "config_host_mode_all_interfaces",
    "config_host_mode_env_driven",
    "config_log_level_debug",
    "config_log_level_info",
    "config_log_level_warn",
    "config_log_level_error",
    "config_csrf_mode_strict",
    "config_csrf_mode_inherit",
    "config_revocation_mode_off",
    "config_revocation_mode_store",
    // NOTE: The accessor-typed `Store.*` query leaves and column-spec builders
    // are omitted here.  Their `runtime_fn` name strings are derived at test
    // time from `ipe_kernels::StdlibKernel::ACCESSOR_INTERCEPT_PLACEHOLDERS`
    // (the SSOT) and unioned into the allowlist inside the test function below.
    // Adding them here a second time would create a parallel list that can
    // drift when a new placeholder kernel is added — exactly the problem the
    // SSOT is meant to prevent.
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
    //    (`pub const fn` is also a callable runtime symbol — e.g. the nullary
    //    `term_color_*` colour constructors — so the pattern accepts `const`.)
    let runtime_src_dir = root.join("src");
    let mut runtime_fns: HashSet<String> = HashSet::new();
    let fn_re = regex::Regex::new(r"pub (?:const )?fn ([a-z_][a-z0-9_]*)").expect("fn_re");

    walk(&runtime_src_dir, &fn_re, &mut runtime_fns);
    assert!(
        !runtime_fns.is_empty(),
        "runtime pub-fn walk found zero functions — the runtime src path is broken"
    );

    // ── 3. Build the allowlist set ────────────────────────────────────────────
    // Seed from the static list, then union in the accessor-intercept placeholder
    // names derived from the SSOT (`StdlibKernel::ACCESSOR_INTERCEPT_PLACEHOLDERS`).
    // This means adding a 25th placeholder kernel only requires updating the SSOT
    // constant — this test and the point-free gate both update automatically.
    let mut allowlist: HashSet<String> = KNOWN_DEAD_OR_EPILOGUE
        .iter()
        .map(|s| s.to_string())
        .collect();
    for k in ipe_kernels::StdlibKernel::ACCESSOR_INTERCEPT_PLACEHOLDERS {
        allowlist.insert(k.def().runtime_fn.to_string());
    }

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

/// Reads every `.rs` file under `dir`, recursively.
fn read_sources(dir: &std::path::Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            read_sources(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs")
            && let Ok(content) = std::fs::read_to_string(&path)
        {
            out.push(content);
        }
    }
}

/// The nesting-depth change `c` makes after `prev`; the `>` of `->` closes nothing.
const fn depth_step(prev: char, c: char) -> i32 {
    match c {
        '(' | '<' | '[' => 1,
        ')' | ']' => -1,
        '>' if prev != '-' => -1,
        _ => 0,
    }
}

/// Splits `s` at its top-level commas, trimming each part and dropping empty ones.
fn split_top_level(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0;
    let mut prev = ' ';
    for c in s.chars() {
        depth += depth_step(prev, c);
        if c == ',' && depth == 0 {
            parts.push(std::mem::take(&mut current));
        } else {
            current.push(c);
        }
        prev = c;
    }
    parts.push(current);
    parts
        .into_iter()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// Splits `s`, which opens with `open`, into the text inside that balanced group and the text after it.
fn balanced_group(s: &str, open: char) -> Option<(String, String)> {
    let body = s.strip_prefix(open)?;
    let mut depth = 1;
    let mut prev = open;
    for (at, c) in body.char_indices() {
        depth += depth_step(prev, c);
        if depth == 0 {
            let inside = body.get(..at)?;
            let after = body.get(at + c.len_utf8()..)?;
            return Some((inside.to_string(), after.to_string()));
        }
        prev = c;
    }
    None
}

/// The whitespace-normalised header of the unique `pub fn symbol` in `sources`.
///
/// The header spans the generics, parameters, return type, and where clause;
/// `None` when the symbol is defined zero or several times.
fn runtime_fn_header(sources: &[String], symbol: &str) -> Option<String> {
    let pattern = format!(r"pub (?:const )?fn {}\b", regex::escape(symbol));
    let def_re = regex::Regex::new(&pattern).ok()?;
    let mut headers = sources.iter().flat_map(|content| {
        def_re.find_iter(content).filter_map(|m| {
            let mut header = String::new();
            let mut depth = 0;
            let mut prev = ' ';
            for c in content.get(m.end()..)?.chars() {
                if depth == 0 && (c == '{' || c == ';') {
                    return Some(header.split_whitespace().collect::<Vec<_>>().join(" "));
                }
                depth += depth_step(prev, c);
                header.push(c);
                prev = c;
            }
            None
        })
    });
    let header = headers.next()?;
    headers.next().is_none().then_some(header)
}

/// Whether each parameter of the runtime fn `header` is callable.
///
/// A parameter is callable when its type is `impl Fn…` / `dyn Fn…` (any of
/// `Fn` / `FnMut` / `FnOnce`), or a generic whose bound is one.
fn callable_params(header: &str) -> Option<Vec<bool>> {
    let (generics, rest) = if header.starts_with('<') {
        balanced_group(header, '<')?
    } else {
        (String::new(), header.to_string())
    };
    let (params, tail) = balanced_group(rest.trim_start(), '(')?;
    let where_clause = tail.split_once("where").map_or("", |(_, w)| w);
    let fn_trait = regex::Regex::new(r"\b(?:Fn|FnMut|FnOnce)\(").ok()?;
    let erased_fn = regex::Regex::new(r"\b(?:dyn|impl) (?:Fn|FnMut|FnOnce)\(").ok()?;
    let fn_bounded: HashSet<String> = split_top_level(&generics)
        .into_iter()
        .chain(split_top_level(where_clause))
        .filter_map(|g| {
            let (name, bound) = g.split_once(':')?;
            fn_trait.is_match(bound).then(|| name.trim().to_string())
        })
        .collect();
    Some(
        split_top_level(&params)
            .iter()
            .filter_map(|p| p.split_once(':').map(|(_, ty)| ty.trim()))
            .map(|ty| {
                erased_fn.is_match(ty) || fn_bounded.contains(ty.trim_start_matches('&').trim())
            })
            .collect(),
    )
}

#[test]
fn declared_arg_order_matches_runtime_signature() {
    use ipe_kernels::{ArgOrder, StdlibKernel, TyShape};

    let mut sources = Vec::new();
    read_sources(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut sources,
    );
    assert!(!sources.is_empty(), "runtime source walk found no files");

    let mut confirmed = 0_usize;
    let mut mismatches = Vec::new();
    for kernel in StdlibKernel::ALL {
        let def = kernel.def();
        let takes_fn_then_container = def.arity == 2
            && matches!(
                def.shape,
                Some(TyShape::Fun(TyShape::Fun(..), TyShape::Fun(second, _)))
                    if !matches!(second, TyShape::Fun(..))
            );
        if !takes_fn_then_container {
            continue;
        }
        let runtime_order = runtime_fn_header(&sources, def.runtime_fn)
            .and_then(|header| callable_params(&header))
            .and_then(|callable| match callable.as_slice() {
                [true, false] => Some(ArgOrder::IpeOrder),
                [false, true] => Some(ArgOrder::ContainerFirst),
                _ => None,
            });
        match runtime_order {
            Some(order) if order == def.arg_order => confirmed += 1,
            Some(order) => mismatches.push(format!(
                "{kernel:?}: declared {:?}, `{}` takes {order:?}",
                def.arg_order, def.runtime_fn
            )),
            None if def.arg_order == ArgOrder::ContainerFirst => mismatches.push(format!(
                "{kernel:?}: declared ContainerFirst, but `{}` has no single signature taking \
                 a container then a function",
                def.runtime_fn
            )),
            None => {}
        }
    }
    assert!(
        confirmed > 0,
        "no kernel's argument order was confirmed against the runtime"
    );
    assert!(
        mismatches.is_empty(),
        "a kernel's `ArgOrder` disagrees with its runtime function's parameter order; fix the \
         row's `ArgOrder` in `StdlibKernel::identity` (the backend swaps and the lowering walks \
         reverse exactly the `ContainerFirst` rows):\n{}",
        mismatches.join("\n")
    );
}
