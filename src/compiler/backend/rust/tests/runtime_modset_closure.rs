//! Fast static SEAL lint for the emitted runtime-module set.
//!
//! The backend trims `ipe_runtime/mod.rs` to a base module set and appends
//! feature-modules per `uses_*` flag. Each appended module carries a
//! `use crate::<dep>` closure that MUST itself be declared in the same emitted
//! `mod.rs` — otherwise `ipe` exits 0 and the emitted crate fails `cargo build`
//! (E0432/E0425/E0412), the module-set SEAL breach class.
//!
//! This test proves that closure WITHOUT invoking cargo: for every reachable
//! combination of the `Module` `uses_*` flags it emits the `ipe_runtime/mod.rs`
//! via the real backend, then checks that every top-level module the emitted
//! `mod.rs` declares has all of its UNCONDITIONAL `use crate::<dep>::`
//! references also declared. It is the fast counterpart to the ground-truth
//! `ipe`-crate `seal_modset` E2E gate (which does the actual `cargo build`).
//!
//! "Unconditional" excludes any `use crate::<dep>` immediately preceded by a
//! `#[cfg(...)]` attribute or living inside a `#[cfg(test)]` module — those are
//! feature-/test-gated and their target module is pulled in by the same cfg,
//! not by the base append (matches the runtime's db-gated telemetry refs, which
//! are correctly NOT a SEAL requirement).

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use ipe_backend::Backend;
use ipe_backend_rust::RustBackend;
use ipe_intern::Interner;
use ipe_ir::{ModPath, Module, Program};

/// Locate the runtime source tree (`src/runtime/rust/src`) — the vendored
/// module files whose `use crate::` closure this test checks.
fn resolve_runtime() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("IPE_RUNTIME_DIR") {
        let p = PathBuf::from(dir);
        if p.is_dir() {
            return Some(p);
        }
    }
    let cwd = std::env::current_dir().ok()?;
    let mut here: Option<&Path> = Some(cwd.as_path());
    while let Some(dir) = here {
        let candidate = dir.join("src").join("runtime").join("rust").join("src");
        if candidate.is_dir() {
            return Some(candidate);
        }
        here = dir.parent();
    }
    None
}

/// A `Module` with every module-set-relevant `uses_*` flag independently
/// settable, all else empty. The record fold is index-driven: bit `i` of `mask`
/// sets flag `i`.
///
/// `uses_ffi` is excluded: it appends `mod ffi;` to `main.rs` (not to
/// `ipe_runtime/mod.rs`) and requires FFI emission inputs the body-free emit
/// here cannot supply — it is orthogonal to the runtime-module closure.
const FLAG_COUNT: usize = 17;

#[allow(clippy::similar_names)] // `uses_ui` / `uses_tui` are intentionally alike
fn module_for_mask(name: ipe_intern::Symbol, mask: u32) -> Module {
    let f = |i: usize| mask & (1 << i) != 0;
    // `uses_ui` is forced true whenever `uses_tui` is true at the lowerer; mirror
    // that invariant here so the emitted set matches a real program (a Tui shape
    // always references Ui element kernels).
    let uses_tui = f(4);
    let uses_ui = f(2) || uses_tui;
    Module {
        name: ModPath(vec![name]),
        types: vec![],
        funcs: vec![],
        entry: None,
        records: vec![],
        uses_tea: f(0),
        uses_server: f(1),
        uses_ui,
        uses_web: f(3),
        uses_tui,
        uses_webview: f(5),
        uses_css: f(6),
        uses_auth: f(7),
        uses_websocket: f(8),
        uses_email: f(9),
        uses_env_public: f(10),
        uses_http: f(11),
        uses_config: f(12),
        uses_compression: f(13),
        uses_csv: f(14),
        uses_crypto: f(15),
        uses_jwt: f(16),
        uses_debug: false,
        uses_ffi: false,
    }
}

/// Extract the set of top-level module names DECLARED by an emitted
/// `ipe_runtime/mod.rs` — every `pub mod X;` / `mod X;` line.
fn declared_modules(mod_rs: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in mod_rs.lines() {
        let t = line.trim();
        // Skip attributes like `#[cfg(feature = "tui")]` that may precede a decl —
        // the decl line itself is what we parse.
        let rest = t
            .strip_prefix("pub mod ")
            .or_else(|| t.strip_prefix("mod "));
        if let Some(rest) = rest
            && let Some(name) = rest.strip_suffix(';')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            out.insert(name.to_owned());
        }
    }
    out
}

/// For one runtime module `name`, read its source (`<name>.rs` or
/// `<name>/mod.rs`) and return the set of top-level modules it references via an
/// UNCONDITIONAL `use crate::<dep>` (dep = first path segment after `crate::`).
/// cfg-/test-gated `use`s are excluded (their target is pulled in by the same
/// cfg, not the base append).
fn unconditional_crate_deps(runtime_root: &Path, name: &str) -> BTreeSet<String> {
    let mut deps = BTreeSet::new();
    let flat = runtime_root.join(format!("{name}.rs"));
    let dir = runtime_root.join(name);
    if flat.is_file() {
        // `<name>.rs` is a top-level module — its module path is `crate::<name>`,
        // so `super` reaches the crate root (1 hop).
        scan_file_deps(&flat, 1, &mut deps);
    }
    if dir.is_dir() {
        // Files under `<name>/`: `<name>/mod.rs` is `crate::<name>` (1 super to
        // root); `<name>/foo.rs` is `crate::<name>::foo` (2); each nested dir +1.
        collect_dir_deps(&dir, 1, &mut deps);
    }
    deps
}

/// Recurse a module's directory, scanning each `.rs` file for crate-root deps.
/// `mod_depth` is the module-path depth of `<dir>/mod.rs` (= supers to root).
fn collect_dir_deps(dir: &Path, mod_depth: usize, deps: &mut BTreeSet<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_dir_deps(&path, mod_depth + 1, deps);
        } else if path.extension().is_some_and(|e| e == "rs") {
            // `mod.rs` sits at `mod_depth`; any other `<file>.rs` is one module
            // level deeper (`crate::…::<dir>::<file>`).
            let is_mod_rs = path.file_name().is_some_and(|n| n == "mod.rs");
            let supers = if is_mod_rs { mod_depth } else { mod_depth + 1 };
            scan_file_deps(&path, supers, deps);
        }
    }
}

/// Extract every UNCONDITIONAL crate-root module dep from one file whose module
/// reaches the crate root in `supers_to_root` `super::` hops. Both
/// `use crate::<dep>` and a matching-length `super`-chain name a top-level
/// module. cfg-/test-gated `use`s are excluded.
fn scan_file_deps(path: &Path, supers_to_root: usize, deps: &mut BTreeSet<String>) {
    let src = std::fs::read_to_string(path).unwrap_or_default();

    let mut in_cfg_test_mod = false;
    let mut cfg_test_brace_depth: i32 = 0;
    let mut prev_is_cfg_attr = false;
    for line in src.lines() {
        let t = line.trim();

        if in_cfg_test_mod {
            let opens = i32::try_from(t.matches('{').count()).unwrap_or(0);
            let closes = i32::try_from(t.matches('}').count()).unwrap_or(0);
            cfg_test_brace_depth += opens - closes;
            if cfg_test_brace_depth <= 0 {
                in_cfg_test_mod = false;
            }
            prev_is_cfg_attr = false;
            continue;
        }
        if t.starts_with("#[cfg(test)]") {
            in_cfg_test_mod = true;
            cfg_test_brace_depth = 0;
            prev_is_cfg_attr = false;
            continue;
        }

        let is_cfg_attr = t.starts_with("#[cfg(") || t.starts_with("#[cfg_attr(");

        if !prev_is_cfg_attr && let Some(dep) = crate_root_dep(t, supers_to_root) {
            deps.insert(dep);
        }

        prev_is_cfg_attr = is_cfg_attr;
    }
}

/// If `line` is a `use` naming a top-level (crate-root) module — either
/// `use crate::<dep>` or `use super::…::<dep>` whose `super`-chain reaches the
/// crate root — return `<dep>`. Otherwise `None`.
fn crate_root_dep(line: &str, supers_to_root: usize) -> Option<String> {
    let after = line.strip_prefix("use ")?;
    let rest = if let Some(r) = after.strip_prefix("crate::") {
        r
    } else {
        // Consume exactly `supers_to_root` `super::` segments to reach the root;
        // any remaining `super::` (or too few) does NOT name a crate-root module.
        let mut r = after;
        let mut climbed = 0;
        while let Some(next) = r.strip_prefix("super::") {
            r = next;
            climbed += 1;
        }
        if climbed != supers_to_root {
            return None;
        }
        r
    };
    let seg: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    if seg.is_empty() {
        return None;
    }
    // A module DEPENDENCY reaches INTO a module: `crate::tea::{…}` /
    // `crate::web::pubsub`. A bare `use crate::IpeMaybe;` (no trailing `::`)
    // imports an ITEM re-exported at the crate root via the `mod.rs` glob
    // (`pub use <mod>::*`), not a module — it needs no separate declaration.
    match rest.strip_prefix(&seg) {
        Some(tail) if tail.starts_with("::") => Some(seg),
        _ => None,
    }
}

/// The core assertion: every reachable `uses_*` combination emits a `mod.rs`
/// whose declared module set is CLOSED under every declared module's
/// unconditional `use crate::<dep>` references.
#[test]
fn emitted_modset_is_closed_over_every_flag_combo() {
    let runtime_root =
        resolve_runtime().expect("runtime source tree (src/runtime/rust/src) must resolve");

    let mut interner = Interner::new();
    let main = interner.intern("Main").expect("intern Main");

    // A module's unconditional crate-dep set is a function of its source alone,
    // not the flag mask; scan each module from disk once and reuse the result
    // across every combination.
    let mut dep_cache: HashMap<String, BTreeSet<String>> = HashMap::new();

    for mask in 0u32..(1u32 << FLAG_COUNT) {
        let module = module_for_mask(main, mask);
        let prog = Program {
            modules: vec![module],
        };
        let emitted = RustBackend::new(&interner)
            .emit(&prog)
            .expect("emit must succeed for a body-free program");
        let mod_rs = emitted
            .files
            .get("src/ipe_runtime/mod.rs")
            .expect("emitted project must contain ipe_runtime/mod.rs");

        let declared = declared_modules(mod_rs);

        for m in &declared {
            let deps = dep_cache
                .entry(m.clone())
                .or_insert_with(|| unconditional_crate_deps(&runtime_root, m));
            for dep in deps.iter() {
                assert!(
                    declared.contains(dep),
                    "module-set SEAL breach: emitted `mod.rs` declares `{m}` \
                     (which does `use crate::{dep}`) but does NOT declare `{dep}` \
                     — flag mask {mask:#019b}. The emitted crate would fail `cargo build`. \
                     Declared modules: {declared:?}"
                );
            }
        }
    }
}

/// The always-on-core rule: a runtime module in the always-compiled BASE floor
/// (the module set emitted for a program that uses no `uses_*` feature — mask 0)
/// must NOT unconditionally `use crate::<gated_module>`. A base→gated
/// unconditional edge would force the gated module always-on (re-coupling the
/// two), defeating the point of gating and pulling the gated module's crates
/// into every program. The base floor must be closed under its OWN unconditional
/// deps: every such dep is itself a base module.
///
/// This is the STRUCTURAL guard that keeps a future module on the correct side
/// of the base/gated boundary — a PR that adds a base→gated edge (e.g. an
/// always-on module reaching into a newly-gated `crypto`/`jwt`/`compression`
/// surface) fails HERE, statically, instead of silently re-coupling the deps or
/// breaking the emitted `cargo build` only under a specific feature combo.
///
/// The base floor is derived, not hardcoded: it is exactly the module set the
/// backend emits for the featureless program, so it tracks the template
/// automatically.
#[test]
fn base_modules_do_not_reach_gated_modules() {
    let runtime_root =
        resolve_runtime().expect("runtime source tree (src/runtime/rust/src) must resolve");

    let mut interner = Interner::new();
    let main = interner.intern("Main").expect("intern Main");

    // The always-compiled BASE floor: the module set emitted with every `uses_*`
    // flag off.
    let base_prog = Program {
        modules: vec![module_for_mask(main, 0)],
    };
    let emitted = RustBackend::new(&interner)
        .emit(&base_prog)
        .expect("emit must succeed for the featureless base program");
    let base_mod_rs = emitted
        .files
        .get("src/ipe_runtime/mod.rs")
        .expect("emitted project must contain ipe_runtime/mod.rs");
    let base = declared_modules(base_mod_rs);

    for m in &base {
        for dep in unconditional_crate_deps(&runtime_root, m) {
            assert!(
                base.contains(&dep),
                "always-on-core rule violation: base module `{m}` does \
                 `use crate::{dep}` unconditionally, but `{dep}` is NOT in the \
                 always-compiled base floor — it is a gated feature module. A \
                 base module must reach only other base modules (outside \
                 `#[cfg(...)]` / `#[cfg(test)]`); move the reach behind a cfg, or \
                 promote `{dep}` to the base floor. Base floor: {base:?}"
            );
        }
    }
}

/// Guard the specific witnesses of the previously-live breaches at the flag
/// level: a `uses_web` shape must declare `tea` (web/pubsub `use crate::tea`),
/// and a `uses_server` shape must declare both `tea` and `http_stream`.
#[test]
fn web_shape_declares_tea() {
    let mut interner = Interner::new();
    let main = interner.intern("Main").expect("intern Main");
    // uses_web only (bit 3).
    let prog = Program {
        modules: vec![module_for_mask(main, 1 << 3)],
    };
    let emitted = RustBackend::new(&interner).emit(&prog).expect("emit");
    let mod_rs = emitted.files.get("src/ipe_runtime/mod.rs").expect("mod.rs");
    let declared = declared_modules(mod_rs);
    assert!(
        declared.contains("tea") && declared.contains("web"),
        "uses_web must declare both `web` and `tea`; got {declared:?}"
    );
}

#[test]
fn server_shape_declares_tea_and_http_stream() {
    let mut interner = Interner::new();
    let main = interner.intern("Main").expect("intern Main");
    // uses_server only (bit 1).
    let prog = Program {
        modules: vec![module_for_mask(main, 1 << 1)],
    };
    let emitted = RustBackend::new(&interner).emit(&prog).expect("emit");
    let mod_rs = emitted.files.get("src/ipe_runtime/mod.rs").expect("mod.rs");
    let declared = declared_modules(mod_rs);
    assert!(
        declared.contains("tea") && declared.contains("server") && declared.contains("http_stream"),
        "uses_server must declare `server`, `tea`, and `http_stream`; got {declared:?}"
    );
}
