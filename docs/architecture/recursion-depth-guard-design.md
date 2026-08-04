# Recursion depth guard: containing unbounded user recursion

A user function that recurses without a reachable base case consumes the native
stack until it hits the guard page, and a stack overflow is an immediate
`abort()` — not a panic. It does not unwind, so every panic-containment
mechanism the runtime already has (the classifying panic hook, the
`catch_unwind`-based task funnels, the server's per-request catch-panic layer)
is bypassed, and the whole process dies. For a long-lived server that is a
one-request denial of service against every in-flight request.

This document specifies the containment design: a **runtime recursion guard**
(a thread-local depth budget plus a remaining-stack red-zone probe) invoked
from a **single-line prologue** in every emitted user function, which converts
the uncatchable abort into an ordinary classified panic that the existing
containment machinery already knows how to isolate. It fixes the failure
semantics per app shape, the default limit and its configuration, the exact
emission hook, and the ordered, test-first plan to land it.

All Rust snippets are illustrative sketches of the intended shape, not
verified output; exact spellings are fixed by the tests of each increment.

## 1. Where the runtime stands

Everything below is verified against the current tree; these are the
mechanisms the design composes with (and the reason the chosen strategy is
small).

| Mechanism | State | Where |
| --- | --- | --- |
| Classifying panic hook — maps an escaping panic to a Ipê error kind + 8-hex `errId`, logs server-side, resumes the unwind | shipped; installed first in emitted `fn main()` | `src/runtime/rust/src/core.rs:961` (`install_panic_classifier`), `:903` (`classify_and_log_panic`), `:853` (`classify_panic`); template call at `src/compiler/backend/rust/templates/main.rs:302` |
| Per-request panic isolation — a panicking handler returns a 500 carrying only the `errId`, server survives | shipped (`tower_http::catch_panic::CatchPanicLayer`) | `src/runtime/rust/src/server.rs:718` (Ipe.Http.Server), `src/runtime/rust/src/web/mod.rs:2179` (Ipe.Tea.Web page path); shared body shape `core.rs:936` (`panic_500_body`) |
| Per-task panic funnels — a panicking future/task maps to `IpeResult::Err` | shipped (`thread::spawn(...).join()` / `try_into_panic`) | `src/runtime/rust/src/task.rs:78` (`block_on`, tokio), `:126` (`block_on`, std-only), `:205` (`block_on_current_thread`), `:534`/`:588` (`task_parallel`, both cfg variants); redaction via `core.rs:88` (`ipe_error_from_panic`) |
| Live-session driver — folds each Msg through the user's `update` inside a `tokio::spawn`ed task | shipped | `src/runtime/rust/src/web/mod.rs:612` (`drive_session`), spawned at `:1669` |
| HTTP handler dispatch — the user handler runs inside the axum service future on a tokio worker thread | shipped | `src/runtime/rust/src/server.rs:637` (`method_router`), `:695` (`server_listen`) |
| Framework-data recursion cap — view-tree descent truncates at a fixed depth | shipped | `src/runtime/rust/src/ui/render.rs:492` (`render_element`), `:507` (`render_element_depth`), ceiling `src/runtime/rust/src/html.rs:176` (`MAX_HTML_DEPTH = 1024`); regression `render.rs:1410` |
| Self-tail-recursion → loop — a proven tail-recursive body emits as `Expr::TailLoop` (no stack growth) | shipped | `src/compiler/lower/src/lower.rs:5982` (`analyze_tail_recursion`), `:6135` (`rewrite_tail_calls`) |
| User function emission — every user def renders as a plain `pub fn` item; the body block is assembled in exactly one place | shipped | `src/compiler/backend/rust/src/emit_expr.rs:9122` (`emit_func`), `:9133` (`emit_func_vis`), body assembly at `:9300` |
| Shared tokio runtime — one process-global `Runtime::new()` behind every `block_on` | shipped | `src/runtime/rust/src/task.rs:39` (`global_runtime`) |

The gap: nothing counts **user compute recursion**. `render_element_depth`
bounds the framework's own walk over user *data*; a user function that calls
itself (directly, mutually, or through a function value) compiles to a native
Rust call with no counter and no guaranteed tail-call elimination beyond the
self-tail `TailLoop` case.

## 2. Scope: stack exhaustion, not non-termination

This design contains **stack-depth exhaustion** only.

- **In scope:** any call chain of emitted user functions whose native stack
  consumption grows without bound — direct recursion, mutual recursion,
  recursion through function values or CPS-style chains. All of these
  re-enter some emitted function on every cycle, which is what the guard
  keys on (§3.3).
- **Out of scope, explicitly:** CPU-bound non-termination that does *not*
  grow the stack — `while`-style infinite loops, and self-tail-recursive
  functions the compiler has already rewritten into a `TailLoop`. A
  tail-rewritten infinite recursion spins at constant stack depth; it is a
  liveness problem (a hung request), not a process-integrity problem (a dead
  server), and belongs to a separate timeout/deadline design. The guard runs
  once at function entry, so a `TailLoop` pays nothing per iteration — the
  exemption is structural, not special-cased.
- **Also out of scope:** static non-termination detection (a compiler lint /
  LSP warning for obviously base-case-free recursion). Valuable developer
  experience, but undecidable in general, so it can never be the enforcement;
  it is a follow-up, not a dependency of this design.

## 3. Enforcement boundary

### 3.1 The candidates

Four boundaries were weighed, under the precedence Security > Correctness >
Soundness > Efficiency > Completeness > Readability:

1. **(a) Runtime depth budget checked at emitted-function entry.** A
   thread-local counter, incremented on entry and decremented on exit; over
   the limit, fail with a controlled error. Deterministic, portable,
   teachable ("your function recursed N times"); catches mutual and
   value-mediated recursion because the counter is per-thread, not
   per-function. Cost: a few nanoseconds per user call; requires the emitter
   to plant one prologue line per function. Alone it is **not sound**: a
   fixed depth cap times an unbounded frame size (users can put large
   by-value records on the stack) can exceed any fixed stack before the cap
   trips.
2. **(b) Per-request / per-task isolation at the server boundary.** Run each
   request in a sacrificial worker (process pool, or respawnable thread)
   so an overflow kills only the worker. Sound but heavy: loses in-process
   live-session state, invites restart storms under attack traffic, has a
   poor Windows/fork story, does nothing for the Console/Terminal shapes,
   and duplicates isolation the runtime already has once the abort is
   converted into a panic — the `CatchPanicLayer` and task funnels of §1
   *are* the per-request isolation, they just never get the chance to run
   today.
3. **(c) Native stack awareness.** Either segmented grow-on-demand stacks
   (`stacker::maybe_grow`-style reserve-and-grow) or a measured
   remaining-stack red-zone check. Growing rejects itself: it converts a
   stack DoS into an unbounded-heap DoS and only postpones the failure.
   The *measurement* half, however, is exactly what makes a depth budget
   sound: "remaining stack below the red zone" catches the fat-frame case a
   counter cannot, at the cost of being platform-dependent about *when* it
   trips.
4. **(d) A compile-time bound.** Rejected as enforcement: termination and
   depth analysis are undecidable; any static scheme has false negatives,
   and a false negative here is the DoS surviving.

### 3.2 Chosen: a runtime guard (depth budget + red-zone probe) behind a one-line emitted prologue

The guard is **one runtime function** the emitted code calls at the top of
every user function body:

```rust
// runtime, core.rs — always compiled, no feature gate, no new dependency
pub struct RecursionGuard { /* private */ }

#[inline]
pub fn recursion_guard() -> RecursionGuard {
    // thread-local state: depth: Cell<usize>, stack_floor: Cell<Option<usize>>
    // 1. depth += 1; if depth > limit()            -> trip
    // 2. (non-wasm) if approx_sp() < floor + RED_ZONE -> trip
    // trip: panic_any(RecursionLimit)  // zero-size typed payload; the
    //       classifier downcasts the type, and renders its fixed message
    //       "maximum recursion depth exceeded" to the log / 500-side / stderr
}

impl Drop for RecursionGuard {
    fn drop(&mut self) { /* depth -= 1 */ }
}
```

and the emitter plants, in the single body-assembly site
(`emit_expr.rs:9300`):

```rust
pub fn main_count_odd(o: MainOdd) -> i64 {
    let _ipe_recursion_guard = recursion_guard();
    // ...existing body...
}
```

Two checks, one TLS access, one trip path:

- **The depth budget (deterministic, teachable).** A per-thread counter
  against a configurable limit (§5). This is the check that fires in the
  common case — typical emitted frames are small — so the failure is
  reproducible at the same depth on every platform and the error message can
  teach ("the function called itself too many times without reaching a base
  case").
- **The red-zone probe (the soundness backstop).** The runtime records a
  stack floor for every thread it owns (§3.4); the guard compares the
  address of a local against `floor + RED_ZONE` (256 KiB). This is what
  makes the design sound for frames of *any* size: even if a single frame is
  hundreds of kilobytes, the probe trips while a comfortable margin remains
  for the panic/unwind machinery and any runtime kernels beneath the last
  guarded entry. On a thread whose floor was never recorded (a foreign
  thread calling into emitted code), the probe degrades to depth-only —
  fail-safe, never a false trip. `cfg(target_arch = "wasm32")` compiles the
  probe out (no native stack introspection; the depth budget remains).
- **The trip is a `panic_any(RecursionLimit)` — a typed zero-size payload.**
  That is the entire trick: a panic *unwinds*, so every containment mechanism
  in §1 that the abort bypassed now works unchanged. The classifier downcasts
  the `RecursionLimit` type rather than matching a message substring, so the
  `RecursionLimit` classification is driven by the type, not by the wording; the
  fixed human message `maximum recursion depth exceeded` is rendered back from
  the type at the log / 500-side / stderr sites, byte-unchanged. The RAII decrement makes the counter
  correct across that unwind (frames between the trip and the catcher
  restore their decrements as they unwind). The binding must be a *named*
  underscore-prefixed variable (`let _ipe_recursion_guard = …`), never
  `let _ = …`, which would drop — and decrement — immediately.

Why this combination wins under the precedence order:

- **Security/Soundness:** the probe makes the containment hold for any frame
  size; the panic conversion reuses containment paths that are already
  regression-tested (`CatchPanicLayer` 500s, task funnels). No unsafe code,
  no signal handling, no new dependency.
- **Correctness:** the depth budget is deterministic; recursion up to the
  limit is untouched (a correct `factorial 500` behaves identically); the
  RAII discipline keeps the counter exact under both normal return and
  unwind.
- **Efficiency:** one inlined TLS bump + two compares per user call. The
  hot-path cost is bounded and measurable; if it ever matters, the
  instrumentation set can shrink (increment 6) without touching the
  mechanism. `TailLoop` iterations — the actual hot loops the compiler
  produces — pay zero.
- **Completeness:** mutual recursion, recursion through stored function
  values, and CPS chains are all caught, because any unbounded chain must
  re-enter *some* emitted function infinitely often, and every emitted
  function is guarded.

### 3.3 Rejected alternatives (summary of §3.1, with the specific "no")

| Alternative | Rejected because |
| --- | --- |
| Worker-process / respawnable-thread isolation as the primary mechanism | Heavy and lossy (live-session state, restart storms, fork-on-Windows); redundant once abort→panic conversion exists — the shipped `CatchPanicLayer` + task funnels already isolate a *panic* per request/task at ~zero added cost; does nothing for CLI shapes |
| Segmented / growable stacks (`stacker::maybe_grow`) | Converts stack DoS into unbounded-memory DoS; still needs per-function instrumentation; adds a native dependency (`psm`) with a portability tax; delays instead of failing fast |
| Guard-page SIGSEGV recovery (sigaltstack + longjmp) | Unsound in Rust (skips destructors — UB), async-signal-safety minefield, no Windows story. Rejected outright |
| Compile-time termination / depth bound as enforcement | Undecidable; false negatives are the DoS surviving. A best-effort lint is follow-up DX only |
| Depth budget alone (no probe) | Not sound: limit × unbounded frame size can exceed any fixed stack before the counter trips |
| Probe alone (no depth budget) | Platform-dependent trip depth: nondeterministic tests and an unteachable error; loses the deterministic common case |
| Instrument only statically-recursive functions from day one | A cycle through function values is not visible in the named-call graph; shipping the narrow set first risks an unguarded cycle. Uniform instrumentation ships first; narrowing is a benchmark-gated later increment with its own soundness argument (§8, increment 6) |

### 3.4 Known stacks: normalizing what the runtime spawns

The probe needs a floor, and the depth default (§5) needs a stack size to be
calibrated against. Today the effective stacks are accidental: `block_on`
(`task.rs:78`) spawns a thread with the platform default (typically 2 MiB),
and `global_runtime` (`task.rs:39`) builds `Runtime::new()` whose worker
threads also default to 2 MiB — meaning server handlers currently live on
stacks a quarter the size of the main thread's. The design makes them
explicit and uniform:

- `global_runtime` builds the runtime with `thread_stack_size(8 MiB)` and an
  `on_thread_start` hook that records the thread's stack floor in the guard's
  TLS (covers axum handlers, `drive_session`, `task_parallel` workers, and
  the blocking pool).
- `block_on`'s entry thread (`task.rs:78`) is spawned via `thread::Builder`
  with the same 8 MiB and records its floor first thing.
- The std-only `block_on` (`task.rs:126`) and `block_on_current_thread`
  (`task.rs:205`) poll on the *calling* thread — the process main thread
  (8 MiB by platform convention). `install_panic_classifier` (`core.rs:961`),
  already the first call in every emitted `main()`, additionally records the
  main thread's floor. A shape that never installs the hook simply runs
  depth-only on that thread.

The floor is recorded as "address of a local at thread entry minus the known
stack size, plus the red zone" — stacks grow downward on every supported
target, so the comparison in the guard is a single pointer compare. No
platform stack-introspection API is required.

The red-zone size (256 KiB) carries an explicit contract: **one emitted frame
plus the deepest non-guarded call chain beneath it must fit in the red
zone.** Runtime kernels satisfy this — they are either non-recursive or
data-capped (`MAX_HTML_DEPTH`); FFI calls are the one surface where the
implementer must confirm the margin (§10).

## 4. Failure semantics

Fail-closed and observable, per shape — in every case the trip carries the
typed `RecursionLimit` payload, which `classify_and_log_panic` downcasts to the
kind `RecursionLimit` before the message path is consulted. The message-based
`classify_panic` retains a `"recursion depth"` arm (ordered *before* the
`"overflow"` arm, and the fixed message omits "overflow") as the fallback for a
panic that carries the recursion wording only as a string; a typed trip never
reaches it, so it can never misclassify as `ArithmeticOverflow`:

| Shape | Today (stack overflow) | With the guard |
| --- | --- | --- |
| Ipe.Http.Server | whole-process SIGABRT; every in-flight request dies | the handler's panic unwinds into `CatchPanicLayer` (`server.rs:718`) → that one request gets the existing 500 body `{"error":"internal server error","ref":"<errId>"}` (`panic_500_body`, `core.rs:936`); the raw classified detail goes to the server log only; concurrent requests and the listener are untouched |
| Ipe.Tea.Web (live) | whole-process SIGABRT; every session dies | a trip inside `update`/`view` on the session driver (`drive_session`, `web/mod.rs:612`) unwinds into the `tokio::spawn` boundary → that one session's driver ends; the client's existing session-lost recovery path (404 + `X-Ipê-Web` probe → reload) re-establishes a fresh session; a trip on the page-request path hits the layer at `web/mod.rs:2179` → 500. The process and every other session survive |
| Ipe.Console / Ipe.Terminal / Program | SIGABRT, no message, no exit code discipline | the panic hook logs the classified line (`[error] RecursionLimit (ref …): maximum recursion depth exceeded`), the `block_on` join funnel (`task.rs:78`) maps it to a typed `Err` via the redacting `ipe_error_from_panic` (`core.rs:88`), the emitted `main` epilogue prints and exits 1 — the same discipline as every other runtime defect |
| `Task.parallel` / spawned tasks | SIGABRT | the existing `try_into_panic` funnel (`task.rs:534`/`:588`) maps the trip to `IpeResult::Err` for that task |

Deliberate semantics choices:

- **It is a defect, not a domain error.** The trip does not surface as a
  user-matchable `Result` variant in the Ipê program, exactly like a
  division-by-zero or index panic: the program's own error type is not
  polluted with an infrastructure failure, and there is no temptation to
  "handle" unbounded recursion by retrying it. What the user observes is the
  shape-appropriate containment above — a 500 with a correlation ref, a dead
  session that recovers, or a clean nonzero exit with a classified message.
- **Never silent.** There is no truncation-and-continue (contrast
  `render_element_depth`, which truncates *data* rendering): a tripped
  computation never produces a partial value.
- **Fixed message text, numbers in the log only.** The panic message is the
  constant `maximum recursion depth exceeded` — byte-stable for golden
  tests, no information leak through the 500 (which carries only the
  `errId` anyway). Whether the server-side log line additionally carries the
  depth/limit is deferred (§10).

## 5. The limit

- **Default depth: 10 000.** Rationale: Python's 1 000 is famously too low
  for functional idioms; JavaScript engines — the reference host for
  Elm-family languages — sit in the ~10 000–60 000 frame range, so programs
  ported from that ecosystem meet a familiar ceiling. Calibrated against the
  normalized 8 MiB stacks (§3.4): typical emitted frames run ~100–800 bytes,
  so 10 000 frames occupy ~1–8 MiB — the budget trips deterministically for
  typical frames, and the red-zone probe covers the fat-frame tail. Legit
  10 000-deep non-tail recursion over user data is rare enough that hitting
  the limit is overwhelmingly a bug report about the user's own code, which
  is the teachable outcome.
- **Override: `IPE_RECURSION_LIMIT`**, read once per process (lazy, cached).
  Unset / unparsable / zero → the default; any positive value is accepted
  (the probe still backstops a value set recklessly high — raising the env
  var can never reintroduce the abort). Environment-variable configuration
  follows the established runtime precedent (`IPE_HTTP_BIND`,
  `IPE_LOG_FORMAT`). No per-app config surface for now: the Elm reference
  has no such knob, and adding one to the cfg record is a reversible later
  decision (§10).
- **Red zone: 256 KiB, not configurable.** It is an internal soundness
  margin with a stated contract (§3.4), not a user-facing tunable.
- **Stack size: 8 MiB on every runtime-owned thread, not configurable.**
  Uniformity is the point: the same program trips at the same depth whether
  the recursion started from a CLI `main`, an HTTP handler, or a live
  session driver.

## 6. The emission hook — and staying out of the row-polymorphism lane

The compiler-side footprint is deliberately a single seam:

- **One insertion point.** `emit_func_vis` assembles every user function as
  `{signature} {{\n    {body}\n}}` at `emit_expr.rs:9300`. The guard is one
  line prepended to `body` at that site. Uniform: every user `Func` item gets
  it — including `ipe_main` and CAF bodies (one call at process start is
  free, and uniformity keeps the goldens and the reasoning simple).
  Signature rendering (`render_fn_signature`, `emit_expr.rs:9368`) is
  untouched.
- **One template shim.** The emitted crate calls a local shim (added to
  `src/compiler/backend/rust/templates/main.rs` beside the existing kernel
  shims) that delegates to the runtime:
  `fn recursion_guard() -> ipe_runtime::core::RecursionGuard`. Split-module
  files reference it as `crate::recursion_guard()`, the same convention
  cross-module user calls already use.
- **Zero overlap with the in-flight row-polymorphism work.** That lane owns
  the lowering gate (`lower.rs:1319`, `canon_type_has_open_row` and its
  callers) and generic-clause rendering (`render_fn_generics`,
  `emit_expr.rs:9550`). This design touches neither `lower.rs` nor any
  signature/generics rendering — only the body-assembly line at
  `emit_expr.rs:9300` and the runtime. The two changes compose textually
  (a row-poly function body gets the same prologue line as any other) and
  can land in either order.
- **Tail calls are already handled upstream.** `rewrite_tail_calls`
  (`lower.rs:6135`) has replaced qualifying self-tail recursion with
  `Expr::TailLoop` before the backend runs; the prologue lands outside the
  loop, preserving the zero-per-iteration property (§2).

## 7. Touch points

Everything the implementer edits, with verified anchors:

| # | File / anchor | Change |
| --- | --- | --- |
| 1 | `src/runtime/rust/src/core.rs` (new section beside the panic gate, near `:853`–`:961`) | `RecursionGuard` RAII type, `recursion_guard()`, TLS state (depth, floor), limit read (`IPE_RECURSION_LIMIT`), red-zone constant, floor-recording helper; wasm cfg for the probe |
| 2 | `src/runtime/rust/src/core.rs:853` (`classify_panic`) | new first-match arm: message contains `"recursion depth"` → `"RecursionLimit"` (ordered before the `"overflow"` arm); extend the kind-map test at `core.rs:1196` |
| 3 | `src/runtime/rust/src/core.rs:961` (`install_panic_classifier`) | record the calling (main) thread's stack floor |
| 4 | `src/runtime/rust/src/task.rs:39` (`global_runtime`) | build via `Builder::new_multi_thread()` with `thread_stack_size(8 MiB)` + `on_thread_start` floor recording |
| 5 | `src/runtime/rust/src/task.rs:78` (`block_on`, tokio variant) | spawn via `thread::Builder` with `stack_size(8 MiB)`; record the floor at closure entry |
| 6 | `src/compiler/backend/rust/templates/main.rs` (beside the kernel shims; entry point at `:298`) | `recursion_guard()` shim delegating to `ipe_runtime::core::recursion_guard` |
| 7 | `src/compiler/backend/rust/src/emit_expr.rs:9300` (`emit_func_vis` body assembly) | prepend `let _ipe_recursion_guard = recursion_guard();` to every user function body |
| 8 | `src/ipe-cli/tests/` golden suite | new fixtures (trip + non-regression, §8); mass re-bless for the prologue line |
| 9 | server E2E tests (beside the existing panicking-handler-500 regression, `src/runtime/rust/src/web/observability.rs:307` area) | request-isolation and session-isolation regressions |
| 10 | docs: explain page for the `RecursionLimit` kind | progressive teaching page per the compiler-as-teacher standard |

Interaction inventory (touched by behavior, not by edit): the emitted call
convention (plain `pub fn` items + `crate::`-qualified cross-module calls —
the shim slots in unchanged), `method_router`/`server_listen`
(`server.rs:637`/`:695` — containment via the existing layer),
`drive_session` (`web/mod.rs:612`), `task_parallel` both variants
(`task.rs:534`/`:588`), the std-only and current-thread `block_on` entries
(`task.rs:126`/`:205`), and `TailLoop` emission (unchanged, exempt by
construction).

## 8. Implementation plan (ordered increments, test-first, each lands green)

Golden re-blessing is automated; byte churn never reshapes an increment.
Every increment ends with the full gate: workspace tests, the clippy
deny-set, golden E2E (`ipe` exit-0 ⇒ `cargo build` green ⇒ run-output
match), the SEALs, the wasm and `--no-default-features` builds.

1. **Runtime guard primitive + classification (runtime-only; nothing emitted
   calls it yet).**
   *Failing tests first (runtime unit tests, `core.rs`):* a driver that
   calls `recursion_guard()` in a recursive helper trips at exactly the
   limit with the fixed message; the RAII decrement balances across both
   normal return and a caught unwind (depth returns to zero after
   `catch_unwind`); `IPE_RECURSION_LIMIT` override honored, garbage/zero
   falls back to the default; `classify_panic("maximum recursion depth
   exceeded")` → `"RecursionLimit"` and an overflow-containing message still
   maps to `"ArithmeticOverflow"` (extend `core.rs:1196`); a probe test on a
   deliberately small `thread::Builder` stack with a recorded floor trips
   via the red zone *below* the depth limit; a floor-less thread runs
   depth-only.
   *Change:* touch points 1–2.
   *Gate:* full (runtime lib incl. `--no-default-features` and wasm cfg
   compile).

2. **Stack normalization + floor recording on every runtime-owned thread.**
   *Failing tests first:* a runtime test asserting the floor is recorded on
   the `block_on` entry thread and on tokio workers (via a task that reads
   the TLS); existing suite green (behavioral non-regression — larger stacks
   only).
   *Change:* touch points 3–5.
   *Gate:* full.

3. **Emission prologue + shim + goldens (the DoS repro becomes a clean
   error).**
   *Failing tests first:* golden `recursion_limit_trip` — a mutually
   recursive pair with no reachable base case (non-tail, so the `TailLoop`
   rewrite cannot absorb it), expected: exit 1, stderr carrying the
   classified `RecursionLimit` line, **no abort** (today this fixture
   SIGABRTs); golden `recursion_normal_depth` — a correct non-tail recursion
   ~1 000 deep returns the right value byte-identically (non-regression: the
   guard never fires on working programs); a `let`-bound-local-recursion
   probe fixture (§10, first bullet); assert the existing
   `tests/golden/mutual_recursion` and tail-call goldens still pass with the
   prologue line present (`backend/rust/tests/tail_call.rs` body-slice
   expectations updated).
   *Change:* touch points 6–7; mass golden re-bless.
   *Gate:* full CI-replica (backend lib, golden suite, static-emit E2E,
   SEALs, wasm, no-default-features, clippy/fmt).

4. **Server request isolation E2E.**
   *Failing tests first:* an Ipe.Http.Server app with one runaway-recursive
   handler — request A returns the 500 body with a `ref`, a concurrent
   request B to a healthy route completes normally, request C after the trip
   is served (listener alive); assert the response body never contains the
   panic message.
   *Change:* test-only (the mechanism landed in 1–3).
   *Gate:* full.

5. **Live-session isolation E2E.**
   *Failing tests first:* an Ipe.Tea.Web app whose `update` recurses
   unboundedly on one Msg — the tripping session's driver ends, the process
   stays alive, and a fresh session (new page load) works; the client
   session-lost recovery path is exercised.
   *Change:* test-only.
   *Gate:* full.

6. **(Optional, benchmark-gated) instrumentation narrowing.**
   *First:* a representative micro/meso benchmark (call-heavy workload) of
   the uniform prologue. Proceed only if the cost exceeds ~1–2%.
   *Change:* restrict the prologue to functions in a named-call-graph SCC
   **plus** every function whose value escapes as data (the conservative
   closure of value-mediated recursion), with a written soundness argument
   and adversarial fixtures (Y-combinator-style value recursion must still
   trip).
   *Gate:* full, plus the new adversarial fixtures.

7. **Docs.**
   *Change:* the `RecursionLimit` explain page (progressive, jargon-gated);
   a short "recursion limits" note in the language docs stating the default,
   the env override, and the tail-call exemption.
   *Gate:* full + docs-sync sweep.

## 9. Risks and cost

- **Golden churn (large, cheap).** Every emitted function gains one line;
  the re-bless is automated and explicitly not a design factor.
- **Hot-path overhead.** One inlined TLS access + two compares per user
  call; `TailLoop` iterations exempt. Bounded, measured in increment 6, with
  a designed narrowing path that never weakens soundness.
- **Foreign threads.** Emitted code invoked on a thread the runtime did not
  spawn (FFI callbacks) runs depth-only (no floor). The depth budget still
  contains unbounded recursion there; only the fat-frame backstop degrades.
  Acceptable: the FFI boundary already has its own hardening track.
- **Main thread under a reduced `ulimit -s`.** The floor recorded for the
  process main thread assumes the 8 MiB platform default (§3.4); the size is
  not queried, because the only portable query is a `getrlimit`/`pthread` FFI
  call and the runtime holds the line at exactly one sanctioned `unsafe` block.
  A synchronous program (the std-only `block_on`, which polls on the main
  thread) run under a soft stack limit *smaller* than 8 MiB therefore has a
  floor calibrated too low and a depth budget (10 000) that may not fit the
  reduced stack, so a runaway recursion there can still overflow before either
  bound trips. Every runtime-*spawned* thread is unaffected — its 8 MiB stack
  is requested explicitly and survives a lowered soft limit — so servers, live
  sessions, and the async-entry `block_on` are contained regardless. Closing
  the synchronous-main-thread case for a shrunken `ulimit` needs the real stack
  bound, i.e. the `getrlimit` FFI; deferred rather than trade away the
  no-new-`unsafe` invariant.
- **Red-zone sufficiency.** The 256 KiB margin must cover one frame plus the
  deepest unguarded callee chain (§3.4 contract). Runtime kernels are
  bounded; the FFI margin is an explicit deferred verification (§10).
- **Classification order fragility.** The new `classify_panic` arm must stay
  ahead of the `"overflow"` substring arm; the extended kind-map unit test
  pins the ordering.
- **Unwind-through-`drive_session`.** The trip ends a live session rather
  than answering it; recovery rides the existing session-lost client path.
  If product feedback wants a friendlier in-page error, that is a separate
  UX design on top of the same containment.
- **The guard is itself on the panic path.** `recursion_guard()` and the
  `Drop` impl must be total (no allocation on the trip path beyond the
  panic itself, no unwrap); the increment-1 unit tests exercise the guard
  under `catch_unwind` to pin it.

## 10. Decisions deferred to implementation

- **Local `let`-bound recursive functions.** Whether the language admits a
  recursive local binding that emits as a closure-only cycle (never
  re-entering a named `Func` item) is settled empirically by the increment-3
  probe fixture. If such a cycle exists, the prologue is additionally
  planted at the lowerer-marked recursive lambda bodies; if the construct is
  rejected upstream, no action.
- **Log-line detail.** Whether the server-side classified log line carries
  the tripped depth and active limit (the client-visible surfaces never do).
- **Per-app configuration.** Whether the limit ever joins an app-level cfg
  record; env-only until a concrete need appears.
- **FFI red-zone verification.** Measure worst-case foreign-frame depth
  beneath a guarded entry before declaring the 256 KiB contract satisfied
  for FFI-using programs.
- **Narrowing threshold.** The exact benchmark and regression budget that
  green-lights increment 6, and whether it ships at all.
