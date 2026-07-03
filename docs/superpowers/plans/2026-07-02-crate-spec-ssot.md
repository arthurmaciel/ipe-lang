# Plan: Crate-version single source of truth + drift guard (#50)

## Goal

Collapse the crate **version** literals currently scattered across
`crates/sky_backend_rust/src/project.rs` (the five manifest-surgery functions —
`db_cargo_toml`, `server_cargo_toml`, `live_cargo_toml`, `tui_cargo_toml`,
`webview_cargo_toml`) into **one typed source of truth**, and add a
`crate_specs_sync`-style drift test that fails the build the moment any emitted
version diverges from `runtime/Cargo.toml` (the vendored runtime the emitted
projects compile against) or from the golden base manifest
`tests/golden/m0/Cargo.toml`.

Today `tokio = "1"` appears **four** times in `project.rs` (server ×2, tui ×2)
plus once in the golden base manifest; `sqlx "0.8"`, `axum "0.7"`,
`tower-http "0.5"`, `async-trait "0.1"`, `serde_urlencoded "0.7"`, `libc "0.2"`,
`crossterm "0.28"`, `unicode-width "0.1"`, `wry "0.55"`, `tao "0.35"` each appear
as free-standing string literals with **no** guard tying them to the versions the
runtime was actually tested against. Bump one and the others silently skew — a
generated project then vendors a runtime pinned to version X while its own
`Cargo.toml` requests version Y. This is exactly the invalid-states class the
plan closes.

## Architecture

Reference parity note (public artifact): `../sky` (branch `feat/runtime-rust`) is
the capability reference. It solves this with an embedded
`src/Sky/Generate/Rust/Builder/crate-specs.toml` that its **Haskell** emitter
parses at compile time, guarded by `runtime-rust/tests/crate_specs_sync.rs`. We
port the *invariant* (one authoritative version source + a drift tripwire), not
the *form*: ipê is Rust-all-the-way, so the source of truth is a **typed
`const` table** (`crate_specs.rs`) rather than a re-parsed TOML string. A typed
table is strictly more principled here — the versions are structured data checked
by the compiler (parse, don't validate), not text re-parsed on every read. What
differs from the reference and why: **file form** (Rust consts vs embedded TOML)
because the host language differs; **drift comparison targets** (we assert
against *both* `runtime/Cargo.toml` and the golden base manifest, because ipê's
base manifest is a static golden artifact the reference does not have).

Data flow after this change:

```
crate_specs.rs (CrateSpec const table)  ← SINGLE SOURCE
        │  crate_specs::SQLX.version, ::TOKIO.version, …
        ▼
project.rs surgery fns  ── emit ──▶  generated Cargo.toml
        ▲                                    │
        │ drift test (crate_specs_sync)      │ must match versions in
        └──────── asserts SSOT ≡ ────────────┴──▶ runtime/Cargo.toml
                                              └──▶ tests/golden/m0/Cargo.toml (tokio)
```

Feature lists and `optional = true` flags stay inline in the surgery functions
(matching the reference policy: "only the version SPEC lives in the SSOT; gating
stays in the emitter"). Only the version substring moves. Anchored `replacen`
surgery with fail-loud `CompilerBug` on anchor-miss is preserved verbatim
(audit item 16, verdict O+ — keep).

## Tech Stack

- Rust, edition 2024, workspace at repo root.
- Crate under change: `sky_backend_rust` (`crates/sky_backend_rust`), package
  name `sky_backend_rust`, workspace member.
- Diagnostics: `sky_diagnostics::{DResult, Diagnostic}` — `Diagnostic::CompilerBug { where_, detail }`.
- Test runner: `cargo test -p sky_backend_rust`.
- Lint posture (from root `Cargo.toml` `[workspace.lints.clippy]`): `unwrap_used`,
  `expect_used`, `panic`, `indexing_slicing`, `unreachable`, `pedantic`,
  `nursery` all **deny**. `clippy.toml` sets `allow-unwrap-in-tests = true` +
  `allow-expect-in-tests = true`, so test code may `unwrap`/`expect` but **must
  still avoid** `panic!` and raw `[..]` indexing (use `.get(..)`,
  `strip_prefix`, `find`, `split_once`). `pedantic`/`nursery` only fire on
  `pub` items, so keep `crate_specs` a **private** module (`mod crate_specs;`)
  to avoid `must_use_candidate` / doc-lint noise on the const accessors.

## Global Constraints

**PRINCIPLES order (strict, tie-break downward):**
security > correctness > soundness > efficiency > completeness > readability.

**Two foundational rules — applied throughout:**
1. **PARSE, DON'T VALIDATE.** The version source is structured typed data
   (`CrateSpec` consts), not a string re-parsed at each use. The drift test
   parses the two external manifests *once* into a typed map, then compares.
2. **MAKE INVALID STATES UNREPRESENTABLE.** After this change there is exactly
   one place a surgery version can be written; a manifest that requests a
   version the runtime was not built against cannot be emitted without the drift
   test going red.

**Fail-closed, never wildcard.** Every anchor-miss stays a
`Diagnostic::CompilerBug` (never a silent no-op `replacen`). The drift test
asserts equality and lists every offending crate; it never `panic!`s explicitly
and never indexes unchecked.

**Behaviour-preserving refactor.** The emitted `Cargo.toml` bytes MUST NOT
change. The existing `golden.rs` byte-equality gate and the `project.rs`
`#[cfg(test)] mod tests` (`server_toml_non_db_inserts_server`,
`server_toml_db_compose_inserts_both`) are the regression net — they must stay
green with zero edits to their assertions.

**Parallel-safety / file overlap:**
- This task touches only `crates/sky_backend_rust/src/{crate_specs.rs (new),
  project.rs, lib.rs}` plus `crates/sky_backend_rust/tests/` is **not** used
  (drift test is co-located in `crate_specs.rs`). It is **disjoint** from the
  in-flight registry migration (`sky_canon`/`sky_types`/`sky_kernels`/
  `sky_lower` — `constrain.rs`, `lower.rs` callee) — zero shared files.
- vs **#49 TCO** (adds 2 `sky_ir` variants, edits `sky_lower/lower.rs` and
  `sky_backend_rust/src/emit_expr.rs`): the only possible shared file is
  `crates/sky_backend_rust/src/lib.rs`, where this task adds a single
  `mod crate_specs;` line. That is a one-line, non-overlapping addition
  (different region from any `emit_expr`/IR wiring) — trivially mergeable.
  `project.rs`, `emit_expr.rs`, and `sky_ir` are otherwise untouched by this
  task. Flag at merge: coordinate the single `lib.rs` `mod` line if both land
  together.

---

## Task 1 — Introduce the `CrateSpec` typed SSOT table

**Files:**
- `crates/sky_backend_rust/src/crate_specs.rs` (new)
- `crates/sky_backend_rust/src/lib.rs` (add `mod crate_specs;`)

**Interfaces**

Consumes: nothing (leaf data module).

Produces:
```rust
/// One authoritative crate version. Feature lists + `optional` flags stay in
/// the emitter (project.rs); only the version SPEC lives here.
pub(crate) struct CrateSpec {
    pub(crate) name: &'static str,
    pub(crate) version: &'static str,
}

// One `pub(crate) const` per crate whose version is emitted by project.rs surgery:
pub(crate) const TOKIO: CrateSpec;            // "1"
pub(crate) const SQLX: CrateSpec;             // "0.8"
pub(crate) const AXUM: CrateSpec;             // "0.7"
pub(crate) const TOWER_HTTP: CrateSpec;       // "0.5"
pub(crate) const ASYNC_TRAIT: CrateSpec;      // "0.1"
pub(crate) const SERDE_URLENCODED: CrateSpec; // "0.7"
pub(crate) const LIBC: CrateSpec;             // "0.2"
pub(crate) const CROSSTERM: CrateSpec;        // "0.28"
pub(crate) const UNICODE_WIDTH: CrateSpec;    // "0.1"
pub(crate) const WRY: CrateSpec;              // "0.55"
pub(crate) const TAO: CrateSpec;              // "0.35"

/// Every spec, for drift-test iteration.
pub(crate) const ALL: &[CrateSpec];
```

Version values are re-verified against HEAD:
`project.rs:388` sqlx `0.8`; `:431/:432` tokio `1`; `:434-435` axum `0.7` +
tower-http `0.5`; `:537` async-trait `0.1` + serde_urlencoded `0.7` + libc `0.2`;
`:616` crossterm `0.28` + unicode-width `0.1`; `:719` wry `0.55` + tao `0.35`.

### Steps

1. **Write failing test.** Create `crates/sky_backend_rust/src/crate_specs.rs`
   with the data and a co-located unit test that pins the table shape:

```rust
//! Single source of truth for the crate VERSIONS the Rust codegen emits into a
//! generated project's `Cargo.toml`. Edit a version HERE; `project.rs`'s
//! manifest-surgery functions read it, so a version can never drift between the
//! emitter, `runtime/Cargo.toml`, and `tests/golden/m0/Cargo.toml`.
//!
//! Feature lists + `optional` flags stay inline in `project.rs` (they depend on
//! usage). Only the version SPEC lives here. The `crate_specs_sync` test below
//! is the drift tripwire.

/// One authoritative crate version.
pub(crate) struct CrateSpec {
    pub(crate) name: &'static str,
    pub(crate) version: &'static str,
}

pub(crate) const TOKIO: CrateSpec = CrateSpec { name: "tokio", version: "1" };
pub(crate) const SQLX: CrateSpec = CrateSpec { name: "sqlx", version: "0.8" };
pub(crate) const AXUM: CrateSpec = CrateSpec { name: "axum", version: "0.7" };
pub(crate) const TOWER_HTTP: CrateSpec =
    CrateSpec { name: "tower-http", version: "0.5" };
pub(crate) const ASYNC_TRAIT: CrateSpec =
    CrateSpec { name: "async-trait", version: "0.1" };
pub(crate) const SERDE_URLENCODED: CrateSpec =
    CrateSpec { name: "serde_urlencoded", version: "0.7" };
pub(crate) const LIBC: CrateSpec = CrateSpec { name: "libc", version: "0.2" };
pub(crate) const CROSSTERM: CrateSpec =
    CrateSpec { name: "crossterm", version: "0.28" };
pub(crate) const UNICODE_WIDTH: CrateSpec =
    CrateSpec { name: "unicode-width", version: "0.1" };
pub(crate) const WRY: CrateSpec = CrateSpec { name: "wry", version: "0.55" };
pub(crate) const TAO: CrateSpec = CrateSpec { name: "tao", version: "0.35" };

/// Every spec emitted by the surgery functions, for drift-test iteration.
pub(crate) const ALL: &[CrateSpec] = &[
    TOKIO, SQLX, AXUM, TOWER_HTTP, ASYNC_TRAIT, SERDE_URLENCODED, LIBC,
    CROSSTERM, UNICODE_WIDTH, WRY, TAO,
];

#[cfg(test)]
mod tests {
    use super::ALL;

    /// The table is non-empty and every entry has a non-empty name + version.
    /// (The real drift guard against the manifests lands in Task 2, appended to
    /// this same `mod tests`.)
    #[test]
    fn table_is_well_formed() {
        assert!(!ALL.is_empty(), "crate spec table must not be empty");
        for spec in ALL {
            assert!(!spec.name.is_empty(), "empty crate name in ALL");
            assert!(!spec.version.is_empty(), "empty version for {}", spec.name);
        }
        assert_eq!(ALL.len(), 11, "expected 11 surgery-emitted crate specs");
    }
}
```

   Wire the module in `crates/sky_backend_rust/src/lib.rs` next to the other
   private module declarations (HEAD `lib.rs:25` reads `mod project;`):

```rust
mod crate_specs;
mod project;
```

2. **Run it — expect fail (does not compile yet before wiring, or asserts once
   compiled).** Until `mod crate_specs;` is added, the file is orphaned and the
   test is not collected:

```
cargo test -p sky_backend_rust crate_specs
```
   Expected before `lib.rs` edit: `0 tests run` for the filter (module not
   compiled). After adding `mod crate_specs;`, re-run.

3. **Minimal impl:** the code above IS the minimal impl. Add the `mod` line.

4. **Run — expect pass:**
```
cargo test -p sky_backend_rust crate_specs
```
   Expected tail:
```
running 1 test
test crate_specs::tests::table_is_well_formed ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; ...
```

5. **Commit:** `git commit -am "sky_backend_rust: add CrateSpec version SSOT table (#50)"`

---

## Task 2 — Drift guard: SSOT ≡ runtime/Cargo.toml (+ golden tokio)

**Files:**
- `crates/sky_backend_rust/src/crate_specs.rs` (extend the `#[cfg(test)] mod tests`)

**Interfaces**

Consumes: `crate_specs::ALL`; on-disk `runtime/Cargo.toml` and
`tests/golden/m0/Cargo.toml`, located via `env!("CARGO_MANIFEST_DIR")` =
`crates/sky_backend_rust`. Verified relative paths from there:
`../../runtime/Cargo.toml` and `../../tests/golden/m0/Cargo.toml` (both confirmed
to resolve at HEAD).

Produces (test-only helpers, inside `mod tests`):
```rust
fn version_of(value: &str) -> Option<String>;               // bare "X" or { version = "X", … }
fn parse_deps(text: &str, only_dep_sections: bool)
    -> std::collections::BTreeMap<String, String>;          // name → version
#[test] fn crate_specs_match_manifests();
```

Notes on the manifests, re-verified against HEAD:
- `runtime/Cargo.toml` contains all 11 SSOT crates. `libc = "0.2"` lives under
  `[target.'cfg(unix)'.dependencies]` — `parse_deps` treats any section header
  containing `"dependencies"` as a dep section, so the target table is included
  (matching the reference parser). `tokio` also appears in `[dev-dependencies]`
  at the same `"1"`; `parse_deps` uses first-insert-wins and `[dependencies]`
  precedes `[dev-dependencies]`, so `[dependencies]`' `tokio = "1"` is captured.
- `tests/golden/m0/Cargo.toml` base manifest contains `tokio = "1"` (the only
  SSOT crate present in the base; sqlx/axum/wry/… are added by surgery, not in
  the base). The golden check is scoped to crates that actually appear there.

### Steps

1. **Write failing test.** Append to `crate_specs.rs`'s `mod tests` the parser +
   drift assertion. To force a red first, temporarily set `SQLX`'s version to a
   wrong value (`"0.0"`) at the top of the file, run, observe the drift failure,
   then restore `"0.8"` for green. The helpers (lint-clean — no `panic!`, no raw
   indexing):

```rust
    use super::ALL;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    /// Extract the version from a Cargo dependency value: `"0.4"` or
    /// `{ version = "0.4", ... }`.
    fn version_of(value: &str) -> Option<String> {
        let v = value.trim();
        if let Some(rest) = v.strip_prefix('{') {
            let idx = rest.find("version")?;
            let after = rest.get(idx + "version".len()..)?.trim_start();
            let after = after.strip_prefix('=')?.trim_start();
            let after = after.strip_prefix('"')?;
            let end = after.find('"')?;
            after.get(..end).map(str::to_owned)
        } else if let Some(rest) = v.strip_prefix('"') {
            let end = rest.find('"')?;
            rest.get(..end).map(str::to_owned)
        } else {
            None
        }
    }

    /// Parse `name = <value>` dependency lines into name → version. When
    /// `only_dep_sections` is true, only lines under a `[...dependencies]`
    /// header are considered (skips `[features]`, `[profile.*]`, …).
    fn parse_deps(text: &str, only_dep_sections: bool) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        let mut in_deps = !only_dep_sections;
        for raw in text.lines() {
            let line = raw.trim();
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            if line.starts_with('[') {
                in_deps = line.contains("dependencies");
                continue;
            }
            if !in_deps {
                continue;
            }
            if let Some((name, value)) = line.split_once('=') {
                let name = name.trim();
                if name.is_empty() || name.contains(' ') || name.contains('"') {
                    continue;
                }
                if let Some(ver) = version_of(value) {
                    out.entry(name.to_owned()).or_insert(ver);
                }
            }
        }
        out
    }

    /// The SSOT versions MUST match `runtime/Cargo.toml` for every crate, and
    /// the golden base manifest for every SSOT crate it declares (tokio). Bump
    /// a version in ONE place and this fails until the others are updated.
    #[test]
    fn crate_specs_match_manifests() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let runtime_path = manifest.join("../../runtime/Cargo.toml");
        let golden_path = manifest.join("../../tests/golden/m0/Cargo.toml");

        let runtime_txt = std::fs::read_to_string(&runtime_path)
            .unwrap_or_else(|e| panic!("read {runtime_path:?}: {e}"));
        let golden_txt = std::fs::read_to_string(&golden_path)
            .unwrap_or_else(|e| panic!("read {golden_path:?}: {e}"));

        let runtime = parse_deps(&runtime_txt, true);
        let golden = parse_deps(&golden_txt, true);

        let mut problems = Vec::new();
        for spec in ALL {
            match runtime.get(spec.name) {
                None => problems.push(format!(
                    "{}: in SSOT ({}) but absent from runtime/Cargo.toml",
                    spec.name, spec.version
                )),
                Some(rt_ver) if rt_ver != spec.version => problems.push(format!(
                    "{}: SSOT = {}, runtime/Cargo.toml = {rt_ver}",
                    spec.name, spec.version
                )),
                Some(_) => {}
            }
            // Golden check only where the base manifest declares the crate.
            if let Some(g_ver) = golden.get(spec.name) {
                if g_ver != spec.version {
                    problems.push(format!(
                        "{}: SSOT = {}, tests/golden/m0/Cargo.toml = {g_ver}",
                        spec.name, spec.version
                    ));
                }
            }
        }
        assert!(
            problems.is_empty(),
            "crate-version drift between the SSOT and the manifests:\n  {}",
            problems.join("\n  ")
        );
    }
```

   > Lint note: the two `unwrap_or_else(|e| panic!(...))` are permitted because
   > `allow-expect-in-tests`/`allow-unwrap-in-tests` cover the `unwrap*` family
   > in test cfg; if the `panic` deny still fires on the explicit `panic!`,
   > replace with `.expect(&format!("read {runtime_path:?}"))` (expect is
   > test-allowed). Prefer `expect` if the first `cargo test` reports the
   > `clippy::panic` deny — verify from the actual first run, do not assume.

2. **Run — expect fail (with the temporary `SQLX = "0.0"`):**
```
cargo test -p sky_backend_rust crate_specs_match_manifests
```
   Expected:
```
---- crate_specs::tests::crate_specs_match_manifests stdout ----
thread '...' panicked at ...:
crate-version drift between the SSOT and the manifests:
  sqlx: SSOT = 0.0, runtime/Cargo.toml = 0.8
test result: FAILED. 0 passed; 1 failed; ...
```

3. **Minimal impl:** restore `SQLX`'s version to `"0.8"`. No production code
   needed — the SSOT already matches the manifests (this task only adds the
   guard).

4. **Run — expect pass:**
```
cargo test -p sky_backend_rust crate_specs
```
   Expected:
```
test crate_specs::tests::table_is_well_formed ... ok
test crate_specs::tests::crate_specs_match_manifests ... ok

test result: ok. 2 passed; 0 failed; ...
```
   Also run clippy to confirm lint-clean:
```
cargo clippy -p sky_backend_rust --tests 2>&1 | rg -i 'panic|indexing|error' || echo CLEAN
```
   Expected: `CLEAN`.

5. **Commit:** `git commit -am "sky_backend_rust: crate_specs_sync drift guard vs runtime + golden manifests (#50)"`

---

## Task 3 — Rewire the surgery functions to read the SSOT

**Files:**
- `crates/sky_backend_rust/src/project.rs` (functions `db_cargo_toml`,
  `server_cargo_toml`, `live_cargo_toml`, `tui_cargo_toml`, `webview_cargo_toml`)

**Interfaces**

Consumes: `crate::crate_specs::{SQLX, TOKIO, AXUM, TOWER_HTTP, ASYNC_TRAIT,
SERDE_URLENCODED, LIBC, CROSSTERM, UNICODE_WIDTH, WRY, TAO}`.

Produces: byte-identical `Cargo.toml` output (regression-gated by `golden.rs`
and the existing `project.rs` unit tests). Signatures unchanged:
`db_cargo_toml() -> DResult<String>`, `server_cargo_toml(base: &str) -> DResult<String>`,
`live_cargo_toml`, `tui_cargo_toml`, `webview_cargo_toml` — all `(&str) -> DResult<String>`.

**Mechanics.** Version-bearing `const &str` literals become `let` bindings built
with `format!`, interpolating `crate_specs::*.version`. Feature lists and
`optional = true` stay inline. Anchor strings that embed a version (the tokio
`replacen` anchors) are likewise derived so the anchor and the golden base
manifest cannot skew independently. Add the import at the top of `project.rs`
(near `use crate::EmitCtx;`, HEAD `project.rs:23`):

```rust
use crate::crate_specs;
```

### Steps

1. **Baseline green (regression net first).** Before editing, confirm the
   byte-equality gates pass so any post-edit diff is attributable:
```
cargo test -p sky_backend_rust golden
cargo test -p sky_backend_rust --lib project::tests
```
   Expected: `golden ... ok` and both `server_toml_*` tests `ok`.

2. **`db_cargo_toml` — sqlx.** Replace the `const SQLX_LINE` (HEAD
   `project.rs:387-388`) with a `format!` built from the SSOT. Note the tokio
   const in this fn is unrelated (no tokio here). Before:
```rust
    const SQLX_LINE: &str =
        "sqlx = { version = \"0.8\", features = [\"runtime-tokio-rustls\", \"sqlite\"] }\n\n";
```
   After (move out of `const`, place before first `let`; keep the other
   `const` anchors as-is):
```rust
    let sqlx_line = format!(
        "sqlx = {{ version = \"{}\", features = [\"runtime-tokio-rustls\", \"sqlite\"] }}\n\n",
        crate_specs::SQLX.version
    );
```
   and update the two `result.push_str(SQLX_LINE)` / capacity references to
   `sqlx_line` (`&sqlx_line`, `sqlx_line.len()`).

3. **`server_cargo_toml` — tokio ×2, axum, tower-http.** HEAD `project.rs:430-435`.
   Convert the four version-bearing consts to `format!` `let`s (declared before
   the first non-const `let`; the pure anchors `DEFAULT_PREFIX`, `DB_FEATURE`,
   `PROFILE_ANCHOR` stay `const`). Before:
```rust
    const TOKIO_TIME: &str =
        r#"tokio = { version = "1", features = ["rt", "rt-multi-thread", "macros", "time"] }"#;
    const TOKIO_NET_SYNC: &str = r#"tokio = { version = "1", features = ["rt", "rt-multi-thread", "macros", "time", "net", "sync"] }"#;
    const SERVER_DEPS: &str = "axum = { version = \"0.7\", features = [\"ws\"] }\n\
         tower-http = { version = \"0.5\", features = [\"fs\", \"catch-panic\"] }\n\n";
```
   After:
```rust
    let tokio_time = format!(
        "tokio = {{ version = \"{}\", features = [\"rt\", \"rt-multi-thread\", \"macros\", \"time\"] }}",
        crate_specs::TOKIO.version
    );
    let tokio_net_sync = format!(
        "tokio = {{ version = \"{}\", features = [\"rt\", \"rt-multi-thread\", \"macros\", \"time\", \"net\", \"sync\"] }}",
        crate_specs::TOKIO.version
    );
    let server_deps = format!(
        "axum = {{ version = \"{}\", features = [\"ws\"] }}\n\
         tower-http = {{ version = \"{}\", features = [\"fs\", \"catch-panic\"] }}\n\n",
        crate_specs::AXUM.version,
        crate_specs::TOWER_HTTP.version
    );
```
   Update the `replacen(TOKIO_TIME, TOKIO_NET_SYNC, 1)` call and the
   `SERVER_DEPS` push/capacity to the `let` names (`&tokio_time`,
   `&tokio_net_sync`, `&server_deps`). The `format!` reproduces the exact prior
   bytes (the `{{`/`}}` escape the literal braces; version `"1"`/`"0.7"`/`"0.5"`
   interpolate identically).

4. **`live_cargo_toml` — async-trait, serde_urlencoded, libc.** HEAD
   `project.rs:537`. The `TOKIO_*_FEATURES` anchors here are feature-only (no
   version) — leave them `const`. Before:
```rust
    const LIVE_DEPS: &str = "async-trait = \"0.1\"\nserde_urlencoded = \"0.7\"\nlibc = \"0.2\"\n\n";
```
   After:
```rust
    let live_deps = format!(
        "async-trait = \"{}\"\nserde_urlencoded = \"{}\"\nlibc = \"{}\"\n\n",
        crate_specs::ASYNC_TRAIT.version,
        crate_specs::SERDE_URLENCODED.version,
        crate_specs::LIBC.version
    );
```
   Update the `LIVE_DEPS` push/capacity references to `live_deps`.

5. **`tui_cargo_toml` — tokio ×2, crossterm, unicode-width.** HEAD
   `project.rs:616,627-629`. Before:
```rust
    const TUI_DEPS: &str = "crossterm = \"0.28\"\nunicode-width = \"0.1\"\n\n";
    const TOKIO_TIME_ONLY: &str =
        r#"tokio = { version = "1", features = ["rt", "rt-multi-thread", "macros", "time"] }"#;
    const TOKIO_TIME_SYNC: &str = r#"tokio = { version = "1", features = ["rt", "rt-multi-thread", "macros", "time", "sync"] }"#;
```
   After:
```rust
    let tui_deps = format!(
        "crossterm = \"{}\"\nunicode-width = \"{}\"\n\n",
        crate_specs::CROSSTERM.version,
        crate_specs::UNICODE_WIDTH.version
    );
    let tokio_time_only = format!(
        "tokio = {{ version = \"{}\", features = [\"rt\", \"rt-multi-thread\", \"macros\", \"time\"] }}",
        crate_specs::TOKIO.version
    );
    let tokio_time_sync = format!(
        "tokio = {{ version = \"{}\", features = [\"rt\", \"rt-multi-thread\", \"macros\", \"time\", \"sync\"] }}",
        crate_specs::TOKIO.version
    );
```
   Update the `replacen(TOKIO_TIME_ONLY, TOKIO_TIME_SYNC, 1)` and the
   `TUI_DEPS` push/capacity to the `let` names.

6. **`webview_cargo_toml` — wry, tao.** HEAD `project.rs:719`. Before:
```rust
    const WEBVIEW_NATIVE_DEPS: &str = "wry = { version = \"0.55\", optional = true }\ntao = { version = \"0.35\", optional = true }\n\n";
```
   After:
```rust
    let webview_native_deps = format!(
        "wry = {{ version = \"{}\", optional = true }}\ntao = {{ version = \"{}\", optional = true }}\n\n",
        crate_specs::WRY.version,
        crate_specs::TAO.version
    );
```
   Update the `WEBVIEW_NATIVE_DEPS` push/capacity to `webview_native_deps`.

   > Note the `WEBVIEW_EMPTY`/`WEBVIEW_WITH_DEPS` feature-wiring anchors carry
   > no version — leave them `const`.

7. **Run — expect pass (byte-identical output).** The existing goldens are the
   proof the refactor changed nothing:
```
cargo test -p sky_backend_rust
```
   Expected: `golden ... ok`, `server_toml_non_db_inserts_server ... ok`,
   `server_toml_db_compose_inserts_both ... ok`, both `crate_specs::tests::*
   ... ok`, and overall `test result: ok.` with the same pre-edit pass count
   plus the 2 new crate_specs tests.

8. **Lint-clean check** (the moved `let`s must not trip `items_after_statements`
   — `format!` `let`s go *after* the remaining `const`s, before other `let`s):
```
cargo clippy -p sky_backend_rust --all-targets 2>&1 | rg -i 'error|warning' || echo CLEAN
```
   Expected: `CLEAN`.

9. **Commit:** `git commit -am "sky_backend_rust: read crate versions from SSOT in manifest surgery (#50)"`

---

## Task 4 — Negative proof + docs sync

**Files:**
- `crates/sky_backend_rust/src/crate_specs.rs` (one more assertion)
- `docs/architecture/sky-rust-backend-reference-audit.md` (flip item 15 status)

**Interfaces**

Consumes: `crate_specs::ALL`, the emitted output of the surgery functions.

Produces: a test asserting each surgery function's OUTPUT contains its SSOT
version string (closes the loop: SSOT ↔ emitted, not just SSOT ↔ manifests);
an audit-doc line recording the change without dev-history narration.

### Steps

1. **Write failing test.** Because `db_cargo_toml`/`server_cargo_toml` are
   `pub(super)`-visible to the co-located `mod tests` (same crate), assert the
   emitted manifest carries the SSOT versions. Add to `crate_specs.rs` `mod
   tests` (import the fns via `crate::project`):

```rust
    /// The emitted manifests must carry the SSOT versions — proves the surgery
    /// reads the table, not a stale literal.
    #[test]
    fn emitted_manifests_use_ssot_versions() {
        let db = crate::project::db_cargo_toml().expect("db_cargo_toml");
        assert!(
            db.contains(&format!("sqlx = {{ version = \"{}\"", super::SQLX.version)),
            "db manifest must emit SSOT sqlx version:\n{db}"
        );
        let srv = crate::project::server_cargo_toml(crate::project::cargo_toml_base())
            .expect("server_cargo_toml");
        assert!(
            srv.contains(&format!("version = \"{}\", features = [\"ws\"]", super::AXUM.version)),
            "server manifest must emit SSOT axum version:\n{srv}"
        );
    }
```

   > **Visibility check to do at implementation time:** `db_cargo_toml` and
   > `server_cargo_toml` are currently private `fn` in `project.rs`, and
   > `CARGO_TOML` is a private `const` there. The existing `project.rs` unit
   > tests reach them via `use super::{...}` because they are *in the same
   > module*. A test in `crate_specs.rs` is a *different* module, so it needs
   > `pub(crate)` on `db_cargo_toml`, `server_cargo_toml`, and a
   > `pub(crate) fn cargo_toml_base() -> &'static str { CARGO_TOML }` accessor
   > in `project.rs` (do NOT make `CARGO_TOML` itself `pub` — expose a reader).
   > If widening visibility is undesirable, the simpler equivalent is to keep
   > this assertion INSIDE `project.rs`'s existing `mod tests` (where the fns
   > are already reachable) and import `crate::crate_specs`. **Prefer the
   > in-`project.rs` placement** — it needs zero visibility changes. Choose it
   > unless there is a reason not to.

   In-`project.rs` variant (preferred — append to `project.rs:840 mod tests`,
   adding `use crate::crate_specs;`):
```rust
    #[test]
    fn emitted_manifests_use_ssot_versions() {
        let db = db_cargo_toml().expect("db_cargo_toml");
        assert!(
            db.contains(&format!("sqlx = {{ version = \"{}\"", crate_specs::SQLX.version)),
            "db manifest must emit SSOT sqlx version:\n{db}"
        );
        let srv = server_cargo_toml(CARGO_TOML).expect("server_cargo_toml");
        assert!(
            srv.contains(&format!("version = \"{}\", features = [\"ws\"]", crate_specs::AXUM.version)),
            "server manifest must emit SSOT axum version:\n{srv}"
        );
    }
```

2. **Run — expect fail if placed before Task 3's rewire is complete** (a stale
   literal would still pass, so to see red, momentarily bump `SQLX.version` to
   `"9.9"` and confirm BOTH this test and `crate_specs_match_manifests` fail
   together, proving the SSOT now drives emission):
```
cargo test -p sky_backend_rust 2>&1 | rg 'FAILED|test result'
```
   Expected: `emitted_manifests_use_ssot_versions ... FAILED` and
   `crate_specs_match_manifests ... FAILED`. Restore `"0.8"`.

3. **Minimal impl:** none beyond Task 3 — this is a proof test. Restore version.

4. **Run — expect pass:**
```
cargo test -p sky_backend_rust
```
   Expected: all green, `test result: ok.`

5. **Docs sync.** In `docs/architecture/sky-rust-backend-reference-audit.md`,
   update the comparison-table row #15 verdict from `**T+**` (theirs-more-
   principled) to `**O+ (closed #50)**` and adjust the "Adoption + roadmap slot"
   cell to state the SSOT + drift guard now exists as a typed const table
   (noting the deliberate form difference from the reference's embedded TOML,
   and that the drift test additionally covers the golden base manifest). Keep
   it a state description — no chronology/PR-trail narration (per repo doc
   hygiene).

6. **Commit:** `git commit -am "sky_backend_rust: prove emitted manifests use SSOT + close audit item 15 (#50)"`

---

## Out of scope (recorded, not done)

- **Folding the entire golden base manifest** (`tests/golden/m0/Cargo.toml`'s
  serde_json/sha2/regex/… version block) into the SSOT. The base manifest is a
  static `include_str!` golden that also anchors the `golden.rs` byte-equality
  gate; generating it from the SSOT is a larger change than this item's scope
  ("scattered literals in `project.rs`"). The drift test already covers the one
  crate duplicated between the golden and the surgery anchors (tokio), closing
  the specific three-copy skew the audit names. Follow-up candidate.
- **Feature-list SSOT.** Feature lists + `optional` flags intentionally remain
  inline (reference policy; they depend on per-usage gating). Only versions move.

## Spec ambiguities resolved (to make it mechanical)

1. **SSOT form.** The reference uses an embedded `crate-specs.toml` parsed by its
   Haskell emitter. For a Rust-native emitter that is a downgrade (string re-parse
   vs typed data), so the plan uses a **typed `const CrateSpec` table**
   (`crate_specs.rs`). Same invariant, more principled form. Recorded as a
   sanctioned divergence from the reference.
2. **Drift-test placement.** The reference test lives in `runtime-rust/tests/`.
   Because the ipê SSOT is Rust consts (not a readable file), the drift test must
   see those consts, so it is **co-located as a `#[cfg(test)] mod tests`** in
   `crate_specs.rs` — keeps the module private (no public-API widening,
   satisfies pedantic/nursery on non-pub items) while still reading the two
   external manifests via `CARGO_MANIFEST_DIR`.
3. **Comparison targets.** The reference asserts SSOT ≡ `runtime-rust/Cargo.toml`
   only. ipê additionally has a static golden base manifest, so the plan asserts
   SSOT ≡ `runtime/Cargo.toml` (all 11 crates) **and** ≡ golden base manifest
   (crates it declares — tokio). This closes the extra skew axis ipê has that the
   reference does not.
4. **libc location.** `libc` sits under `[target.'cfg(unix)'.dependencies]` in
   `runtime/Cargo.toml`; the ported `parse_deps` treats any `"dependencies"`-
   containing header as a dep section, so it is captured — verified against HEAD.
5. **Lint compliance.** The reference test uses `panic!` and raw slice indexing,
   both **denied** in this workspace. The ported helpers use `.get(..)` /
   `strip_prefix` / `split_once` and `assert!`; `unwrap`/`expect` are permitted
   in tests via `clippy.toml`. If `clippy::panic` fires on the file-read
   `unwrap_or_else(|e| panic!)`, switch to `.expect(...)` (verify from the first
   real `cargo test` run).
