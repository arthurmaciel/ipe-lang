# ipê — Project Roadmap

This document enumerates the remaining and future work for the ipê
compiler, backend, and runtime, in priority order. It is the durable
plan of record: finish the compiler + backend + runtime first, then
the parked FFI subsystem, then the post-completion program, then the
longer-horizon standing work.

**Principles order.** security > correctness > soundness > efficiency
> completeness > readability. Every decision below is resolved in
favour of the earlier principle when two conflict.

**Two fundamental design rules** govern all work:

- **Parse, don't validate** — turn unstructured input into precise
  types at the boundary; never re-check the same invariant downstream.
- **Make invalid states unrepresentable** — encode invariants in the
  type system so illegal configurations cannot be constructed.

---

## A. Critical path — compiler + backend + runtime to completion

**DONE gate.** Completion is defined by the example sweep: every
non-Go-only example (the 43 examples minus the Go-FFI directories)
passes the GitHub example sweep with all three checks green —
**build ✓, run ✓, equivalent-to-Go-reference ✓.**

### A.1 — Finish all five app shapes on the vetted runtime

Deliver the full backend surface for each of the five application
shapes on the reviewed runtime: **CLI**, **HTTP server**
(Sky.Http.Server), **live web** (Sky.Live), **TUI** (Sky.Tui), and
**Webview**. Concretely this is the Std.Ui / Std.Live / Std.Tui /
Std.Webview backend surfaces.

*Status:* in progress — the Std.Ui contract phase comes first (it is
the shared rendering foundation), followed by the Live / Tui / Webview
surfaces, which can proceed in parallel once the Std.Ui contract is
fixed.

*Rationale:* the app shapes are the user-visible product surface; the
example sweep exercises all five, so none can be deferred without
leaving the DONE gate unreachable.

### A.2 — Fix outstanding pendencies, folded into the shape work

Resolve the known open items, bundling each into the shape work where
scope overlaps:

- **Phase-4 hardening** — continue the make-invalid-states-
  unrepresentable pass so the remaining representable-but-illegal
  states are removed from the IR and runtime boundaries.
- **M5a follow-ups** — the Task arity-3 ICE and the
  Task-in-ADT-constructor gate.
- **M5b-http follow-ups** — header-case parity and the additional
  Http request builders.
- **M5b-db follow-up** — a clean `--features db` build without the
  `live` feature.
- **Principles-audit findings** — the 1 high (`ty_is_equatable`), the
  4 medium, and the 19 low findings, plus a fresh unbiased re-audit
  pass to catch anything the first sweep missed.

*Rationale:* these are correctness/soundness debts already surfaced by
audits and milestone reviews; folding them into overlapping shape work
avoids redundant context-switching and keeps the DONE gate honest.

### A.3 — Reclaim build-cache / disk headroom before heavy local builds

Before any heavy local build, prune build caches and confirm free
disk. A near-full disk fails a build mid-run as `ENOSPC` *after*
type-check and codegen succeed, so it surfaces as a file-copy / install
error and **masquerades as a codegen regression** — wasting the entire
run on a mis-diagnosis.

*Rationale:* this dev box is disk-constrained; the failure mode is
expensive and misleading, so the check is a standing precondition, not
an afterthought.

### A.4 — Run the example sweep on GitHub CI

Move the sweep to GitHub's matrix, which is more capable than the local
box:

- Port the sweep harness — script, supporting library, and skill — and
  its workflow from the reference repository.
- Adapt the harness to drive `skyc`.
- Vendor the non-Go-only examples into `examples/`.
- Run the full sweep on GitHub's matrix.

The first iteration is **build + run** (Go-reference equivalence is
phased in — see A.5). The CI run's results are the authoritative
to-do generator for A.5.

*Rationale:* GitHub's matrix is the sweep engine; the local box cannot
hold a full sweep's artifacts. Making CI the source of the remaining
to-do list keeps the endgame driven by real, reproducible results.

### A.5 — Close every end-to-end gap the sweep surfaces

Iterate through GitHub CI runs, closing each gap the sweep reports,
**including Go-reference equivalence** for every example — build, run,
and equivalence all green.

*Rationale:* equivalence to the reference behaviour is the correctness
bar for the port; a passing build/run without equivalence is not
completion.

### A.6 — Finalize CI and publish

Finalize the CI configuration and push to the public `ipe-lang`
repository. **A green full sweep on GitHub is the completion signal.**

*Rationale:* the public repo plus a green matrix sweep is the durable,
externally-verifiable artifact that marks the critical path done.

---

## B. FFI subsystem — parked until A completes

The FFI design is complete and reviewed
(`docs/architecture/ffi-port-spec.md`). Implementation does not start
until the compiler is done. Scope: **fully-automatic, shim-free
binding of arbitrary Rust crates** (not Go packages).

**Divergence from Sky:** the reference implementation binds Go
packages; ipê's FFI binds Rust crates. The subsystem is otherwise
designed to reach the same fully-automatic, no-user-written-shim
experience.

Ordered work:

1. **Inspector hardening** — make the crate introspector robust across
   the crate shapes the generator will consume.
2. **Sandbox** — the untrusted-crate-compile security gate. This is a
   blocking prerequisite: compiling arbitrary third-party crates is an
   execution-of-untrusted-code surface, so security precedes any
   generation work per the principles order.
3. **Consumer / generator port** — port the binding consumer and
   generator from the reference implementation. This additionally
   depends on the kernel-registry milestone (**M4**).
4. **Prove on pure/sync crates first, then async SDKs** — validate the
   pipeline on simple pure and synchronous crates before extending to
   asynchronous SDKs.

*Rationale:* parking FFI keeps the critical path focused; the ordering
puts the security gate ahead of convenience so an untrusted-crate
compile can never run unsandboxed.

---

## C. Post-completion program

### C.1 — Project rename to `ipe`

Ship a single `ipe` binary spanning the compiler, the future
interpreter, the project doctor, and watch. Apply consistent naming
throughout the codebase, retaining a single acknowledgement line in the
README.

*Rationale:* one binary and one name is the coherent product identity;
the acknowledgement line records lineage without diluting it.

### C.2 — Module-namespace redesign

Replace the two-tier core/std split with a **single flat standard
library**, with nothing imported by default and LSP auto-import on
first use. Research prelude handling in Rust, Elm, Gleam, Haskell, Go,
and Zig before committing to the shape.

*Rationale:* a flat namespace with editor-driven auto-import removes
the friction of remembering which tier a module lives in, while
keeping programs free of implicit global scope.

### C.3 — Source-name de-abbreviation

Rename abbreviated source identifiers for readability — for example
`kernel_ty` → `kernel_type`, `Ty::Var` → `Type::Variable`. Idiomatic
Rust abbreviations are retained.

*Rationale:* readability is a project principle; spelled-out names lower
the cost of navigating the compiler, while conventional Rust shorthand
stays where it aids fluency.

### C.4 — Guarantee Elm `core` library coverage

Audit the standard library against `elm/core` and add the missing
modules and functions (Array, Tuple, Bitwise, and any others the audit
surfaces). The authoritative `elm/core` inventory — every module, type,
and function — is enumerated in
[`elm-core-coverage.md`](architecture/elm-core-coverage.md); the audit
fills in per-function coverage against that reference.

*Rationale:* Elm-core coverage is the completeness bar for a
language in the Elm family; the audit makes the gap explicit and
closeable.

### C.5 — Evaluate more principled compilation strategies

Study the reference Haskell backend and `elm/compiler`, and adopt a
strategy only where it **strictly improves a project principle without
harming a higher one**. Where a strategy is not adopted, record a
comparison table capturing the trade-off.

*Rationale:* the principles order is the arbiter; documenting the
rejected alternatives preserves the reasoning for future revisits.

### C.6 — Implement the filed divergent language features

Implement the language features filed in
`docs/ideas/departures-from-sky.md`:

- hot-reloading
- Std.Ui-as-IR
- standalone TEA
- deep-update sugar
- or-patterns
- pattern guards
- effect-sequencing binds
- record punning
- extended Unicode support
- a dev-only time-travel debugger

**Divergence from Sky:** these features are intentional departures
from the reference language; each is tracked with its own rationale in
the ideas document.

*Rationale:* these are deliberate, pre-filed enhancements to
developer experience and expressiveness, sequenced after the core is
complete so they build on a stable foundation.

---

## D. Longer-horizon / standing

### D.1 — Incremental compilation (salsa)

Introduce salsa-based incremental compilation across the compiler and
the LSP — the foundation for fast watch and hotpatching.

*Rationale:* incremental recomputation is the enabling technology for
a responsive edit loop; it is a dedicated effort best undertaken once
the core is stable.

### D.2 — Standard-library behaviour audit against Elm semantics

Audit standard-library behaviour against Elm semantics, covering at
least: JSON object key order, integer-decoder strictness, float
formatting, and null / oneOf / nullable handling.

*Rationale:* subtle semantic mismatches are the hardest bugs to find
after the fact; a targeted audit against a known reference catches them
before they reach users.

### D.3 — Full floating-point Set/Dict keys and locale-correct case mapping

Support full floating-point keys in Set and Dict (ordered-float) and
locale-correct case mapping.

*Rationale:* these complete the standard library's data-structure and
text-handling surfaces for correctness across the full input domain.

---

## E. Designed compilation targets (specs approved; priority to be set)

Each has a complete, security-reviewed design spec; sequencing against
sections A–D is a product decision.

### E.1 — WASM / browser target

Compile ipê programs to WebAssembly so apps run client-side in the
browser (TEA in the browser, reusing the ported VNode/diff to drive the
real DOM), and support an online playground. The design fixes the
public-bundle secret boundary at compile time (server-only effects are
unrepresentable under `--target wasm`; a distinct `HydrationState` type
gates what may enter the SSR hydration island) and preserves the
no-eval / strict-CSP posture. Spec:
[`wasm-target.md`](architecture/wasm-target.md).

*Rationale:* client-side execution is what a real online experience
requires; the capability matrix records exactly what does and does not
cross to the browser.

### E.2 — Static compilation (portable single binaries)

Produce fully-static, portable binaries — musl on Linux, static-CRT on
Windows, with an honest macOS limitation — with a pure-Rust **`dlmalloc`
default allocator** (clears the musl-malloc throughput cliff without a C
dependency, per the security-first order); mimalloc is an explicit,
notice-emitting opt-in. Spec:
[`static-compilation.md`](architecture/static-compilation.md).

*Rationale:* single-binary distribution is the baseline for deployment;
the allocator choice is set by the principle order (security over the
concurrent-throughput headroom a C allocator would add).

### E.3 — Language server (LSP)

A salsa-backed, editor-agnostic language server: diagnostics, hover,
go-to-definition, completion, semantic tokens, formatting, and rename —
reusing the compiler's single type-checker (no divergent analyzer). Its
headline feature is **TEA scaffolding** — snippets, code actions ("add
`Msg` variant + matching `update` arm", "convert `main = Task.run` to a
worker"), and lints/hints — delivered over standard LSP so it works in
most editors. Every generated edit passes a `VerifiedEdit` gate that
re-checks the whole edit blast radius, so a scaffold can never break the
build. Spec: [`ipe-lsp.md`](architecture/ipe-lsp.md).

*Rationale:* the editor experience is where TEA's ergonomics are taught;
making scaffolds correct by construction keeps that experience
trustworthy.

### E.4 — TEA everywhere (opt-in worker shape)

Make The Elm Architecture an opt-in program shape for every backend —
including a headless `Std.Worker.program` (init / update / subscriptions,
no view) for CLI and long-running processes, modelled on Elm's
`Platform.worker`. Least-intrusive: existing entries (`main = Task.run`,
`Live.app`, `Server.listen`) are byte-unchanged; TEA is strictly
additive and reuses the ported TEA runtime. The headless loop terminates
soundly by tracking live source-task liveness (a signal-only daemon
stays alive for SIGTERM; a quiescent worker exits cleanly). Spec:
[`tea-everywhere.md`](architecture/tea-everywhere.md).

*Implementation invariants (from the design review, to enforce at build
time):* sequence the counter Acquire-loads before `try_recv`; `select!`
over mailbox-recv and a quit-notify so a full-mailbox daemon still
observes SIGTERM; abort (not await) source tasks during the quit-drain.

*Rationale:* TEA is the defining strength of the Elm family; extending
it uniformly to CLI/worker programs — without forcing it where routes
are the better model — makes that strength available everywhere.
