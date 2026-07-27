# PRINCIPLES.md

Every enforced rule of the Rust-backend project lives here, stated once. The
other governance docs — `AGENTS.md` (Ipê language authoring reference),
`DEVELOPMENT.md` (dev-ops workflow), `misc/scripts/progressive-development/context.md`
(autonomous-lane contract) — reference this file rather than restate it.

## The main values

Ipê is meant to be:

1. explicitly principled: the values, principles, rules and declarations
   stated in this document are entrenched/eternity clauses. They and their
   priority order cannot be changed, even by decisions taken by the majority
   of the community. If there is a need to change them (eg. favor exclusivity
   over community-openess or favor efficiency over security or correctness), it
   is better to fork the project and start a new one with different principles.

3. community-centered: diversity is not only welcomed, but it is the foundation of
   the Ipê project. Respecting diversity in its many forms (related to age, gender,
   sexual orientation, ethinicity, race, culture, physical and cognitive ability,
   experience and socio-economic levels, etc) is MANDATORY. While respecting the
   values, principles and rules, the community has full autonomy to modify and
   extend the language, always trying to reach consensus. If after 3
   attempts of discussions and votes no consensus is reached, a fourth round of
   discussion and vote should elect the majority's decision.

## The six technical principles (strict order)

1. **Security** — generated code and runtime give an attacker no foothold: no
   injection (SQL, shell, path, header, log), no secret leakage into logs or
   errors, no auth/CSRF bypass, no timing oracle on a secret comparison, no
   unbounded resource a remote party can exhaust. On untrusted input the safe
   outcome is the only reachable outcome — fail closed: absent proof the input
   is safe, take the conservative, secure branch, never the permissive one.
2. **Correctness** — same well-typed Ipê program + same input ⇒ the Rust output
   matches the Go reference's observable behaviour (ideally byte-for-byte).
   Deliberate divergence is documented, never silent.
3. **Soundness** — a well-typed Ipê program can never trigger a runtime failure
   in the generated Rust: no panic, no `.unwrap()`/`.expect()` blowup, no
   out-of-bounds index, no integer-overflow abort, no unchecked downcast, no
   UB. Correctness is "the result is right"; soundness is the stronger
   structural guarantee that no input can make the program fall over.
4. **Efficiency** — within the bounds of 1–3: no needless allocation or
   cloning, no hot-path recomputation, no O(n²) where O(n) is trivial, small
   binary and memory footprint. Never bought by trading a higher principle.
5. **Completeness** — cover as much of the language + stdlib as possible. A
   missing kernel/feature is a documented limitation, not a bug; the goal is
   to keep shrinking that set.
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
  valid, the representable outcome is rejection, not a deferred blowup.
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

### Mechanical enforcement — comply by construction

When a lint or gate fires, fix the code — never the lint level, never the gate.

- **Clippy deny-set** (root `Cargo.toml` `[workspace.lints.clippy]`, all
  `"deny"`): `unwrap_used`, `expect_used`, `panic`, `indexing_slicing`,
  `unreachable`, `todo`, `unimplemented`, `pedantic`, `nursery`. `pedantic`
  includes `doc_markdown`: code identifiers in doc (`///`/`//!`) and `//`
  comments MUST be backticked (`` `CloneOk` ``, `` `Vec<T>` ``,
  `` `--all-targets` ``). Applies to `tests/*.rs` too (the `--all-targets`
  end-state). `runtime/src/lib.rs` additionally carries
  `#![cfg_attr(not(test), deny(clippy::indexing_slicing, clippy::panic,
  clippy::unreachable, clippy::todo, clippy::unimplemented,
  clippy::panic_in_result_fn))]`.
- **Escape hatch:** per-site `#[allow(lint)] // one-line why` ONLY — the 3
  INFALLIBLE-tagged HMAC sites in `runtime/`, the 2 `ffi_polyfills`
  dynamic-dispatch `clippy::panic` fallbacks, a shared-test-helper
  `dead_code`. Never a crate- or gate-wide relaxation.
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

Master only ever advances to a full-gate-certified sha.

- **Cheap gate — per landed lane (~minutes):** `cargo check -p ipe` + scoped
  `nextest` on the touched crates + scoped clippy `-D warnings`; for a
  sweep/example blocker, build the fresh `ipe` (`cargo build -p ipe` — a build,
  not a check, since the SEAL step needs the binary) and rebuild that example to
  confirm the original diagnostic is gone (THE SEAL on the specific example).
  A cheap-green lane lands but stays uncertified.
- **Full gate — the ONE authoritative run per batch** (every N cycles or when
  pending work drains): `cargo nextest run --workspace`; `cargo nextest run
  -p ipe-runtime-rust --features full` (LOAD-BEARING — the runtime's
  `default = []` means the workspace run skips every feature-gated test,
  including the entire `live::*` surface); `cargo test --workspace --doc`;
  `cargo clippy --workspace --all-targets -- -D clippy::cargo -D clippy::complexity -D clippy::correctness -D clippy::pedantic -D clippy::perf -D clippy::style -D warnings`; fuzz + full
  examples sweep. Full-green → certify the batch + advance master. Full-red →
  reset to the last certified sha + re-queue.
- The two gates MUST agree on lint scope, and the cheap gate is never
  *stricter* than the full gate. Exact commands: `context.md` §6 and
  `misc/scripts/progressive-development/autopilot.sh`.

### Write-boundary

Exactly two writable locations, for everyone (agent or human):

- **Cargo targets + scratch build state → under `~/.cache/ipe/` ONLY.** Never
  `/tmp`, never `$HOME` root, never a bare `~/.cache/<name>-target`. A target
  outside this root is invisible to disk-reclaim and fills the disk to 100%.
  Emergency prune is one place: `rm -rf ~/.cache/ipe/*` (rebuilds warm).
- **Source/doc/test edits → the repo working tree ONLY.** Boundary: `crates/`,
  `runtime/`, ipe-stdlib compiled-source, `examples/` fixtures, `docs/`.
  `../ipe` is READ-ONLY reference — never edit the Haskell/Go backend or
  upstream.

### Agent-lane operational rules

Every dispatched lane (autopilot or hand-dispatched):

- **Foreground only.** No background processes: no `&`, no `nohup`, no
  `run_in_background`, no Monitor/self-poll on your own build — a detached
  monitor outlives the lane and resurrects killed builds.
- **Every build/test wrapped in `timeout`.** A hung command is killed and
  filed, never waited out.
- **Own `CARGO_TARGET_DIR` per lane** under `~/.cache/ipe/` whenever the
  change touches compiled code — parallel lanes sharing a target race
  (phantom errors). Leaf-only/doc edits may share.
- **No sub-agent dispatch.** A dispatched lane never spawns its own
  sub-agents; the orchestrator is the only dispatcher.
- **Understand before changing — port, don't invent.** Query the indexes
  BEFORE `rg`: `scripts/ipe-index` for our Rust (`def`/`refs`/`kind`/
  `locate`/`parity`), `ipedex` for the `../ipe` reference (`locate`/`rdeps`/
  `covers`/`parity`). Learn how the reference handles the construct across
  compiler + backend + runtime before designing the fix.

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
