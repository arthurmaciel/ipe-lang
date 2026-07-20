# Package coordination & capability-based trust — implementation roadmap

> Companion to `package-coordination-and-capabilities-design.md`. This is the
> dependency-sequenced decomposition, not a task-level plan: the feature is five
> independent sub-projects, and each gets its own full bite-sized TDD plan when it
> is picked up. This roadmap fixes the order and marks what can start now versus
> what is blocked.

**Goal:** ship an authoritative, CI-gated, capability-audited coordination system
for third-party Ipê packages, superseding packaging decision D6.

**Architecture:** a curated GitHub-repo index (no bespoke server) + a compiler
capability-inference pass + a `ipe package` CLI + a gate CI that runs the whole
compiler-quality bar on every submission + `ipe diff` enforced semver. Native code
is bounded fail-closed by `ipe_sandbox`.

**Global constraints (apply to every sub-project):**
- Principle order is a strict tie-breaker: Security > Correctness > Soundness >
  Efficiency > Completeness > Readability.
- The SEAL: `ipe` exit 0 must imply the emitted Rust builds.
- Native capability enforcement is fail-closed: the sandbox denies everything
  outside the inferred-plus-declared capability set.
- Keep D1–D5 of the packaging spec; supersede only D6.
- Commits: scoped, plain messages, no AI attribution / no trailer.

## The five sub-projects

### SP1 — Capability model + inference pass (compiler). Status: UNBLOCKED. Foundational.
Tag each kernel in the registry with its capability (`network`, `filesystem`,
`env`, `subprocess`, `clock`, `random`, `native-ffi`). Add a compiler pass that
computes a program's capability set from the transitively-reachable kernels (plus
any `Rust.` crossing ⇒ `native-ffi`). Surface it: the compiler can report a
program's inferred capabilities, and verify a declared set equals the used set
(mismatch is an error). Standalone value even before any registry exists — `ipe`
can tell you what a program is allowed to do. Depends on nothing (kernels + the
compiler exist).

### SP2 — `ipe rust add/remove` + `ipe.toml` schema. Status: UNBLOCKED. Small.
Rename the current crate-FFI `add`/`remove` dispatch to `ipe rust add` / `ipe rust
remove`, freeing `ipe add`/`remove` for packages (stubbed until SP3). Extend the
`ipe.toml` parser: `[dependencies]` (index name→version, `{git=}`/`{path=}`
escapes), `[rust.dependencies]` (existing crate FFI), `[capabilities]` (generated
by SP1). Add the "Package authoring" help section. The rename is independent; the
`[capabilities]` block consumes SP1.

### SP3 — Index repo + entry schema + resolver + lockfile. Status: BLOCKED on SP2.
Define the index git-repo entry schema (name → source URL, version → content hash,
capabilities, publisher). Implement `ipe add <name>`: fetch the index, resolve the
version, download, verify the checksum, write the lockfile; plus the `{git=}`/
`{path=}` escape resolution. Depends on SP2's manifest schema.

### SP4 — Gate CI (universal Tier-1 + native Tier-2 + fail-closed sandbox). Status: BLOCKED on SP1–SP3 and FFI-subsystem maturity (native tier).
A GitHub Actions workflow in the index repo: on the submission PR, download +
checksum, then the universal tier on the emitted crate — `ipe build` → `cargo
build` → run → tests, `cargo clippy` (strict lints), `cargo fmt --check` — and, for
`Rust.`-bearing packages, the native tier — miri, cargo-audit, cargo-deny, the
declared-capability sandbox check, the "contains native code" label, and manual
review. Failure classification opens tickets (a SEAL break is CRITICAL; a strict-
lint or fmt failure on a well-typed program is High). The ticket target waits on
the separate "issue-tracker" decision. `ipe_sandbox` is configured to the
inferred-plus-declared capabilities (v1 coarse: network namespace, fs/env/pid
isolation, prlimit; v2 fine-grained seccomp). Depends on SP1 (caps), SP2/SP3
(manifest + index), the existing `ipe_sandbox`, and the FFI subsystem being far
enough along for the native tier.

### SP5 — `ipe diff` enforced semver. Status: BLOCKED on the typed-interface surface.
`ipe diff <old> <new>` computes the public API delta from the typed module
interfaces and classifies the required bump (pre-1.0: compatible ⇒ patch, breaking
⇒ minor); the gate rejects a wrong bump. Depends on the per-module typed interfaces
(the `.ipei` / LSP type surface — partly present, partly in flight).

## Dependency order

Illustrative diagram (not commands):

```text
SP1 (unblocked, foundational)
  └─> SP2 (unblocked; [capabilities] consumes SP1)
        └─> SP3 (index + resolver)
              └─> SP4 (gate CI; also needs SP1 + FFI maturity)
SP5 (needs the typed-interface surface) ─────> feeds SP4's semver gate
```

## Recommended build order

1. **SP1 — capability inference.** Unblocked, foundational, standalone value. Build
   first; it is the security core everything else consumes.
2. **SP2 — CLI rename + manifest.** Unblocked, small, enables SP3.
3. **SP3 — index + resolver.**
4. **SP5 — `ipe diff`.** When the typed-interface surface is ready.
5. **SP4 — gate CI.** Last: needs SP1–SP3 and the FFI subsystem matured for the
   native tier.

## Next step

Each sub-project gets its own full bite-sized TDD plan (via the writing-plans flow)
when it is picked up; SP1's is the one to write first. This designed-target sits
behind the current in-flight work (the native formatter, the `ipe fmt` Elm battle-
test, the PR-workflow cutover) unless re-prioritized.
