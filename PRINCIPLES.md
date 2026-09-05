# PRINCIPLES.md

Every enforced rule of the Rust-backend project lives here, stated once. The
other governance docs — the root `AGENTS.md` (contributor onboarding),
`src/ipe-cli/templates/AGENTS.md.in` (Ipê language authoring reference), and
`docs/internals/dev-ops.md` (deep operational procedure) — reference this file
rather than restate it.

## The main values

Ipê is meant to be:

1. explicitly principled: the values, principles, rules and declarations
   stated in this document are entrenched/eternity clauses. They and their
   priority order cannot be changed, even by decisions taken by the majority
   of the community. If there is a need to change them (eg. favor exclusivity
   over community-openess or favor efficiency over security or correctness), it
   is better to fork the project and start a new one with different principles.

2. community-centered: diversity is not only welcomed, but it is the foundation of
   the Ipê project. Respecting diversity in its many forms (related to age, gender,
   sexual orientation, ethnicity, race, culture, physical and cognitive ability,
   experience and socio-economic levels, etc) is MANDATORY. While respecting the
   values, principles and rules, the community has full autonomy to modify and
   extend the language, always trying to reach consensus. If after 3
   attempts of discussions and votes no consensus is reached, a fourth round of
   discussion and vote should elect the majority's decision.

   The core language and standard library are kept deliberately small and
   principled. Breadth — new capabilities, platform and device integrations,
   domain libraries — is meant to grow as community packages built on these
   principles, so developers and users extend the language together rather than
   waiting on the core to cover every need.

## The six technical principles (strict order)

Ipê users must find a language that embraces all the below principles and
rules during development and use.

1. **Security** — generated code and runtime give an attacker no foothold: no
   injection (SQL, shell, path, header, log), no secret leakage into logs or
   errors, no auth/CSRF bypass, no timing oracle on a secret comparison, no
   unbounded resource a remote party can exhaust. On untrusted input the safe
   outcome is the only reachable outcome — fail closed: absent proof the input
   is safe, take the conservative, secure branch, never the permissive one.
2. **Correctness** — the same well-typed Ipê program with the same input yields
   the same deterministic output, every run. Behaviour is defined by the
   language's own semantics, not by any external oracle; a deliberate divergence
   is documented, never silent.
3. **Soundness** — a well-typed Ipê program can never trigger a runtime failure
   in the generated Rust: no panic, no `.unwrap()`/`.expect()` blowup, no
   out-of-bounds index, no integer-overflow abort, no unchecked downcast, no
   UB. Correctness is "the result is right"; soundness is the stronger
   structural guarantee that no input can make the program fall over.
   **Bounded by construction:** the compiler and the emitted runtime alike
   refuse work whose size an input dictates without a ceiling — every loop,
   recursion, and growable buffer has a declared limit. Source nested tens of
   thousands deep, or a decode of an absurd length, is turned back with a typed
   limit error before it can exhaust the stack or the heap; a process that
   instead dies on such input has broken soundness (and, when the input arrives
   over the network, principle 1's exhaustion clause too).
4. **Efficiency** — within the bounds of 1–3: no needless allocation or
   cloning, no hot-path recomputation, no O(n²) where O(n) is trivial, small
   binary and memory footprint. Never bought by trading a higher principle.
5. **Ease of use** — the language stays out of the developer's way: a small and
   predictable surface, sensible defaults, little ceremony, and diagnostics that
   explain both the problem and the fix rather than only naming it. A feature
   that is present but hard to use correctly is not yet finished.
6. **Readability** — codegen and generated Rust are clear, well-named,
   maintainable. Everything else equal, the clearer form wins.

**The ordering is a strict tie-breaker, not a weighting:** at any conflicting
decision the higher-numbered principle yields — a faster path that opens a
soundness hole is rejected, a more readable form that breaks correctness is
rejected. A lower principle can never justify compromising a higher one.

## The fundamental technical rules

Beneath the ranked principles, every design and code pass obeys:

- **Parse, don't validate.** Convert untrusted/untyped input into a precise
  typed value ONCE at the boundary, so downstream code never re-encounters the
  unvalidated form. Foreign/JSON/config values enter through a typed decode
  point; error channels are typed (`Diagnostic`/`Error`), never `String`.
- **Make invalid states unrepresentable.** Encode invariants in types: a sum
  type over a bool-pair admitting impossible combinations; an exhaustive
  `match` (no wildcard that silently swallows a new variant); a smart
  constructor over an open field. A kernel the resolver recognises but the
  type-scheme table does not cover MUST be a compile-time error — never a
  silent flexible type variable that defers failure to the downstream Rust
  build. This is fail-closed by construction: with no proof the state is
  valid, the representable outcome is rejection, not a deferred blowup. A
  value's *role* lives in its type, never in a bare primitive another value of
  the same shape could stand in for: a message and a key are separate types so
  neither is passable for the other (as the runtime's crypto roles already
  are), and a position, a length, and a byte count are kept apart rather than
  shared as one integer. The swap that would otherwise compile — a secret where
  a plaintext belongs, a length used as an index — has no representation to
  begin with.
- **Fix the structure, not the symptom.** Repair the generative cause — the
  missing invariant, the drifting table, the untyped boundary, the
  special-case that should be a general rule — so the whole defect class
  cannot recur. Before writing a fix, ask "what structural property, if it
  held, would make this class of failure impossible?" and establish it. An
  ad-hoc patch that silences the visible symptom resurfaces one shape over
  (the next `match` arm, the next kernel, another call site). Example:
  coercing only inline-lambda sibling branches to the `Arc` carrier is ad-hoc
  — the identical `E0308` returns when the sibling is a top-level function
  reference; the structural fix eta-expands every function-typed leaf over the
  group's arrow type, closing the class.
- **Single source of truth.** Every fact — a colour, a version, a capability
  name, a kernel signature, a user-facing phrase — is defined in exactly one
  place. Where it must appear in a second form that cannot import the first (a
  shell script mirroring a Rust palette), generate one from the other or assert
  their equality in a test; never hand-sync. SSOT serves the precedence order: a
  one-line duplication caught by a test beats a leaky shared abstraction that
  hurts Correctness or Readability.
- **Defend in depth.** A security- or soundness-critical invariant is enforced
  at more than one independent boundary, so no single missed or bypassed check
  opens the hole. The safe surface never rests its guarantee on one gate: an
  identifier is validated where it is built AND again where it reaches SQL; the
  SEAL is re-checked at scheme, lowering, and emit; a row policy filters in the
  query AND in the database. Enforcing a critical invariant twice is not
  redundant waste — it is the margin that survives a single mistake.

The ordering says what wins in a conflict; these rules say how to build code
that doesn't create the conflict.

### The mandatory technical SEAL — no ipe-exit-0-then-cargo-fail

**If `ipe` accepts a program (exit 0), the emitted Rust MUST `cargo build`.
Never emit codegen that type-checks in ipe but fails cargo.** This is
make-invalid-states-unrepresentable applied to the pipeline itself: an
unschemed-but-resolved kernel, an arity table drifted from its callee table, a
generic where a concrete was required — each is a representable-but-illegal
pipeline state whose symptom is exit-0-then-cargo-fail. Every new acceptance
path (kernel, scheme, lowering arm, emitter case) fails closed at ipe time,
never open at cargo time.

Where two constants or two parallel tables must agree — an arity table with its
callee table, a diagnostic's code family with its prefix letter — assert that
agreement at build time (a `const` check, a type-level relation) so the instant
they drift the *build* breaks, not a test that can be skipped nor a cargo error
two steps downstream. A test guarding the agreement can be deleted; a build that
refuses to compile cannot.

### §0 No shortcuts — root cause or honest blocker

Removing or skipping the file, example, test, fixture, golden, or line that
*triggers* a bug is NOT a fix — it hides it. NEVER edit a reference example,
fixture, or golden to dodge a compiler gap; never weaken a gate; never
`#[allow]` a real violation; never fake a seal. Exactly two acceptable
outcomes for any defect: **root-cause it**, or **report it honestly as a
tracked blocker**. A green obtained by deleting the red is a FAILURE.

- **Root causes only.** Never suppress a type error or warning; a defensive
  cover-up that hides a contract violation IS a violation.
- **Outcome ladder** (governs every change): clean → proceed; a principle is
  hurt → rethink and reimplement within the boundary; no adequate in-boundary
  fix exists → revert, log why, signal the user. Never ship a silent
  workaround.
- **No deferral.** "Pre-existing" / "known edge case" is never a shipping
  excuse: any bug surfacing during dev/sweep/CI/testing — introduced or
  pre-existing — enters the task pipeline on the spot. Only an explicit user
  override ("ship without fixing X") permits shipping a known unfixed issue.
- Every task is a means to the larger goal — making Ipê a better language for
  its developers and users. "Make the sweep green" means make the programs
  actually compile and run correctly, not make red rows disappear. When a
  shortcut would satisfy the literal ask but betray that goal, do the harder
  correct thing or surface the tradeoff — never take the shortcut silently.

### Prove the refusals

A suite that exercises only the happy path certifies half a feature. Pin every
path that must be *rejected* — the malformed input turned away, the fail-closed
branch taken, the value one step past the last legal one — because those are the
paths a regression or an attacker walks in on. A rejection that no test drives is
a rejection one edit away from vanishing unnoticed; the standing check on it is
what keeps it real.

### Mechanical enforcement — comply by construction

When a lint or gate fires, fix the code — never the lint level, never the gate.

- **Clippy deny-set** — enforced by root `Cargo.toml` `[workspace.lints.clippy]`
  (the SSOT: the broad groups + a cherry-picked `restriction` slice, with two
  `cargo` lints allowed as workspace noise). Change the policy there, never in a
  command — every `cargo clippy` is just `-- -D warnings`. `pedantic`
  includes `doc_markdown`: code identifiers in doc (`///`/`//!`) and `//`
  comments MUST be backticked (`` `CloneOk` ``, `` `Vec<T>` ``,
  `` `--all-targets` ``). Applies to `tests/*.rs` too (the `--all-targets`
  end-state). `runtime/src/lib.rs` additionally carries
  `#![cfg_attr(not(test), deny(clippy::indexing_slicing, clippy::panic,
  clippy::unreachable, clippy::todo, clippy::unimplemented,
  clippy::panic_in_result_fn))]`.
- **Escape hatch:** per-site `#[allow(lint)] // one-line why` ONLY — every
  ledgered production allow carries an `IPE-RUST-AUDIT:ACCEPTED` marker, and
  `tools/panic-scan` is the single source of truth for that inventory. Never a
  crate- or gate-wide relaxation.
- **`unsafe` is forbidden.** Exactly ONE sanctioned `unsafe` block exists —
  `prctl(PR_SET_PDEATHSIG)` in `live::console_proxy` — the only reason the
  runtime is not crate-wide `forbid(unsafe_code)`. Every other module is
  `unsafe`-free and stays that way.
- **Edition 2024** — workspace crates and every emitted project.

### No `dyn Any` — concrete over generic

The backend NEVER emits `dyn Any` / `.downcast` / type-erasure. Wildcard `any`
is not polymorphism — it has exactly ONE concrete lowering (an opaque carrier
type chosen per position, e.g. `Dict String String` in pub/sub payload
position), emitted at EVERY position (enum field, pattern binder, fn/decoder
param, Db row arg, eta lambda param, return). Only genuine named type
variables (`a`, `msg`) become Rust generics, monomorphized by rustc. A generic
emitted where a concrete was possible passes a mechanical gate but can ship a
silent runtime bug (e.g. a `TypeId`-keyed broker needs publisher and
subscriber on the same concrete `T`) — always emit concrete when concrete is
possible. Sanctioned runtime-internal *container* exceptions (payload itself
never erased or downcast): `runtime/src/ipe_runtime/cache.rs` and
`runtime/src/ipe_runtime/live/pubsub.rs`.

### The two-tier gate

Master only ever advances to a full-gate-certified sha. Two tiers hold this: a
**cheap gate** on each landed lane (scoped check/test/lint on the touched crates,
plus THE SEAL on the specific example a sweep blocker names) certifies nothing on
its own; a single **full gate** per batch (the whole workspace test/doc/lint set,
fuzz, and the full example sweep) is the one authoritative run — full-green
certifies and advances master, full-red resets to the last certified sha. The two
gates must agree on lint scope, and the cheap gate is never *stricter* than the
full gate. Step lists and exact commands: `docs/internals/dev-ops.md`.

### Write-boundary

Exactly two writable locations, for everyone (agent or human):

- **Cargo targets and scratch build state → under `~/.cache/ipe/` only.** Never
  `/tmp`, never `$HOME` root, never a bare `~/.cache/<name>-target`. A target
  outside this root is invisible to disk-reclaim and fills the disk to 100%.
- **Source, doc, and test edits → the repo working tree only** — the compiler
  crates under `src/compiler/`, the runtime, the compiled-source stdlib, the
  `examples/` fixtures, and `docs/`.

Target naming, the reclaim order, and pruning procedure: `docs/internals/dev-ops.md`.

### Agent-lane operational rules

Every dispatched lane is **foreground-only** (no `&`, `nohup`, `run_in_background`,
or self-monitor — a detached monitor outlives the lane and resurrects killed
builds), wraps every build and test in `timeout`, uses its **own
`CARGO_TARGET_DIR`** under `~/.cache/ipe/` when it touches compiled code (parallel
lanes sharing a target race into phantom errors), and **never dispatches its own
sub-agents** — the orchestrator is the only dispatcher. Understand before
changing: query `tools/scripts/ipe-index` before `rg`, and port a construct
deliberately rather than inventing one. The operational detail is in
`docs/internals/dev-ops.md`.

### Documentation & code standards

- **No archaeology.** Docs and comments state what the rule or design IS now,
  never how it got there: no dates, no task numbers, no phase/milestone/
  campaign labels, no "was X, now Y", no incident stories (ADRs are the one
  sanctioned history home). Git history already records when and why. A
  rationale, when needed, is structural ("a target outside the prune root is
  invisible to reclaim"), not narrative.
- **Comments say WHAT, not HOW — and only when non-obvious.** Names are
  self-explaining to a first-time reader; a comment restating the code, or a
  name that needs a comment, is a smell.
