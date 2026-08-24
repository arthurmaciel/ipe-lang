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
use ipe_ir::{ModPath, Module, Program, Target};

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
const FLAG_COUNT: usize = 29;

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
        // Fixed false: the `principal` module carries no runtime dependency
        // edge, so it is outside this surface-flag closure matrix.
        uses_principal: false,
        uses_websocket: f(8),
        uses_email: f(9),
        uses_time: false,
        uses_env_public: f(10),
        uses_http: f(11),
        uses_config: f(12),
        uses_compression: f(13),
        uses_csv: f(14),
        // `uses_cache` is a pure surface (no reactor) — like `csv` it is NOT in the
        // `uses_async_runtime` union above. A standalone leaf: gates the `cache`
        // module + the `cache_kernel` feature.
        uses_cache: f(28),
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
        // `uses_json` is a pure surface (no reactor) — NOT in the async union. It
        // gates the two prelude `Value`/`Decoder` aliases and the `json` feature
        // (demoted from the floor). A standalone leaf here; db/config/jwt fold into
        // `reaches_json` via their own bits.
        uses_json: f(27),
        uses_crypto: f(15),
        uses_jwt: f(16),
        uses_url: f(17),
        uses_debug: false,
        uses_ffi: false,
        uses_async_runtime,
    }
}

/// Extract the module name from one `mod.rs` line — handles both the
/// semicolon form (`pub mod foo;`) and the inline-block form
/// (`pub mod foo {`). Returns `None` for any other line.
fn module_name_from_line(line: &str) -> Option<String> {
    let t = line.trim();
    let rest = t
        .strip_prefix("pub mod ")
        .or_else(|| t.strip_prefix("mod "))?;
    // Semicolon form: `pub mod foo;`
    if let Some(name) = rest.strip_suffix(';')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Some(name.to_owned());
    }
    // Inline-block form: `pub mod foo {` (possibly with trailing whitespace
    // before the brace). Captures the name token before the `{`.
    let name: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    if !name.is_empty() {
        let after = rest.get(name.len()..).unwrap_or("").trim_start();
        if after.starts_with('{') {
            return Some(name);
        }
    }
    None
}

/// Extract the set of top-level module names DECLARED by an emitted
/// `ipe_runtime/mod.rs` — every `pub mod X;` / `mod X;` line and every
/// `pub mod X { … }` inline-block declaration. Skips nested child declarations
/// (e.g. `pub mod route;` inside `pub mod web { … }`).
fn declared_modules(mod_rs: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut brace_depth: i32 = 0;
    for line in mod_rs.lines() {
        let t = line.trim();
        if brace_depth == 0
            && let Some(name) = module_name_from_line(line)
        {
            out.insert(name);
        }
        let opens = i32::try_from(t.matches('{').count()).unwrap_or(0);
        let closes = i32::try_from(t.matches('}').count()).unwrap_or(0);
        brace_depth = (brace_depth + opens - closes).max(0);
    }
    out
}

/// Every top-level module DECLARATION line in an emitted `ipe_runtime/mod.rs`,
/// as it literally appears — a `Vec`, NOT a set, so a module declared twice
/// shows up twice. Distinct from [`declared_modules`], whose `BTreeSet` silently
/// dedups and so cannot witness a double `pub mod`. Skips nested child
/// declarations inside inline blocks.
fn declared_module_lines(mod_rs: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut brace_depth: i32 = 0;
    for line in mod_rs.lines() {
        let t = line.trim();
        if brace_depth == 0
            && let Some(name) = module_name_from_line(line)
        {
            out.push(name);
        }
        let opens = i32::try_from(t.matches('{').count()).unwrap_or(0);
        let closes = i32::try_from(t.matches('}').count()).unwrap_or(0);
        brace_depth = (brace_depth + opens - closes).max(0);
    }
    out
}

/// The names a `mod.rs` declares more than once (each reported once).
fn duplicate_declarations(mod_rs: &str) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut dups = BTreeSet::new();
    for name in declared_module_lines(mod_rs) {
        if !seen.insert(name.clone()) {
            dups.insert(name);
        }
    }
    dups
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
        imports_unsafe_submodule: false,
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

/// The raw text of the emitted `ipe_runtime/mod.rs` for one flag mask — the
/// literal file, before any dedup, so a doubled `pub mod` line is observable.
#[allow(clippy::expect_used)] // test scaffolding: a body-free emit cannot fail
fn mod_rs_for_mask(interner: &Interner, main: ipe_intern::Symbol, mask: u32) -> String {
    let prog = Program {
        imports_unsafe_submodule: false,
        modules: vec![module_for_mask(main, mask)],
    };
    let emitted = RustBackend::new(interner)
        .emit(&prog)
        .expect("emit must succeed for a body-free program");
    emitted
        .files
        .get("src/ipe_runtime/mod.rs")
        .expect("emitted project must contain ipe_runtime/mod.rs")
        .clone()
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

// ── no duplicate `pub mod` declaration in the emitted mod.rs ─────────────────

/// No emitted `ipe_runtime/mod.rs` may declare the same top-level module twice.
/// The emitter builds the file from a base list plus per-flag appends; if a
/// module sits in BOTH the base and a gated append, enabling that flag emits
/// `pub mod X;` twice and the emitted crate fails `cargo build` with
/// `error[E0428]: the name X is defined multiple times` — while `ipe` itself
/// exits 0. That is a SEAL breach: the ground-truth failure is a downstream
/// cargo error, so this proves the same property statically, without cargo.
///
/// Checked over the featureless base, every singleton flag, the all-flags
/// union, and the deterministic full-mask sample. The declaration set is a
/// per-flag union of appends (monotone; see module docs), so a duplicate can
/// only arise from a base/append or append/append overlap — both of which a
/// singleton or the union exercises. `declared_modules`' `BTreeSet` dedups
/// silently, so this test parses the raw text via `duplicate_declarations`.
#[test]
fn emitted_mod_rs_declares_no_module_twice() {
    let mut interner = Interner::new();
    let main = interner.intern("Main").expect("intern Main");
    let all: u32 = (1u32 << FLAG_COUNT) - 1;

    let mut masks: BTreeSet<u32> = sampled_masks(SAMPLE_MASKS).into_iter().collect();
    masks.insert(0);
    masks.insert(all);
    for bit in 0..FLAG_COUNT {
        masks.insert(1u32 << bit);
    }
    for mask in masks {
        let mod_rs = mod_rs_for_mask(&interner, main, mask);
        let dups = duplicate_declarations(&mod_rs);
        assert!(
            dups.is_empty(),
            "duplicate-declaration SEAL breach: emitted `mod.rs` declares \
             {dups:?} more than once — flag mask {mask:#019b}. A module in both \
             the base list and a gated append emits `pub mod` twice; the \
             emitted crate fails `cargo build` (E0428) though `ipe` exits 0. \
             Declare it SOLELY via its gated append, or SOLELY in the base."
        );
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
/// base-floor module reaching into a gated `crypto`/`jwt`/`compression`
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
        imports_unsafe_submodule: false,
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

/// Guard the `ct_eq` closure edge: `crypto_core` and `secret` both
/// `use crate::ct_eq::…` unconditionally, so every emitted `mod.rs` that
/// declares either of those modules MUST also declare `ct_eq`. Both modules
/// live in the base floor (mask 0), so this is a base-floor edge rather than a
/// per-flag edge — the base-floor test [`base_modules_do_not_reach_gated_modules`]
/// already covers it structurally, but this named witness makes the exact defect
/// class (SEAL: missing `ct_eq` in the emitted module set) fail loudly.
#[test]
fn base_declares_ct_eq_with_crypto_core_and_secret() {
    let mut interner = Interner::new();
    let main = interner.intern("Main").expect("intern Main");
    // The featureless base (mask 0) emits `crypto_core` and `secret` from the
    // base template; `ct_eq` must accompany both.
    let declared = declared_for_mask(&interner, main, 0);
    assert!(
        declared.contains("ct_eq"),
        "base floor must declare `ct_eq` — `crypto_core` and `secret` use \
         `crate::ct_eq::…` unconditionally; without `ct_eq` the emitted crate \
         fails `cargo build` (E0433). Declared: {declared:?}"
    );
    assert!(
        declared.contains("crypto_core"),
        "base floor must declare `crypto_core`; got {declared:?}"
    );
    assert!(
        declared.contains("secret"),
        "base floor must declare `secret`; got {declared:?}"
    );
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
        imports_unsafe_submodule: false,
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
        imports_unsafe_submodule: false,
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

// ── wasm vendored module-set closure ─────────────────────────────────────────

/// The unconditional crate-root deps of a top-level module in the WASM
/// vendored module set. Differs from [`unconditional_crate_deps`] in how it
/// handles inline module declarations such as `pub mod web { pub mod route; }`:
/// an inline block compiles ONLY its explicitly-declared children (here just
/// `web/route.rs`), not the whole `web/` directory (which includes
/// `web/mod.rs` with server-only deps). For a regular `pub mod foo;` module
/// the two functions are equivalent.
///
/// `inline_children`: a pre-parsed map from module name → its explicitly-listed
/// children (populated from `pub mod X { pub mod Y; … }` blocks in
/// `WASM_RUNTIME_MOD_RS`).
fn wasm_unconditional_crate_deps(
    runtime_root: &Path,
    name: &str,
    inline_children: &HashMap<String, Vec<String>>,
) -> BTreeSet<String> {
    let mut deps = BTreeSet::new();
    if let Some(children) = inline_children.get(name) {
        // Inline block: only the declared child files are compiled.
        for child in children {
            let child_file = runtime_root.join(name).join(format!("{child}.rs"));
            if child_file.is_file() {
                // Child is 1 level inside `crate::<name>`, so 2 supers to root.
                scan_file_deps(&child_file, 2, &mut deps);
            }
        }
    } else {
        // Regular `pub mod name;`: use the shared scanner.
        let flat = runtime_root.join(format!("{name}.rs"));
        let dir = runtime_root.join(name);
        if flat.is_file() {
            scan_file_deps(&flat, 1, &mut deps);
        }
        if dir.is_dir() {
            collect_dir_deps(&dir, 1, &mut deps);
        }
    }
    deps
}

/// Parse the inline-block children from a `WASM_RUNTIME_MOD_RS`-style string.
/// Returns a map from top-level inline module name → list of declared children.
/// Example: `pub mod web { pub mod route; }` → `{"web": ["route"]}`.
fn parse_inline_modules(mod_rs: &str) -> HashMap<String, Vec<String>> {
    let mut result: HashMap<String, Vec<String>> = HashMap::new();
    let mut in_block: Option<String> = None;
    let mut brace_depth: i32 = 0;
    for line in mod_rs.lines() {
        let t = line.trim();
        if let Some(ref parent) = in_block.clone() {
            let opens = i32::try_from(t.matches('{').count()).unwrap_or(0);
            let closes = i32::try_from(t.matches('}').count()).unwrap_or(0);
            brace_depth += opens - closes;
            if brace_depth <= 0 {
                in_block = None;
                brace_depth = 0;
            } else {
                // Parse `pub mod child;` inside the block.
                let rest = t
                    .strip_prefix("pub mod ")
                    .or_else(|| t.strip_prefix("mod "));
                if let Some(rest) = rest
                    && let Some(child) = rest.strip_suffix(';')
                    && child.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                {
                    result
                        .entry(parent.clone())
                        .or_default()
                        .push(child.to_owned());
                }
            }
            continue;
        }
        // Check for an inline-block opener: `pub mod name {`.
        let rest = t
            .strip_prefix("pub mod ")
            .or_else(|| t.strip_prefix("mod "));
        if let Some(rest) = rest {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                let after = rest.get(name.len()..).unwrap_or("").trim_start();
                if after.starts_with('{') {
                    in_block = Some(name.clone());
                    brace_depth = 1;
                    result.entry(name).or_default();
                }
            }
        }
    }
    result
}

/// The wasm vendored `WASM_RUNTIME_MOD_RS` module set is closed under the
/// unconditional `use crate::<dep>` references of every module it declares.
///
/// The vendored path emits `src/ipe_runtime/mod.rs` from a STATIC constant
/// (`WASM_RUNTIME_MOD_RS`) rather than a per-flag append list, so a module that
/// calls `crate::<dep>::…` without a `#[cfg(...)]` guard can only compile if
/// `<dep>` is also declared in that constant. This test proves closure WITHOUT
/// invoking cargo — the same obligation the per-flag native tests above prove for
/// the native module set, applied to the wasm vendored constant.
///
/// The breach class is: a module `M` in `WASM_RUNTIME_MOD_RS` calls
/// `crate::<dep>::fn()` unconditionally, but `<dep>` is absent from the
/// constant. `ipe` exits 0; the emitted wasm crate fails `cargo check` (E0433).
/// A regression in the native modset closure tests would not catch this because
/// those tests emit via `RustBackend::new(interner)` (the NATIVE path, which
/// uses the native template + per-flag appends — not `WASM_RUNTIME_MOD_RS`).
///
/// Inline modules (e.g. `pub mod web { pub mod route; }`) are handled correctly:
/// only the explicitly-listed child files are scanned, not the whole directory.
#[test]
fn wasm_vendored_modset_is_closed() {
    let runtime_root =
        resolve_runtime().expect("runtime source tree (src/runtime/rust/src) must resolve");
    let mut interner = Interner::new();
    let main = interner.intern("Main").expect("intern Main");

    // Emit the wasm VENDORED mod.rs (no `with_runtime_dep`) — this path writes
    // `WASM_RUNTIME_MOD_RS` into `src/ipe_runtime/mod.rs`.
    let prog = Program {
        imports_unsafe_submodule: false,
        modules: vec![module_for_mask(main, 0)],
    };
    let emitted = RustBackend::new(&interner)
        .with_target(Target::WasmClient)
        .emit(&prog)
        .expect("wasm vendored emit must succeed for a body-free program");
    let mod_rs = emitted
        .files
        .get("src/ipe_runtime/mod.rs")
        .expect("wasm vendored emit must include src/ipe_runtime/mod.rs");
    let declared = declared_modules(mod_rs);
    let inline_children = parse_inline_modules(mod_rs);

    let mut dep_cache: HashMap<String, BTreeSet<String>> = HashMap::new();
    for m in &declared {
        let deps = dep_cache
            .entry(m.clone())
            .or_insert_with(|| wasm_unconditional_crate_deps(&runtime_root, m, &inline_children));
        for dep in deps.iter() {
            assert!(
                declared.contains(dep),
                "wasm vendored module-set SEAL breach: `WASM_RUNTIME_MOD_RS` declares \
                 `{m}` (which does `use crate::{dep}` unconditionally) but does NOT \
                 declare `{dep}`. The emitted wasm crate fails `cargo check \
                 --target wasm32-unknown-unknown` (E0433) though `ipe` exits 0. \
                 Add `pub mod {dep};` to `WASM_RUNTIME_MOD_RS` in \
                 `src/compiler/backend/rust/src/project.rs`. \
                 Declared wasm modules: {declared:?}"
            );
        }
    }
}

/// The wasm vendored module set must declare `app_config` — `log.rs` calls
/// `crate::app_config::resolve_log_level_override()` unconditionally, so the
/// absence of `app_config` from `WASM_RUNTIME_MOD_RS` breaks `cargo check
/// --target wasm32-unknown-unknown` with E0433.
///
/// A named witness alongside the structural test above: the structural test
/// catches any future drift; this named test makes the specific regression
/// fail loudly with a diagnostic that names the original defect.
#[test]
fn wasm_vendored_modset_declares_app_config() {
    let mut interner = Interner::new();
    let main = interner.intern("Main").expect("intern Main");
    let prog = Program {
        imports_unsafe_submodule: false,
        modules: vec![module_for_mask(main, 0)],
    };
    let emitted = RustBackend::new(&interner)
        .with_target(Target::WasmClient)
        .emit(&prog)
        .expect("wasm vendored emit must succeed");
    let mod_rs = emitted
        .files
        .get("src/ipe_runtime/mod.rs")
        .expect("wasm vendored emit must include src/ipe_runtime/mod.rs");
    let declared = declared_modules(mod_rs);
    assert!(
        declared.contains("app_config"),
        "`WASM_RUNTIME_MOD_RS` must declare `app_config` — `log.rs` calls \
         `crate::app_config::resolve_log_level_override()` unconditionally, so \
         its absence is E0433 in `cargo check --target wasm32-unknown-unknown`. \
         Declared: {declared:?}"
    );
    assert!(
        declared.contains("log"),
        "`WASM_RUNTIME_MOD_RS` must declare `log` (precondition for the \
         `app_config` edge); got {declared:?}"
    );
}
