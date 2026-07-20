# SP1 — Capability model + inference pass: implementation plan

> **For agentic workers:** implement this plan task-by-task, TDD, one commit per
> task. Steps use checkbox (`- [ ]`) syntax for tracking. All fenced blocks below
> are **illustrative implementation targets, not commands to run** — the exact
> tokens are bound by the implementer against the real tree; the `cargo` lines are
> the TDD loop to run.

**Goal:** the Ipê compiler can compute, from a program alone, the exact set of
capabilities it exercises (`network`, `filesystem`, `env`, `subprocess`, `clock`,
`random`, `native-ffi`), report that set, and verify a declared set equals the used
set.

**Architecture:** each stdlib kernel is tagged with its capability via an
exhaustive per-kernel method on `StdlibKernel` (mirroring `required_runtime_module`),
so a newly-added kernel cannot compile without a capability decision — the set
*cannot drift*. A program's capability set is the union over its transitively-used
kernels plus `native-ffi` if the source crosses `Rust.`. A `ipe capabilities`
subcommand surfaces it; a library `verify` function compares a declared set to the
inferred set. Sandbox enforcement and manifest generation are **out of scope** (SP4
and SP2 consume this).

**Tech Stack:** Rust workspace crates `ipe_kernels` (`src/compiler/kernels`),
`ipe_lower` (`src/compiler/lower`), `ipe-cli` (`src/ipe-cli`); `cargo nextest`.

## Global Constraints

- Principle order is a strict tie-breaker: Security > Correctness > Soundness >
  Efficiency > Completeness > Readability.
- The SEAL holds: `ipe` exit 0 ⇒ emitted Rust builds. This plan adds only analysis
  and a read-only subcommand; it must not alter emission or lowering output.
- The capability set is *generated, cannot drift*: the kernel→capability mapping is
  an **exhaustive `match` with no wildcard arm**, so adding a kernel without a
  capability is a compile error (`make-invalid-states-unrepresentable`).
- `native-ffi` is inferred from any `Rust.` crossing, the same audit primitive as
  `rg '\bRust\.'`.
- Coarse (whole-capability) granularity for v1: `network` whole, not per-host;
  `filesystem` whole, not per-path. Finer granularity is a tracked follow-up.
- Comments say WHAT, not HOW; no archaeology (dates / task numbers / phase labels)
  outside `docs/adr/`; self-explaining names.
- Commits: scoped, plain messages, no AI attribution / no trailer.

## File structure

- `src/compiler/kernels/src/capability.rs` (**create**) — the `Capability` enum + its
  `as_str`/`ALL`. One responsibility: the capability vocabulary.
- `src/compiler/kernels/src/lib.rs` (**modify**) — `mod capability; pub use`; add
  `StdlibKernel::capability(self) -> Option<Capability>` next to
  `required_runtime_module` (:4041). One responsibility extended: per-kernel metadata.
- `src/compiler/lower/src/capabilities.rs` (**create**) — `program_capabilities(...)
  -> BTreeSet<Capability>`: union the reachable kernels' capabilities + the `Rust.`
  crossing check. One responsibility: whole-program capability inference.
- `src/ipe-cli/src/lib.rs` (**modify**) — dispatch `"capabilities"` at :1440; a
  `run_capabilities(rest)` reporter; a `verify_capabilities(entry, declared)` library
  fn for SP2/SP4.

## Out of scope (later sub-projects)

- Writing the inferred set into `ipe.toml [capabilities]` — **SP2**.
- Configuring `ipe_sandbox` from the set / fail-closed enforcement — **SP4**.
- Declared *native* capabilities (author-declared for `Rust.` code) — the datatype
  admits `native-ffi`, but reconciling declared-native vs sandbox is **SP4**.

---

### Task 1: Capability vocabulary + per-kernel tag

**Files:**
- Create: `src/compiler/kernels/src/capability.rs`
- Modify: `src/compiler/kernels/src/lib.rs` (add `mod`/`pub use`; add
  `StdlibKernel::capability` near `required_runtime_module` at :4041)
- Test: inline `#[cfg(test)]` in `capability.rs` + a registry-total test in
  `lib.rs`'s test module

**Interfaces:**
- Produces: `pub enum Capability { Network, Filesystem, Env, Subprocess, Clock,
  Random, NativeFfi }` (derives `Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug,
  Hash, serde::Serialize, serde::Deserialize`); `Capability::as_str(self) ->
  &'static str`; `Capability::ALL: &'static [Capability]`; `StdlibKernel::capability(self)
  -> Option<Capability>` (`None` = pure; the vast majority).

- [ ] **Step 1: Write the failing test** — every wired kernel has a capability
  decision, and representative effect kernels map correctly.

*Illustrative target (test):*
```rust
#[test]
fn every_wired_kernel_has_a_capability_decision() {
    // The exhaustive match in `capability()` is the real guarantee; this asserts
    // it is *callable* over the whole registry (no panic, total).
    for k in StdlibKernel::ALL {
        let _ = k.capability(); // Option — None for pure kernels
    }
}

#[test]
fn effect_kernels_map_to_their_capability() {
    assert_eq!(StdlibKernel::HttpGet.capability(), Some(Capability::Network));
    assert_eq!(StdlibKernel::StringToUpper.capability(), None);
    // + one representative per family the implementer confirms exists in ALL:
    // File* => Filesystem, System/Env => Env, Process* => Subprocess,
    // Time.now/clock => Clock, Random* => Random.
}
```

- [ ] **Step 2: Run it, verify it fails** — `cargo nextest run -p ipe_kernels capability`
  → FAIL (`Capability` / `capability` undefined).

- [ ] **Step 3: Implement `Capability`** in `capability.rs`.

*Illustrative target:*
```rust
/// What a program is permitted to do, on the security-relevant axis. Coarse
/// (whole-capability) for v1: `Network` is any network, not per-host.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash,
         serde::Serialize, serde::Deserialize)]
pub enum Capability { Network, Filesystem, Env, Subprocess, Clock, Random, NativeFfi }

impl Capability {
    pub const ALL: &'static [Capability] = &[
        Capability::Network, Capability::Filesystem, Capability::Env,
        Capability::Subprocess, Capability::Clock, Capability::Random,
        Capability::NativeFfi,
    ];
    pub const fn as_str(self) -> &'static str {
        match self {
            Capability::Network => "network",
            Capability::Filesystem => "filesystem",
            Capability::Env => "env",
            Capability::Subprocess => "subprocess",
            Capability::Clock => "clock",
            Capability::Random => "random",
            Capability::NativeFfi => "native-ffi",
        }
    }
}
```

- [ ] **Step 4: Implement `StdlibKernel::capability`** — an **exhaustive `match self`
  with no `_` arm**, next to `required_runtime_module` (:4041). Classify by kernel:
  the effect families (`Http*`→`Network`, `File*`→`Filesystem`, `System`/env
  reads→`Env`, process/subprocess→`Subprocess`, `Time.now`/monotonic→`Clock`,
  `Random*`→`Random`) return `Some(..)`; everything pure (`String*`, `List*`,
  `Math*`, `Dict*`, `Json*`, `Crypto*` pure transforms, `Log*`, UI/Html/Tea/Db
  builders, …) returns `None`. **No wildcard** — the non-exhaustive-match compile
  error is the checklist that forces a decision for every one of `StdlibKernel::ALL`.

*Illustrative shape:*
```rust
pub const fn capability(self) -> Option<Capability> {
    use StdlibKernel::*;
    match self {
        HttpGet | HttpPost /* … all Http* … */ => Some(Capability::Network),
        FileRead | FileWrite /* … all File* … */ => Some(Capability::Filesystem),
        // … Env / Subprocess / Clock / Random families …
        LogPrintln | StringToUpper /* … every pure kernel … */ => None,
    }
}
```

- [ ] **Step 5: Run tests, verify pass** — `cargo nextest run -p ipe_kernels
  capability` → PASS. Then `cargo build -p ipe_kernels` compiles (proves the match
  is exhaustive). `cargo clippy -p ipe_kernels` clean; `rustfmt --edition 2024`
  clean on both files.

- [ ] **Step 6: Commit** — `feat(kernels): per-kernel capability tag + Capability
  vocabulary`.

---

### Task 2: Whole-program capability inference

**Files:**
- Create: `src/compiler/lower/src/capabilities.rs`
- Modify: `src/compiler/lower/src/lib.rs` (`mod capabilities; pub use`)
- Test: inline `#[cfg(test)]` in `capabilities.rs` + a fixture-driven test

**Interfaces:**
- Consumes: `StdlibKernel::capability` (Task 1); the existing per-program reachable-
  kernel set the lowerer already computes for the `uses_*` flags (the
  `RuntimeModule` doc in `kernels/src/lib.rs` names this scan — locate it via
  `scripts/ipe-index` and reuse it; do **not** add a second traversal).
- Produces: `pub fn program_capabilities(<lowered program handle>) ->
  std::collections::BTreeSet<Capability>` — deterministic (`BTreeSet` ordering),
  union of the reachable kernels' `capability()` plus `Capability::NativeFfi` iff the
  program contains any `Rust.` crossing (FFI kernel / `Ffi.*`). The exact input type
  is whatever the reachable-kernel scan already consumes; bind it there.

- [ ] **Step 1: Write the failing test** over three fixtures the implementer creates
  under the crate's test fixtures dir (or reuses from `examples/`):

*Illustrative target (test):*
```rust
#[test]
fn pure_program_has_no_capabilities() {
    let caps = program_capabilities(lower_fixture("pure_string.ipe"));
    assert!(caps.is_empty());
}
#[test]
fn http_program_infers_network() {
    let caps = program_capabilities(lower_fixture("uses_http.ipe"));
    assert_eq!(caps, BTreeSet::from([Capability::Network]));
}
#[test]
fn rust_crossing_infers_native_ffi() {
    let caps = program_capabilities(lower_fixture("uses_rust_ffi.ipe"));
    assert!(caps.contains(&Capability::NativeFfi));
}
```

- [ ] **Step 2: Run it, verify it fails** — `cargo nextest run -p ipe_lower
  capabilities` → FAIL (`program_capabilities` undefined).

- [ ] **Step 3: Implement `program_capabilities`** — reuse the existing reachable-
  kernel scan, `filter_map(StdlibKernel::capability)` into a `BTreeSet`, then insert
  `NativeFfi` if the program crosses `Rust.`. Reuse the FFI-kernel predicate the
  lowerer/backend already has for the `Rust.` detection; do not re-parse source.

- [ ] **Step 4: Run tests, verify pass** — `cargo nextest run -p ipe_lower
  capabilities` → PASS. `cargo clippy -p ipe_lower` clean; rustfmt clean.

- [ ] **Step 5: Commit** — `feat(lower): whole-program capability inference`.

---

### Task 3: `ipe capabilities` report + declared-set verification

**Files:**
- Modify: `src/ipe-cli/src/lib.rs` — dispatch arm at :1440; `run_capabilities`;
  `pub fn verify_capabilities(entry: &Path, declared: &BTreeSet<Capability>) ->
  Result<(), CliError>`
- Modify: the help text (the `Development` or a new `Analysis` line) + a per-command
  `ipe capabilities --help`
- Test: `src/ipe-cli/tests/capabilities.rs`

**Interfaces:**
- Consumes: `program_capabilities` (Task 2); the entry-resolution + lower path
  `run_explain`/`emit_ir_text` already use (lib.rs :1996 / :2124).
- Produces: CLI `ipe capabilities <entry.ipe>` prints one capability per line
  (`as_str`, sorted), or `none` if empty; exit 0. `verify_capabilities` returns
  `Ok(())` iff `declared == program_capabilities(entry)`, else `Err` naming the
  missing / extra capabilities (SP2/SP4 consume this).

- [ ] **Step 1: Write the failing test** (CLI integration):

*Illustrative target (test):*
```rust
#[test]
fn reports_inferred_capabilities() {
    let out = run_cli(&["capabilities", "tests/fixtures/uses_http.ipe"]);
    assert!(out.status.success());
    assert_eq!(stdout(&out).trim(), "network");
}
#[test]
fn verify_rejects_underdeclared() {
    // declared {} but program uses network => Err
    let r = verify_capabilities(Path::new("tests/fixtures/uses_http.ipe"),
                                &BTreeSet::new());
    assert!(r.is_err());
}
```

- [ ] **Step 2: Run it, verify it fails** — `cargo nextest run -p ipe --test
  capabilities` → FAIL.

- [ ] **Step 3: Implement** the dispatch arm (`cmd == "capabilities" =>
  run_capabilities(rest)`), `run_capabilities` (resolve entry → lower →
  `program_capabilities` → print), and `verify_capabilities` (set compare, precise
  error). Add the help line + `--help`.

- [ ] **Step 4: Run tests, verify pass** — `cargo nextest run -p ipe --test
  capabilities` → PASS. `cargo clippy -p ipe` clean; rustfmt clean; `cargo fmt
  --check` clean.

- [ ] **Step 5: Commit** — `feat(cli): ipe capabilities report + declared-set verify`.

---

### Task 4: End-to-end acceptance over a real example

**Files:**
- Test: `src/ipe-cli/tests/capabilities.rs` (extend) — run `ipe capabilities` over a
  real `examples/*` app that uses effects and assert the reported set.
- Modify: `README.md` — a one-paragraph `ipe capabilities` description + a runnable
  example (user-facing surface → README per house rule).

**Interfaces:**
- Consumes: everything above.

- [ ] **Step 1: Write the failing acceptance test** — pick an `examples/` app whose
  effects are known (e.g. one making HTTP calls + reading files) and assert
  `capabilities` reports exactly that union. Use `scripts/ipe-index` to find a
  suitable example rather than guessing.

- [ ] **Step 2: Run it, verify it fails** (fixture path / expected set not yet wired).

- [ ] **Step 3: Make it pass** — adjust the expected set to the *correct* inferred
  value; if it surprises you (a kernel tagged wrong in Task 1), fix the tag, not the
  test. This is the first real-program check of the mapping.

- [ ] **Step 4: Full-crate green** — `cargo nextest run -p ipe_kernels -p ipe_lower
  -p ipe` → all green. Confirm no emission/golden test changed (this plan is
  analysis-only): `cargo nextest run -p ipe --test golden_basics` unchanged.

- [ ] **Step 5: Update the README** paragraph + example; verify the example runs.

- [ ] **Step 6: Commit** — `feat(cli): capabilities acceptance over examples + README`.

---

## Self-review

- **Spec coverage:** the design's "Ipê code — inferred, nothing to declare" (kernel
  walk → capability set, cannot drift) = Tasks 1–2; "the compiler can report a
  program's inferred capabilities" + "verify the declared set equals the used set" =
  Task 3; `native-ffi` from `Rust.` = Task 2. Sandbox/manifest deliberately deferred
  (marked out of scope).
- **No drift, by construction:** Task 1's exhaustive no-wildcard match is the
  enforcement — not a test that could rot. The registry-total test is a live-ness
  check, not the guarantee.
- **Type consistency:** `Capability` (Task 1) is the single type threaded through
  `program_capabilities` (Task 2) and the CLI (Task 3); `BTreeSet<Capability>` is the
  set representation everywhere (deterministic output).
- **Ambiguity resolved:** coarse granularity for v1 (stated in Global Constraints);
  the input type of `program_capabilities` is bound to the existing reachable-kernel
  scan rather than invented.

## Handoff

On landing, SP2 consumes `program_capabilities` to generate `ipe.toml
[capabilities]`, and SP4 consumes `verify_capabilities` + the set to configure the
fail-closed `ipe_sandbox`. This sub-project stands alone: after it lands, `ipe
capabilities <app>` tells a user exactly what any program may do.
