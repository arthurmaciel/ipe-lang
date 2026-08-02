//! Fast static SEAL for the emitted runtime FEATURE set.
//!
//! The dependency-model emit selects a set of `ipe-runtime-rust` cargo features
//! per program (the `runtime_features` SSOT in the backend). `ipe` exiting 0
//! must imply the emitted project `cargo build`s; under the dependency model the
//! breach class is "selected feature set under which the runtime crate does not
//! compile or does not export a referenced kernel". This test proves the closure
//! WITHOUT invoking cargo, over the SAME exhaustive `uses_*` flag-mask sweep the
//! module-set SEAL uses.
//!
//! For every reachable combination of the `Module` `uses_*` flags it:
//!
//! 1. asks the real backend for the SSOT feature set (`runtime_feature_names`);
//! 2. checks every selected feature is declared in the runtime crate's
//!    `[features]` universe (`src/runtime/rust/Cargo.toml`);
//! 3. resolves the set through that `[features]` table (Cargo's transitive
//!    feature unification) and asserts the resolution is closed;
//! 4. **the obligation the module-set SEAL never had** — statically scans the
//!    EMITTED user files for every `ipe_runtime::<module>::` reference the
//!    generated prelude/main hard-codes, and asserts each referenced module is
//!    `cfg`-satisfied by the resolved feature set (walking the runtime crate's
//!    own `src/mod.rs` `#[cfg(feature = …)]` attributes — the same source-parse
//!    discipline the modset test uses on the emitted `mod.rs`).
//!
//! A "declared-but-feature-absent" drift fails at (2)/(3); a
//! "prelude-references-a-module-whose-feature-is-off" drift fails at (4). Both
//! fail-closed, exhaustively.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use ipe_backend::Backend;
use ipe_backend_rust::RustBackend;
use ipe_intern::Interner;
use ipe_ir::{ModPath, Module, Program};

/// The flag space, identical to `runtime_modset_closure.rs`: bit `i` sets
/// `uses_*` flag `i`. Keeping the two SEALs on one mask enumeration means the
/// featureset closure covers exactly the programs the module closure does.
const FLAG_COUNT: usize = 18;

/// Locate the runtime crate root (`src/runtime/rust`) — the manifest whose
/// `[features]` universe and the `src/mod.rs` whose cfg gates this test reads.
fn resolve_runtime_crate() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("IPE_RUNTIME_DIR") {
        let p = PathBuf::from(dir);
        // Accept either the crate root or the legacy `src/` tree (walk up one).
        if p.join("Cargo.toml").is_file() {
            return Some(p);
        }
        if let Some(parent) = p.parent()
            && parent.join("Cargo.toml").is_file()
        {
            return Some(parent.to_owned());
        }
    }
    let cwd = std::env::current_dir().ok()?;
    let mut here: Option<&Path> = Some(cwd.as_path());
    while let Some(dir) = here {
        let candidate = dir.join("src").join("runtime").join("rust");
        if candidate.join("Cargo.toml").is_file() {
            return Some(candidate);
        }
        here = dir.parent();
    }
    None
}

/// Build a body-free `Module` with the mask's `uses_*` flags, mirroring the
/// module-set SEAL's invariants (tui⇒ui; every gated surface forces the async
/// spine so the emitted manifest matches a real program).
#[allow(clippy::similar_names)]
fn module_for_mask(name: ipe_intern::Symbol, mask: u32) -> Module {
    let f = |i: usize| mask & (1 << i) != 0;
    let uses_tui = f(4);
    let uses_ui = f(2) || uses_tui;
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
        uses_crypto: f(15),
        uses_jwt: f(16),
        uses_url: f(17),
        uses_debug: false,
        uses_ffi: false,
        uses_async_runtime,
    }
}

// ── the runtime crate's `[features]` universe + resolution ─────────────────

/// Parse `src/runtime/rust/Cargo.toml`'s `[features]` table into
/// `feature -> [enabled tokens]`. A token is another feature name or an
/// (implicit) optional-dep feature; the `dep:`, `crate/`, and `dep?/` prefixes
/// are normalized to the bare feature-or-dep name that a `#[cfg(feature = …)]`
/// can test.
fn parse_feature_table(cargo_toml: &str) -> BTreeMap<String, Vec<String>> {
    let mut table = BTreeMap::new();
    let mut in_features = false;
    for raw in cargo_toml.lines() {
        let line = raw.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            in_features = line == "[features]";
            continue;
        }
        if !in_features {
            continue;
        }
        // `name = ["a", "b/c", "dep:d"]` — may span lines; the crate keeps each
        // feature on one line, so parse a single-line array.
        let Some((name, rhs)) = line.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let mut enabled = Vec::new();
        for tok in rhs.split(['[', ']', ',', '"']) {
            let t = tok.trim();
            if t.is_empty() {
                continue;
            }
            enabled.push(normalize_feature_token(t));
        }
        table.insert(name.to_owned(), enabled);
    }
    table
}

/// Normalize a feature-list token to the bare feature-or-dep name a
/// `#[cfg(feature = "…")]` tests: `dep:tokio` → `tokio`, `sqlx?/sqlite` →
/// `sqlite` (the sqlx sub-feature — not a crate feature, so it will simply not
/// resolve as a key, which is correct), `sqlx/postgres` → `postgres`,
/// `tokio/sync` → `sync`. A bare `json` stays `json`.
fn normalize_feature_token(tok: &str) -> String {
    let tok = tok.strip_prefix("dep:").unwrap_or(tok);
    // `crate?/feat` or `crate/feat` — the enabled thing is the sub-feature; for
    // our cfg-satisfaction purposes the crate itself is what a
    // `#[cfg(feature = "crate")]` would test, so keep the crate name.
    if let Some((crate_name, _sub)) = tok.split_once('/') {
        return crate_name.trim_end_matches('?').to_owned();
    }
    tok.to_owned()
}

/// Resolve `selected` to the full set of enabled feature-or-dep names under
/// Cargo's transitive unification (a feature enables everything on its list,
/// recursively). Non-key tokens (optional-dep implicit features like `tokio`,
/// sub-features like `sync`) are enabled leaves.
fn resolve_features(selected: &[&str], table: &BTreeMap<String, Vec<String>>) -> BTreeSet<String> {
    let mut resolved = BTreeSet::new();
    let mut stack: Vec<String> = selected.iter().map(|s| (*s).to_owned()).collect();
    while let Some(feat) = stack.pop() {
        if !resolved.insert(feat.clone()) {
            continue;
        }
        if let Some(deps) = table.get(&feat) {
            for d in deps {
                if !resolved.contains(d) {
                    stack.push(d.clone());
                }
            }
        }
    }
    resolved
}

// ── the runtime crate's `src/mod.rs` cfg gates ─────────────────────────────

/// A `#[cfg(...)]` predicate over the runtime crate's features, restricted to
/// the shapes `src/mod.rs` uses on module declarations.
#[derive(Debug, Clone)]
enum Cfg {
    /// Always compiled (no cfg).
    Always,
    /// `feature = "x"`.
    Feature(String),
    /// `all(...)`.
    All(Vec<Self>),
    /// `any(...)`.
    Any(Vec<Self>),
    /// `target_arch = "wasm32"` (or any other non-feature atom) — evaluated as
    /// its constant truth on the native target this SEAL models.
    TargetAtom(bool),
}

impl Cfg {
    /// Evaluate under the resolved native-target feature set.
    fn eval(&self, feats: &BTreeSet<String>) -> bool {
        match self {
            Self::Always => true,
            Self::Feature(f) => feats.contains(f),
            Self::All(cs) => cs.iter().all(|c| c.eval(feats)),
            Self::Any(cs) => cs.iter().any(|c| c.eval(feats)),
            Self::TargetAtom(v) => *v,
        }
    }
}

/// Parse a `#[cfg(...)]` inner-predicate string (the text between the outer
/// parentheses) into a [`Cfg`]. `target_arch = "wasm32"` is false on the native
/// target this SEAL models; any other `target_*`/`unix` atom is likewise a
/// non-feature and evaluated false (the emitted references this SEAL checks are
/// native-target references — a wasm-only module is never referenced by native
/// emitted output).
fn parse_cfg_pred(inner: &str) -> Cfg {
    let s = inner.trim();
    if let Some(rest) = s.strip_prefix("all(") {
        let body = rest.strip_suffix(')').unwrap_or(rest);
        return Cfg::All(
            split_cfg_args(body)
                .iter()
                .map(|a| parse_cfg_pred(a))
                .collect(),
        );
    }
    if let Some(rest) = s.strip_prefix("any(") {
        let body = rest.strip_suffix(')').unwrap_or(rest);
        return Cfg::Any(
            split_cfg_args(body)
                .iter()
                .map(|a| parse_cfg_pred(a))
                .collect(),
        );
    }
    if let Some(rest) = s.strip_prefix("feature") {
        // `feature = "x"`.
        if let Some(eq) = rest.trim_start().strip_prefix('=') {
            let name: String = eq.trim().trim_matches('"').to_owned();
            return Cfg::Feature(name);
        }
    }
    // `target_arch = "wasm32"` and friends: non-feature atom → native-false.
    Cfg::TargetAtom(false)
}

/// Split the comma-separated arguments of an `all(...)`/`any(...)` at the top
/// nesting level (commas inside a nested `all`/`any` stay with their group).
fn split_cfg_args(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    for ch in body.chars() {
        match ch {
            '(' => {
                depth += 1;
                cur.push(ch);
            }
            ')' => {
                depth -= 1;
                cur.push(ch);
            }
            ',' if depth == 0 => {
                if !cur.trim().is_empty() {
                    out.push(cur.trim().to_owned());
                }
                cur.clear();
            }
            _ => cur.push(ch),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_owned());
    }
    out
}

/// Walk the runtime crate's `src/mod.rs` and return `module -> gating Cfg` for
/// every top-level `pub mod X;` / `mod X;` declaration. A declaration preceded
/// by one or more `#[cfg(...)]` attribute lines carries their conjunction; an
/// un-attributed declaration is [`Cfg::Always`].
fn module_gates(mod_rs: &str) -> BTreeMap<String, Cfg> {
    let mut gates = BTreeMap::new();
    let mut pending: Vec<Cfg> = Vec::new();
    for raw in mod_rs.lines() {
        let line = raw.trim();
        // A one-line `#[cfg(...)] pub mod x;` would be parsed as neither an
        // attribute (no trailing `)]`) nor a `mod` decl (starts with `#`) and so
        // slip through UN-gated — a silent false negative. The crate keeps every
        // cfg attribute on its own line; assert that, so a future inline edit
        // trips here instead of weakening the closure check.
        assert!(
            !(line.contains("#[cfg(") && line.contains(" mod ")),
            "runtime src/mod.rs has an inline `#[cfg(...)] mod` on one line — the \
             featureset-closure gate parser needs the cfg attribute on its own \
             line above the `mod` decl: {line}"
        );
        if let Some(inner) = cfg_attr_inner(line) {
            pending.push(parse_cfg_pred(&inner));
            continue;
        }
        let decl = line
            .strip_prefix("pub mod ")
            .or_else(|| line.strip_prefix("mod "));
        if let Some(rest) = decl
            && let Some(name) = rest.strip_suffix(';')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            let gate = match pending.as_slice() {
                [] => Cfg::Always,
                [only] => only.clone(),
                _ => Cfg::All(pending.clone()),
            };
            // First declaration wins (the `pub use` lines that follow repeat the
            // same cfg but are not `mod` decls, so they are skipped anyway).
            gates.entry(name.to_owned()).or_insert(gate);
            pending.clear();
            continue;
        }
        // Any non-attribute, non-mod line (blank, comment, `pub use …`, other
        // item) breaks a pending attribute run that did not attach to a `mod`.
        if !line.is_empty() && !line.starts_with("//") {
            pending.clear();
        }
    }
    gates
}

/// If `line` is a single-line `#[cfg(...)]` attribute, return its inner
/// predicate text (between the outer parens). Multi-line cfg attributes are
/// joined by the caller before this is reached — but `src/mod.rs` keeps each on
/// its own logical run; a cfg opened on one line and closed on another is
/// reassembled by [`join_cfg_attrs`].
fn cfg_attr_inner(line: &str) -> Option<String> {
    let s = line.strip_prefix("#[cfg(")?;
    let inner = s.strip_suffix(")]")?;
    Some(inner.to_owned())
}

/// Join `src/mod.rs` so every `#[cfg(...)]` attribute is on ONE line — the
/// crate wraps the multi-line `any(feature = …, …)` gates across several source
/// lines, which [`cfg_attr_inner`] cannot parse line-by-line.
fn join_cfg_attrs(src: &str) -> String {
    let mut out = String::new();
    let mut buf = String::new();
    let mut in_attr = false;
    for line in src.lines() {
        let t = line.trim();
        if in_attr {
            buf.push(' ');
            buf.push_str(t);
            if t.ends_with(")]") {
                out.push_str(buf.trim());
                out.push('\n');
                buf.clear();
                in_attr = false;
            }
            continue;
        }
        if t.starts_with("#[cfg(") && !t.ends_with(")]") {
            in_attr = true;
            buf.push_str(t);
            continue;
        }
        out.push_str(t);
        out.push('\n');
    }
    if !buf.is_empty() {
        out.push_str(buf.trim());
        out.push('\n');
    }
    out
}

// ── emitted-output scan: every `ipe_runtime::<mod>::` hard reference ────────

/// Extract every top-level runtime module named by an `ipe_runtime::<mod>::`
/// path anywhere in an emitted file (the prelude/main hard-references kernels
/// module-qualified). The first path segment after `ipe_runtime::` is the
/// module; a trailing `::` distinguishes a module reach from a bare re-exported
/// item.
fn referenced_runtime_modules(emitted: &str) -> BTreeSet<String> {
    let mut mods = BTreeSet::new();
    let needle = "ipe_runtime::";
    let mut i = 0;
    while let Some(pos) = emitted[i..].find(needle) {
        let start = i + pos + needle.len();
        let seg: String = emitted[start..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        i = start;
        if seg.is_empty() {
            continue;
        }
        // Require a module reach (`ipe_runtime::mod::…`), not a bare item
        // (`ipe_runtime::IpeMaybe`) re-exported at the crate root by a glob.
        let after = start + seg.len();
        if emitted.get(after..after.saturating_add(2)) == Some("::") {
            mods.insert(seg);
        }
    }
    mods
}

// A bare `ipe_runtime::foo` path may name a module that `mod.rs` declares
// unconditionally with a `pub use foo::*` glob (so the item is ALSO reachable at
// the root). The closure check only fails on a reference to a module whose gate
// is UNSATISFIED — an always-on module (gate `Always`) trivially passes.
#[test]
fn emitted_featureset_covers_every_prelude_reference() {
    let crate_root =
        resolve_runtime_crate().expect("runtime crate root (src/runtime/rust) must resolve");
    let cargo_toml =
        std::fs::read_to_string(crate_root.join("Cargo.toml")).expect("read runtime Cargo.toml");
    let table = parse_feature_table(&cargo_toml);
    assert!(
        !table.is_empty(),
        "runtime crate [features] table parsed empty"
    );

    let mod_rs_raw =
        std::fs::read_to_string(crate_root.join("src").join("mod.rs")).expect("read src/mod.rs");
    let gates = module_gates(&join_cfg_attrs(&mod_rs_raw));
    assert!(
        !gates.is_empty(),
        "runtime src/mod.rs module gates parsed empty"
    );

    let mut interner = Interner::new();
    let main = interner.intern("Main").expect("intern Main");

    for mask in 0u32..(1u32 << FLAG_COUNT) {
        let module = module_for_mask(main, mask);
        let prog = Program {
            modules: vec![module],
        };
        let backend = RustBackend::new(&interner);

        // (1) the SSOT feature set.
        let selected = backend
            .runtime_feature_names(&prog)
            .expect("runtime_feature_names must succeed for a body-free program");

        // (2) every selected feature is in the crate's declared universe.
        for feat in &selected {
            assert!(
                table.contains_key(*feat),
                "featureset SEAL breach: selected feature `{feat}` is NOT declared \
                 in the runtime crate's [features] universe — flag mask {mask:#019b}. \
                 Declared: {:?}",
                table.keys().collect::<Vec<_>>()
            );
        }

        // (3) resolve + closure self-consistency: the resolution is a fixpoint
        // (resolving the resolved set adds nothing new).
        let resolved = resolve_features(&selected, &table);
        let reresolved = resolve_features(
            &resolved.iter().map(String::as_str).collect::<Vec<_>>(),
            &table,
        );
        assert_eq!(
            resolved, reresolved,
            "featureset SEAL breach: feature resolution is not closed under the \
             crate [features] table — flag mask {mask:#019b}"
        );

        // (4) every emitted `ipe_runtime::<mod>::` reference is cfg-satisfied.
        let emitted = backend.emit(&prog).expect("emit must succeed");
        for (path, text) in &emitted.files {
            let path = path.as_str();
            // Only the emitted USER code (main.rs / ipe_mods) hard-references
            // kernels; the vendored `ipe_runtime/*` files reference themselves
            // via `crate::`/`super::` (the module-set SEAL's domain), not the
            // `ipe_runtime::` extern path.
            if Path::new(path).extension().is_none_or(|ext| ext != "rs") {
                continue;
            }
            for m in referenced_runtime_modules(text) {
                let gate = gates.get(&m).cloned().unwrap_or(Cfg::Always);
                assert!(
                    gate.eval(&resolved),
                    "featureset SEAL breach: emitted `{path}` references \
                     `ipe_runtime::{m}::…` but the runtime crate gates `mod {m}` \
                     behind {gate:?}, UNSATISFIED by the selected feature set \
                     {selected:?} (resolved {resolved:?}) — flag mask {mask:#019b}. \
                     The emitted crate would fail `cargo build` (E0433).",
                );
            }
        }
    }
}

/// Fail-closed proof for direction (4): a hand-built feature set that DROPS a
/// required feature must make the coverage check fail. Uses the same gate/scan
/// machinery, so a regression that weakens the closure (e.g. treating an
/// unsatisfied gate as satisfied) trips here.
#[test]
fn prelude_reference_gap_fails_closed() {
    let crate_root =
        resolve_runtime_crate().expect("runtime crate root (src/runtime/rust) must resolve");
    let cargo_toml =
        std::fs::read_to_string(crate_root.join("Cargo.toml")).expect("read runtime Cargo.toml");
    let table = parse_feature_table(&cargo_toml);
    let mod_rs_raw =
        std::fs::read_to_string(crate_root.join("src").join("mod.rs")).expect("read src/mod.rs");
    let gates = module_gates(&join_cfg_attrs(&mod_rs_raw));

    let mut interner = Interner::new();
    let main = interner.intern("Main").expect("intern Main");
    // A `uses_auth` program: its emitted prelude hard-references
    // `ipe_runtime::auth::…`, and `mod auth` is gated on `feature = "jwt"`.
    let prog = Program {
        modules: vec![module_for_mask(main, 1 << 7)],
    };
    let backend = RustBackend::new(&interner);
    let emitted = backend.emit(&prog).expect("emit");

    // The correct set covers the `auth` reference; the mutated set drops `jwt`
    // (auth's gating feature).
    let full = backend.runtime_feature_names(&prog).expect("features");
    assert!(
        full.contains(&"jwt"),
        "precondition: a uses_auth program selects `jwt` (auth's gate); got {full:?}"
    );
    let mutated: Vec<&str> = full.into_iter().filter(|f| *f != "jwt").collect();
    let resolved = resolve_features(&mutated, &table);

    // Find at least one emitted `ipe_runtime::auth::…` reference and assert the
    // dropped-feature set does NOT cover it — the fail-closed direction.
    let mut saw_ref = false;
    let mut uncovered = false;
    for text in emitted.files.values() {
        for m in referenced_runtime_modules(text) {
            if m == "auth" {
                saw_ref = true;
                let gate = gates.get("auth").cloned().unwrap_or(Cfg::Always);
                if !gate.eval(&resolved) {
                    uncovered = true;
                }
            }
        }
    }
    assert!(
        saw_ref,
        "a uses_auth program must emit at least one `ipe_runtime::auth::…` reference"
    );
    assert!(
        uncovered,
        "fail-closed proof: dropping the `jwt` feature must leave the emitted \
         `ipe_runtime::auth::…` reference UNCOVERED — the closure check would have \
         a false negative otherwise"
    );
}

/// The universe check is non-trivial: the SSOT DOES select features across the
/// sweep (a green run where nothing is ever selected would be vacuous). Assert
/// the union of selected features over the sweep is a non-empty subset of the
/// declared universe and includes the dependency-bearing surfaces.
#[test]
fn ssot_selects_a_meaningful_subset_of_the_universe() {
    let crate_root =
        resolve_runtime_crate().expect("runtime crate root (src/runtime/rust) must resolve");
    let cargo_toml =
        std::fs::read_to_string(crate_root.join("Cargo.toml")).expect("read runtime Cargo.toml");
    let table = parse_feature_table(&cargo_toml);

    let mut interner = Interner::new();
    let main = interner.intern("Main").expect("intern Main");

    let mut union: BTreeSet<String> = BTreeSet::new();
    // A representative slice of the sweep (full sweep is exercised by the
    // coverage test); each single-flag mask plus the empty mask suffices to
    // union every surface feature.
    for bit in 0..FLAG_COUNT {
        let prog = Program {
            modules: vec![module_for_mask(main, 1 << bit)],
        };
        for f in RustBackend::new(&interner)
            .runtime_feature_names(&prog)
            .expect("features")
        {
            union.insert(f.to_owned());
        }
    }
    assert!(!union.is_empty(), "SSOT selected nothing across the sweep");
    for f in &union {
        assert!(
            table.contains_key(f),
            "union feature `{f}` not in the declared universe"
        );
    }
    for expect in [
        "json",
        "url",
        "http_client",
        "server",
        "web",
        "tui",
        "websocket_client",
    ] {
        assert!(
            union.contains(expect),
            "SSOT sweep never selected the dependency-bearing feature `{expect}`: {union:?}"
        );
    }
}
