# SP2 — `ipe rust` CLI group + `ipe.toml` schema: implementation plan

> **For agentic workers:** implement task-by-task, TDD, one commit per task. Steps
> use checkbox (`- [ ]`). All fenced blocks are **illustrative targets, not commands
> to run** — bind exact tokens against the real tree; the `cargo` lines are the TDD
> loop.

**Goal:** free the top-level `ipe add` / `ipe remove` for Ipê packages by moving the
existing crate-FFI commands under an `ipe rust` group, extend `ipe.toml` with the
three dependency/capability sections the package system needs, and add the "Package
authoring" help section. Package *resolution* is stubbed (SP3 implements it).

**Architecture:** the `Ipe.` vs `Rust.` namespace split reaches the CLI —
`ipe add`/`remove` are for Ipê packages, `ipe rust add`/`remove` for the native
crate-FFI boundary. `ipe.toml` gains `[dependencies]` (Ipê, index-resolved),
`[rust.dependencies]` (crates.io FFI, per D5), `[capabilities]` (SP1's inferred set +
declared native). The manifest is parsed parse-don't-validate into typed values.

**Tech Stack:** `ipe-cli` (`src/ipe-cli`), `toml` crate (already a dep — confirm),
`ipe_kernels::Capability` (from SP1), `cargo nextest`.

## Global Constraints

- Principle order (strict tie-breaker): Security > Correctness > Soundness >
  Efficiency > Completeness > Readability.
- The SEAL is untouched — this is CLI + manifest surface, not emission.
- **Parse, don't validate** the manifest: parse `ipe.toml` into typed values
  (`Capability`, a typed dependency-source enum), never stringly-typed maps threaded
  onward. An invalid manifest is a typed error at the parse boundary.
- `make-invalid-states-unrepresentable`: a dependency is *either* an index version
  *or* a `{git=}` *or* a `{path=}` escape — model it as an enum, not a struct with
  three optional fields.
- Package resolution is **stubbed** in SP2: `ipe add <name>` reports that Ipê-package
  resolution arrives with the index (SP3), loudly and cleanly — never a silent no-op.
- Comments say WHAT not HOW; no archaeology outside `docs/adr/`; self-explaining
  names. Commits scoped, plain messages, no AI attribution / no trailer.

## File structure

- `src/ipe-cli/src/lib.rs` (**modify**) — dispatch: replace the `add`/`remove`/
  `install` arms (:1464-1466) with a `"rust"` group arm + stub `add`/`remove` arms.
- `src/ipe-cli/src/ffi.rs` (or wherever `ffi::run_add`/`run_remove`/`run_install`
  live — **modify**) — add `run_rust(rest)` that sub-dispatches `add`/`remove`/
  `install`; the existing bodies move under it unchanged.
- `src/ipe-cli/src/project.rs` (**modify**, parser at :231) — extend the manifest
  type + parser with `[dependencies]`, `[rust.dependencies]`, `[capabilities]`.
- `src/ipe-cli/src/pkg.rs` (**create**) — the stubbed `ipe add`/`ipe remove` (Ipê
  packages) + their help.
- `src/ipe-cli/src/help.rs` (**modify**, `SECTIONS` :244-252) — add the "Package
  authoring" section; move the crate-FFI commands' help under `rust`.

## Out of scope (later)

- `ipe add` actually resolving/downloading (SP3). `ipe package publish`/`audit`
  (SP3/SP4). Sandbox enforcement of `[capabilities]` (SP4). SP2 defines the schema
  and the stubs only.

---

### Task 1: `ipe rust` command group

**Files:** Modify `src/ipe-cli/src/lib.rs` (:1464-1466), `src/ipe-cli/src/ffi.rs`;
Test: `src/ipe-cli/tests/rust_group.rs`

**Interfaces:**
- Produces: `ffi::run_rust(rest: &[String]) -> Result<(), CliError>` sub-dispatching
  `add`/`remove`/`install` to the existing `run_add`/`run_remove`/`run_install`
  bodies (unchanged behavior); `ipe rust` with no subcommand prints the group help.

- [ ] **Step 1: Write the failing test** — `ipe rust add <crate>` behaves as the old
  `ipe add <crate>` did; `ipe rust` alone prints group usage.

*Illustrative target:*
```rust
#[test]
fn rust_add_is_the_old_add() {
    // same effect as the pre-rename `ipe add serde` on a fixture project
    let out = run_cli_in(fixture, &["rust", "add", "serde"]);
    assert!(out.status.success());
}
#[test]
fn bare_rust_prints_group_help() {
    let out = run_cli(&["rust"]);
    assert!(stdout(&out).contains("rust add"));
}
```

- [ ] **Step 2: Run it, verify it fails** — `cargo nextest run -p ipe --test rust_group`.
- [ ] **Step 3: Implement** — add `run_rust` sub-dispatch in `ffi.rs`; in `lib.rs`
  replace the three arms with `cmd == "rust" => ffi::run_rust(rest)`.
- [ ] **Step 4: Run tests, verify pass**; clippy + rustfmt clean.
- [ ] **Step 5: Commit** — `feat(cli): ipe rust command group for the crate-FFI boundary`.

---

### Task 2: `ipe.toml` schema — dependencies, rust.dependencies, capabilities

**Files:** Modify `src/ipe-cli/src/project.rs` (parser at :231); Test: inline
`#[cfg(test)]` there.

**Interfaces:**
- Consumes: `ipe_kernels::Capability` (SP1).
- Produces: manifest type extended with `dependencies: BTreeMap<String, IpeDep>`,
  `rust_dependencies: BTreeMap<String, RustDep>`, `capabilities: BTreeSet<Capability>`;
  `enum IpeDep { Index(VersionReq), Git{url, rev?}, Path(PathBuf) }` (invalid-states-
  unrepresentable). Existing crate-dep parsing migrates under `[rust.dependencies]`.

- [ ] **Step 1: Write the failing test** — a manifest with all three sections parses
  into typed values; each escape shape parses; an unknown capability string is a typed
  error.

*Illustrative target:*
```rust
#[test]
fn parses_all_three_dependency_sections() {
    let m = parse_manifest(r#"
        [dependencies]
        http-extras = "1.2"
        my-fork = { git = "https://…", rev = "abc" }
        local = { path = "../local" }
        [rust.dependencies]
        serde = "1"
        [capabilities]
        set = ["network", "database"]
    "#).unwrap();
    assert!(matches!(m.dependencies["http-extras"], IpeDep::Index(_)));
    assert!(matches!(m.dependencies["my-fork"], IpeDep::Git{..}));
    assert!(m.capabilities.contains(&Capability::Network));
}
#[test]
fn unknown_capability_is_a_typed_error() {
    assert!(parse_manifest("[capabilities]\nset = [\"wormhole\"]").is_err());
}
```

- [ ] **Step 2: Run it, verify it fails** — `cargo nextest run -p ipe project` (or the
  crate the parser test lives in).
- [ ] **Step 3: Implement** — add the typed fields + `IpeDep`/`RustDep` enums; parse
  capability strings via a `Capability::from_str` (add it beside `as_str` in SP1's
  `capability.rs` — the inverse of the wire vocabulary; unknown → error). Keep prior
  manifest fields working (back-compat: absent sections → empty).
- [ ] **Step 4: Run tests, verify pass**; clippy + rustfmt clean.
- [ ] **Step 5: Commit** — `feat(cli): ipe.toml dependency + capability schema`.

---

### Task 3: Stubbed `ipe add`/`remove` + "Package authoring" help

**Files:** Create `src/ipe-cli/src/pkg.rs`; Modify `src/ipe-cli/src/lib.rs` (dispatch),
`src/ipe-cli/src/help.rs` (`SECTIONS` :244-252 + coverage tests :428/:472); Test:
`src/ipe-cli/tests/pkg_stub.rs`

**Interfaces:**
- Produces: `pkg::run_add`/`pkg::run_remove` — parse args, then report cleanly that
  Ipê-package resolution ships with the index (SP3); exit non-zero (unavailable), not
  a silent success. A new `SECTIONS` entry `"Package authoring"` listing `add`,
  `remove` (and `rust` under FFI).

- [ ] **Step 1: Write the failing test** — `ipe add foo` prints the index-coming stub
  and exits non-zero; the help overview shows a "Package authoring" section; the
  `SECTIONS` coverage tests still pass with the new commands.

*Illustrative target:*
```rust
#[test]
fn ipe_add_is_stubbed_loudly() {
    let out = run_cli(&["add", "foo"]);
    assert!(!out.status.success());
    assert!(stdout(&out).contains("package index")); // points to SP3
}
#[test]
fn help_has_package_authoring_section() {
    assert!(run_cli(&["--help"]).stdout_str().contains("Package authoring"));
}
```

- [ ] **Step 2: Run it, verify it fails**.
- [ ] **Step 3: Implement** — `pkg.rs` stubs; dispatch `cmd == "add" => pkg::run_add`,
  `"remove" => pkg::run_remove`; add the `SECTIONS` entry + register the commands so
  `sections_reference_only_known_commands` and
  `plain_top_level_names_every_command_and_section` pass. Add `ipe add --help` /
  `ipe remove --help`.
- [ ] **Step 4: Run tests, verify pass**; clippy + rustfmt clean.
- [ ] **Step 5: Commit** — `feat(cli): stubbed ipe add/remove + Package authoring help`.

---

### Task 4: Full-surface acceptance + README

**Files:** Modify `README.md`; Test: `src/ipe-cli/tests/pkg_stub.rs` (extend).

- [ ] **Step 1: Write the acceptance test** — `ipe rust add`/`ipe rust --help`/`ipe
  add` stub / `--help` sections all behave; a fixture `ipe.toml` with all three
  sections round-trips (parse → the typed manifest).
- [ ] **Step 2: Run, verify it fails** (fixture not yet wired).
- [ ] **Step 3: Make it pass**.
- [ ] **Step 4: Full green** — `cargo nextest run -p ipe` all green; golden suite
  unchanged (`--test golden_basics`); clippy + `cargo fmt --check` clean.
- [ ] **Step 5: README** — document the `ipe rust` group, the `ipe.toml` sections, and
  that `ipe add` (Ipê packages) arrives with the index. Verify each shown command runs.
- [ ] **Step 6: Commit** — `feat(cli): package-authoring surface acceptance + README`.

## Self-review

- **Spec coverage:** rename → Task 1; `[dependencies]`/`[rust.dependencies]`/
  `[capabilities]` schema → Task 2; "Package authoring" section + `add`/`remove` stubs
  → Task 3. Resolution deferred to SP3 (stubbed, loud).
- **Invalid-states-unrepresentable:** `IpeDep` enum (index xor git xor path), not
  three-optional-fields; `Capability` parsed to the typed set, unknown → error.
- **Type consistency:** `Capability` is SP1's type (add `from_str` there, the inverse
  of `as_str`); `IpeDep`/`RustDep` defined in Task 2 are consumed by SP3's resolver.
- **No silent shortcut:** the `ipe add` stub exits non-zero and names SP3's index.

## Handoff

SP3 consumes the Task-2 manifest types (`IpeDep`, `[dependencies]`) to implement the
index resolver + `ipe add` download/verify/lockfile, replacing the Task-3 stub. The
`[capabilities]` block is populated from SP1's `program_capabilities` at
publish/audit time (SP3/SP4).
