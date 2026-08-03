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
//!
//! ## Why not an exhaustive `2^FLAG_COUNT` sweep
//!
//! Brute-forcing every flag mask is `O(2^FLAG_COUNT)` — infeasible as the
//! runtime feature count grows. This SEAL proves the SAME closure in linear
//! time by per-flag closure + monotonicity + composition (the featureset SEAL
//! carries the full argument). The declared-module set is monotone in the mask
//! — the emitter only APPENDS feature modules — so a superset of flags declares
//! a superset of modules. Each module's unconditional `use crate::<dep>` closure
//! is source-only (mask-independent). Therefore: if every singleton flag's
//! declared set is closed under its deps, and the declared set is monotone, then
//! every union is closed (the composed all-flags declaration is exactly the
//! union of the per-flag declarations, and each dep-target it needs is present
//! in the flag that introduced its module). A bounded, deterministic sampled
//! sweep is retained as a backstop.

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
///
/// The proof is per-flag + monotone + composed (see module docs), NOT a
/// `2^FLAG_COUNT` enumeration — it scales linearly.
const FLAG_COUNT: usize = 27;

/// How many random full masks the backstop [`sampled_full_masks_are_closed`]
/// checks, on top of the deterministic corners. Bounded so cost stays constant
/// as `FLAG_COUNT` grows.
const SAMPLE_MASKS: usize = 256;

/// A deterministic full-mask stream (fixed-seed splitmix64 — no entropy/`Date`,
/// so a failure reproduces). Masks are taken modulo the flag space.
fn sampled_masks(count: usize) -> Vec<u32> {
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15; // fixed seed
    let mask_space: u64 = 1u64 << FLAG_COUNT;
    (0..count)
        .map(|_| {
            state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            u32::try_from(z % mask_space).unwrap_or(0)
        })
        .collect()
}

#[allow(clippy::similar_names)] // `uses_ui` / `uses_tui` are intentionally alike
fn module_for_mask(name: ipe_intern::Symbol, mask: u32) -> Module {
    let f = |i: usize| mask & (1 << i) != 0;
    // `uses_ui` is forced true whenever `uses_tui` is true at the lowerer; mirror
    // that invariant here so the emitted set matches a real program (a Tui shape
    // always references Ui element kernels).
    let uses_tui = f(4);
    let uses_ui = f(2) || uses_tui;
    // Every gated surface below (db/tea/server/web/tui/webview/websocket/email/
    // http/config/compression/csv/crypto/jwt/auth/url) reaches a
    // reactor-requiring kernel, so a REAL program that sets any of them also
    // sets `uses_async_runtime` — which restores the tokio spine the per-surface
    // manifest augmenters (`tea_cargo_toml`, `server_cargo_toml`, …) anchor on.
    // Mirror that invariant here: any masked surface flag forces the async spine
    // on, so the SEAL emit sees the same manifest a real program would.
    // `uses_css` is the one whitelisted-pure surface (CssSafety kernels), so it
    // alone does NOT force it.
    let uses_async_runtime = f(0)
        || f(1)
        || f(3)
        || uses_tui
        || f(5)
        || f(7)
        || f(8)
        || f(9)
        || f(11)
        || f(12)
        || f(13)
        || f(14)
        || f(15)
        || f(16)
        || f(17);
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
        uses_time: false,
        uses_env_public: f(10),
        uses_http: f(11),
        uses_config: f(12),
        uses_compression: f(13),
        uses_csv: f(14),
        // `uses_encoding` is a pure surface (no reactor) — like `crypto`/`csv` it
        // is NOT added to the `uses_async_runtime` union above.
        uses_encoding: f(18),
        // `uses_regex` / `uses_uuid` / `uses_random` / `uses_log` /
        // `uses_decimal` / `uses_char_category` are pure surfaces (no reactor) —
        // like `encoding`/`crypto`/`csv`, NOT in the async union. `uses_log` drives
        // `reaches_time_core` (chrono) alongside web / webview (bits 3 / 5);
        // `uses_decimal` (with the `db` surface, bit set elsewhere) drives
        // `reaches_decimal` (rust_decimal); `uses_char_category` is a standalone
        // leaf.
        uses_regex: f(19),
        uses_uuid: f(20),
        uses_random: f(21),
        uses_log: f(22),
        uses_decimal: f(23),
        uses_char_category: f(24),
        // `uses_crypto_core` / `uses_secret` are pure surfaces (no reactor) — like
        // `crypto`/`encoding`, NOT in the async union. `uses_crypto_core` drives
        // `reaches_crypto_core` (the crypto-floor feature); `uses_secret` gates
        // `secret.rs` and implies `crypto-core` (shared `subtle`).
        uses_crypto_core: f(25),
        uses_secret: f(26),
        uses_crypto: f(15),
        uses_jwt: f(16),
        uses_url: f(17),
        uses_debug: false,
        uses_ffi: false,
        uses_async_runtime,
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

/// The set of top-level modules the emitted `ipe_runtime/mod.rs` DECLARES for
/// one flag mask.
#[allow(clippy::expect_used)] // test scaffolding: a body-free emit cannot fail
fn declared_for_mask(interner: &Interner, main: ipe_intern::Symbol, mask: u32) -> BTreeSet<String> {
    let prog = Program {
        modules: vec![module_for_mask(main, mask)],
    };
    let emitted = RustBackend::new(interner)
        .emit(&prog)
        .expect("emit must succeed for a body-free program");
    let mod_rs = emitted
        .files
        .get("src/ipe_runtime/mod.rs")
        .expect("emitted project must contain ipe_runtime/mod.rs");
    declared_modules(mod_rs)
}

/// The per-mask closure obligation: the declared module set is CLOSED under
/// every declared module's unconditional `use crate::<dep>` references. Shared
/// by the per-flag, composed, and sampled proofs so they run an identical check.
/// `dep_cache` memoises the source-only, mask-independent per-module dep scan.
fn assert_mask_closed(
    runtime_root: &Path,
    interner: &Interner,
    main: ipe_intern::Symbol,
    mask: u32,
    dep_cache: &mut HashMap<String, BTreeSet<String>>,
) {
    let declared = declared_for_mask(interner, main, mask);
    for m in &declared {
        let deps = dep_cache
            .entry(m.clone())
            .or_insert_with(|| unconditional_crate_deps(runtime_root, m));
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

// ── (1) per-flag closure — O(FLAG_COUNT) ────────────────────────────────────

/// Every single `uses_*` flag on its own (and the featureless base, mask 0)
/// emits a `mod.rs` whose declared set is closed under its unconditional deps.
/// The base case of the composition argument (module docs).
#[test]
fn each_flag_declares_a_closed_modset() {
    let runtime_root =
        resolve_runtime().expect("runtime source tree (src/runtime/rust/src) must resolve");
    let mut interner = Interner::new();
    let main = interner.intern("Main").expect("intern Main");
    let mut dep_cache: HashMap<String, BTreeSet<String>> = HashMap::new();
    assert_mask_closed(&runtime_root, &interner, main, 0, &mut dep_cache);
    for bit in 0..FLAG_COUNT {
        assert_mask_closed(&runtime_root, &interner, main, 1u32 << bit, &mut dep_cache);
    }
}

// ── (2) monotonicity — the compose glue ─────────────────────────────────────

/// The declared module set distributes over union of flags:
/// `declared(a | b) = declared(a) ∪ declared(b)`. The emitter only APPENDS
/// feature modules, so enabling a flag can only add declarations, never drop
/// one — which lets per-flag closure compose to every combination. Verified on
/// the deterministic mask sample crossed with the singletons.
#[test]
fn declared_modset_is_monotone() {
    let mut interner = Interner::new();
    let main = interner.intern("Main").expect("intern Main");
    for a in sampled_masks(SAMPLE_MASKS) {
        for bit in 0..FLAG_COUNT {
            let b = 1u32 << bit;
            let mut composed = declared_for_mask(&interner, main, a);
            composed.extend(declared_for_mask(&interner, main, b));
            let union_declared = declared_for_mask(&interner, main, a | b);
            assert_eq!(
                union_declared, composed,
                "monotonicity broken: declared(a | b) != declared(a) ∪ \
                 declared(b) — a={a:#019b} b={b:#019b}. Enabling a flag REMOVED a \
                 module declaration; per-flag closure no longer composes and a \
                 targeted combination check is needed for this interaction."
            );
        }
    }
}

// ── (3) composition — per-flag + monotone ⇒ every mask closed ───────────────

/// The composition made explicit: the all-flags declared set equals the UNION
/// of the per-flag declared sets, and that union is closed under its deps. Given
/// [`each_flag_declares_a_closed_modset`] and [`declared_modset_is_monotone`],
/// this stands in for the whole `2^FLAG_COUNT` sweep — any mask's declaration is
/// the union of its set-bit declarations, and every dep-target sits in the flag
/// that introduced the depending module (per-flag closure), so no union can drop
/// a needed module.
#[test]
fn composed_full_modset_is_closed() {
    let runtime_root =
        resolve_runtime().expect("runtime source tree (src/runtime/rust/src) must resolve");
    let mut interner = Interner::new();
    let main = interner.intern("Main").expect("intern Main");
    let all: u32 = (1u32 << FLAG_COUNT) - 1;

    let mut composed: BTreeSet<String> = BTreeSet::new();
    for bit in 0..FLAG_COUNT {
        composed.extend(declared_for_mask(&interner, main, 1u32 << bit));
    }
    let full = declared_for_mask(&interner, main, all);
    assert_eq!(
        full, composed,
        "composition invalid: declared(all) != ⋃ declared(singleton) — the \
         module set is not a pure per-flag union, so per-flag closure does not \
         compose"
    );

    let mut dep_cache: HashMap<String, BTreeSet<String>> = HashMap::new();
    assert_mask_closed(&runtime_root, &interner, main, all, &mut dep_cache);
}

// ── the sampled full-mask backstop (redundancy, not the proof) ──────────────

/// A bounded, deterministic sample of full masks run through the whole per-mask
/// closure check, plus the corners (all-off, all-on, every singleton). A
/// redundancy net, not the primary guarantee; fixed seed ⇒ reproducible;
/// constant cost as `FLAG_COUNT` grows.
#[test]
fn sampled_full_masks_are_closed() {
    let runtime_root =
        resolve_runtime().expect("runtime source tree (src/runtime/rust/src) must resolve");
    let mut interner = Interner::new();
    let main = interner.intern("Main").expect("intern Main");
    let all: u32 = (1u32 << FLAG_COUNT) - 1;
    let mut dep_cache: HashMap<String, BTreeSet<String>> = HashMap::new();

    let mut masks: BTreeSet<u32> = sampled_masks(SAMPLE_MASKS).into_iter().collect();
    masks.insert(0);
    masks.insert(all);
    for bit in 0..FLAG_COUNT {
        masks.insert(1u32 << bit);
    }
    for mask in masks {
        assert_mask_closed(&runtime_root, &interner, main, mask, &mut dep_cache);
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
