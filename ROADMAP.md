# ipê — Project Roadmap

Where the project is, and what is left. Terse and honest: DONE / IN-FLIGHT /
PENDING against the real tree. Pending work items live in
`scripts/progressive-development/backlog.jsonl` (query via
`scripts/progressive-development/backlog.sh list`/`show`) — the flat SSOT the
progressive-development loop reads and writes. Accepted design decisions live
in `docs/adr/`; this file plans, it does not restate them. Enforcement rules
(the SEAL, principle order, the two-tier gate) live in `PRINCIPLES.md`.

**Principle order** (strict tie-breaker): security > correctness > soundness >
efficiency > completeness > readability. **Two fundamental rules**: parse don't
validate; make invalid states unrepresentable. **The SEAL**: if `skyc` accepts
a program, the emitted Rust must `cargo build`.

---

## Current position

The **Sky(Haskell)→Rust compiler port is essentially done.** Compiler,
backend, runtime, and stdlib all reach broad parity with the Go/Haskell
reference; the milestone ladder M0–M6 is complete. The completion gate is the
example sweep — every non-Go-only example must **build ✓, run ✓, and match the
Go reference ✓**.

**Sweep: one red left.** Every per-example blocker is closed except
`36-composite-server` (#221, `SKY-L0126`). Its diagnostic-misattribution half
(Defect B) has landed; the substantive half (Fix A) is in flight — see below.

**Stdlib: 4 deferred families (#210).** `Std.Cache` and `Std.PubSub` are wired.
`Std.Email`, `Sky.Core.WebSocket` (client subs), `Std.Config`, and the residual
`Std.Cache` emit-layer work fail closed honestly (they need runtime-struct-alias
+ phantom-param + trait-bound codegen beyond plain kernel registration), tracked
as #210. Not sweep-blocking.

Then the pre-push **restructure endgame** (Steps A–D below), then the macro
(Elm-parity) program, then FFI last.

---

## Tier ordering (the spine)

1. **Tier 1 — compiler to completion.** Sweep to green (only #221 left) +
   security hardening (ships before push, never deferred) → the restructure
   endgame → push to `arthurmaciel/ipe-lang`.
2. **Tier 3 — macro / Elm-parity.** Elm-core coverage, adopted principled
   compilation strategies, filed divergent language features, source-level lint
   tool (#207), exhaustive-case-on-finite-ADT (#208).
3. **Tier 2 — FFI (LAST).** Fully-automatic, shim-free binding of arbitrary
   Rust crates, behind a blocking RCE-sandbox security gate.

FFI is last because the ordering puts the security gate ahead of convenience:
an untrusted-crate compile must never run unsandboxed.

---

## IN-FLIGHT — Fix A: clonable function-value carrier (#221)

The one open sweep red. `36-composite-server` legally partially-applies a
captured let-bound function value; our lowerer fail-closes it `SKY-L0126`
because the general first-class-function carrier is non-`Clone` `Box<dyn Fn>`.
The reference Rust backend handles the exact shape with a clonable `Arc<dyn Fn>`
carrier + pre-cloned captures.

**The fix** flips the general `IrType::Fun` carrier `Box<dyn Fn>` → `Arc<dyn
Fn>`, so a captured function value becomes `Clone` and the whole
L0125/L0126/`reject_fn_value_reuse` fail-close family dissolves as
over-representation — a carrier-level structural fix, not a per-site
special-case. `clone_class` becomes derived from the emitter's carrier table
(one predicate, two readers) so the two can never drift. `FnOnceChain` and
`Decoder` keep their carriers (one-shot / nominal, not the `Fun` family).

Execution-ready spec: `docs/architecture/fix-a-clone-carrier-execution-spec-2026-07-16.md`.
Root cause: `docs/architecture/sweep-red-221-l0126-root-cause-2026-07-16.md`.
Escalated as a multi-lane foundational campaign; sequenced **before** the
larger clone-relay move/clone-discipline restructure (`fix-a-pre` tag marks the
rollback point). Expect one re-diagnosis round on `36` after the flip (latent
`kont` / multi-use members predicted to dissolve, not surprise).

---

## Tier 1 — remaining work

### Sweep to green

| Item | Priority | Status |
|---|---|---|
| #221 `36-composite-server` SKY-L0126 — Fix A carrier flip | High | in-flight (Defect B landed) |

Every other per-example blocker (SEAL breaches, kernel gaps, HttpRequest
false-fold, entry-point Task.run elision, clone-relay binder sites, stdlib
Layer-3 resolution, server route-body equivalence) is closed — see the git
history and `docs/adr/`.

### Security hardening (ships before push)

The full security tier has landed: opaque `Secret` (#44) and `SqlFragment`
(#61) sealed newtypes; the Live/HTTP web-security invariants (TLS-gated cookies,
CSRF double-submit + TLS-gating, CSWSH origin checks, body-size floor); the
SQL/DB remainder (typed NULL witness, multi-driver compile-time selection,
tenant-prefix SQL-WHERE enforcement, cacheable-URL parsing); CSS injection
defences (value-as-data escaping, style-marker forgery sink gates); and the
blocking-offload / TOCTOU kernel-robustness pass. Decisions are distilled in
`docs/adr/0004`, `0006`, `0012`, `0013`, `0014`.

The FFI sandbox (#41) is the one security item deferred — it gates FFI shipping,
not the push.

### The restructure endgame → push

The pre-push work is **one campaign, four ordered steps** (spec:
`docs/architecture/repo-restructure-spec.md`). Ordering is deliberate: relocate
before rename before flatten before fmt, so no path or import churns twice and
fmt seals a settled tree exactly once.

| Step | What | Status | Backlog |
|---|---|---|---|
| **A** | Repo-layout relocation (`git mv` only; no renames, no import edits) | pending | — |
| **B** | Sky → Ipê rename (`sky_*`→`ipe_*`, `SKY-`→`IPE-`); **user-review-gated** — classify every finding, user approves a TSV, only then apply; upstream `../sky` refs stay `Sky` | pending | #212 |
| **C** | Namespace flatten — single flat stdlib, nothing imported by default, LSP auto-import on first use (`namespace-imports-and-packaging-spec.md`) | pending | — |
| **D** | Sanctioned `cargo fmt` seal (`SKY_ALLOW_FMT=1`), **LAST** — one workspace-wide pass on the settled tree, pinned rustfmt so CI + local agree | pending | #214 |

Alongside / feeding the push:

| Item | Priority | Status | Backlog / spec |
|---|---|---|---|
| #35 Examples-sweep ported to skyc + run as the gate | High | ported; gates on #221 landing | — |
| #110 Oracle full activation (HTML/tui/scenario normalizers, release-skyc rebuild, CI phase-2 flip, divergence corpus) | High | pending | — |
| #37 CI (examples-sweep.yml + ci.yml) + push to `arthurmaciel/ipe-lang`; includes the upstream-example patch queue | High | pending | — |
| Publish the honest README (relation-to-Elm-and-Sky framing) after re-running the divergences review | Medium | pending | `docs/README-draft-relation-to-elm-and-sky.md` |

### Hardening follow-ups (correctness/efficiency debts, non-sweep-blocking)

| Item | Priority | Status | Backlog |
|---|---|---|---|
| #210 Register the 4 deferred stdlib families needing emit-layer marshalling (`Std.Cache` residual, `Std.Email`, `Sky.Core.WebSocket`, `Std.Config`) | High | pending | #210 |
| #169 `ws_client.rs` client-side WebSocket subs — relax over-strict bound in the same commit that first wires their `KernelFn` arm | Medium | pending (unreachable today) | #169 |
| #170 onSubmit payload classifier — extend `is_definitely_not_callable` to record/tuple/list literal payloads | Low | pending | #170 |
| #31 make-invalid-states-unrepresentable hardening remainder | Medium | pending | — |
| #53 Emit backend via a typed token AST instead of `String` concatenation | Low | pending (guardian-design) | — |
| Efficiency-audit residual (7 gated findings: `ModPathId` `Ord`, scope map, `lower_callee` table, Http consts keying, lexer streaming, `SafeCssValue` lazy) | Low | pending | `docs/architecture/efficiency-audit-2026-07-02.md` |
| AUD-09 Bug-29 (`any`-return matches any `Ty::Con`) — Class-1 guardian item, needs a short spec | Medium | pending | — |

---

## Tier 3 — macro / post-completion program

Elm-parity and de-abbreviation, on a verified-complete base. Divergences go
last. The single `ipe` binary and product identity are settled by Steps B/C of
the restructure.

| Item | Priority | Backlog / spec |
|---|---|---|
| Guarantee Elm `core` library coverage — audit stdlib vs `elm/core`, add missing modules (Array, Tuple, Bitwise, …) | Medium | `docs/architecture/elm-core-coverage.md` |
| #155 Route URL changes to a Msg (Elm `Browser.application` parity); demote the magic `page` field to sugar | Medium | `docs/architecture/url-navigation-msg-design.md` |
| #116 Entry contract — auto-run Task/backend-app `main`, drop trailing `\|> Task.run` | Medium | `docs/architecture/adopt-from-sky-v0172.md` |
| #128 Drop `Task.run`/`Task.perform` from the surface (#116 companion) | Medium | `docs/architecture/drop-task-run-surface-design.md` |
| #131 `Task.map2..5` + `Task.parallel2..5` + `parallelDo` block | Medium | `docs/architecture/task-combinators.md` |
| #133 Multiline-string margin stripping | Medium | `docs/architecture/multiline-string-margin-stripping-design.md` |
| Idea-7 effect `do` block (kills the `let _ = TaskExpr` auto-force wart) | Medium | `docs/ideas/idea-7-effect-do-block-design.md` |
| Source-name de-abbreviation (`kernel_ty`→`kernel_type`, `Ty::Var`→`Type::Variable`; idiomatic Rust abbreviations kept) | Medium | `docs/architecture/readability-and-naming-audit.md` |
| Evaluate more principled compilation strategies from the reference Haskell backend + `elm/compiler`; adopt only where a principle strictly improves, else record the comparison | Medium | `docs/architecture/sky-upstream-learnings.md` |
| Implement filed divergent features (deep-update sugar, or-patterns, pattern guards, record punning, hot-reload family, time-travel debugger, …) | Medium | `docs/divergences-from-sky.md#planned-future-divergences` |
| #56b Row-var record annotation syntax `{ r \| f : T }` (needs per-record-shape callee monomorphisation) | Medium | `docs/adr/0018-row-poly-records-pinned-before-lowering.md` |
| #207 Ipê-level source lint tool (elm-review / clippy shaped, over the typed AST) — DEPARTURE from Sky | Low | #207 |
| #208 Refuse a wildcard `case` arm on a finite-variant ADT (force exhaustive arms) — DEPARTURE from Elm | Low | #208 |
| #209 Co-located property verification (`verify` blocks: examples + compiler-checked laws) — EXTENSION, deferred pending validity review | Low | #209 (deferred) |

---

## Tier 2 — FFI (last)

Design complete and reviewed (`docs/architecture/ffi-port-spec.md`).
Implementation starts only after Tier 1. Scope: **fully-automatic, shim-free
binding of arbitrary Rust crates** (the reference binds Go packages; this is a
recorded divergence). Prove on pure/sync crates first, then async SDKs —
shim-free async-SDK binding is the acceptance metric.

| Item | Priority | Spec |
|---|---|---|
| #40 FFI Phase 0 — inspector hardening (disjoint `tools/` crate) | High | `docs/architecture/ffi-subsystem-design.md` |
| #41 FFI sandbox — blocking security gate before `ipe add` ships | Critical | `docs/architecture/ffi-sandbox-and-generator-impl-ready.md` |
| #42 FFI generator (Haskell → Rust `ipe_ffi` crate); depends on the kernel registry | High | `docs/architecture/ffi-port-spec.md` |
| Async FFI bridge — bind async Rust SDKs (tokio-runtime bridge, `AbortOnDrop` cancel, error funnel) | Medium | `docs/architecture/async-ffi-bridge-design.md` |

---

## Longer-horizon / standing

| Item | Priority | Spec |
|---|---|---|
| Incremental compilation (salsa) across the compiler + LSP — foundation for fast watch + hotpatching | Low | `docs/architecture/incremental-compilation-and-watch.md` |
| Standard-library behaviour audit vs Elm semantics (JSON key order, integer-decoder strictness, float formatting, null/oneOf/nullable) | Low | `docs/architecture/stdlib-elm-behaviour-audit-plan.md` |
| Full floating-point Set/Dict keys (ordered-float) + locale-correct case mapping (lifts SKY-L0117) | Low | `docs/architecture/float-keys-and-locale-case-design.md` |

---

## Designed compilation targets (specs approved; sequencing is a product call)

Each has a complete, security-reviewed spec; where each lands against the tiers
above is a product decision.

- **WASM / browser target** — TEA in the browser, online playground; the
  public-bundle secret boundary is fixed at compile time (server-only effects
  unrepresentable under `--target wasm`; a distinct `HydrationState` gates the
  SSR hydration island); no-eval / strict-CSP posture preserved.
  `docs/architecture/wasm-target.md`.
- **Static compilation** — portable single binaries (musl on Linux, static-CRT
  on Windows, honest macOS limitation) with a pure-Rust `dlmalloc` default
  allocator; mimalloc an explicit notice-emitting opt-in.
  `docs/architecture/static-compilation.md`.
- **Language server (LSP)** — salsa-backed, reusing the compiler's one
  type-checker; headline feature is TEA scaffolding (snippets, code actions,
  lints), every generated edit behind a `VerifiedEdit` re-check gate.
  `docs/architecture/ipe-lsp.md`.
- **TEA everywhere** — opt-in headless `Std.Worker.program` shape for every
  backend, modelled on Elm's `Platform.worker`; strictly additive (existing
  entries byte-unchanged), sound headless termination via source-task liveness.
  `docs/architecture/tea-everywhere.md`.
