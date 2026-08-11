# IR interpreter for `ipe run`

The design for strategy S6 of ADR 0054:
`ipe run` executes the lowered `ipe_ir` program directly, removing `rustc`
from the dev loop. `ipe build`, `--release`, and every FFI-bearing program
keep the rustc AOT path unchanged. This is the only strategy that reaches the
true-milliseconds dev-loop budget — the measured warm rebuild is stuck at
~2.4 s because the per-edit cost is the rustc recompile itself, which no
amount of dependency gating touches.

Governing order, as everywhere: Security > Correctness > Soundness >
Efficiency > Completeness > Readability. Efficiency is the goal here, and the
whole design is shaped so it never buys Efficiency by weakening a higher
principle — hence the mandatory differential oracle (Correctness), the
shared-kernel binding (Correctness), the no-panic bar (Soundness), and the
shared jail decision point (Security).

## Shape of the change

| Surface | Engine |
|---|---|
| `ipe run` (dev, no FFI, supported program) | **interpreter** — new default once gated |
| `ipe run` with `Rust.` FFI anywhere in the program | AOT (fail-closed fallback, loud) |
| `ipe run --engine=aot` | AOT (explicit escape) |
| `ipe build`, `ipe build --release`, `ipe exec`, wasm | AOT / existing paths, untouched |
| `ipe watch` | interpreter-backed restart loop (same supervisor) |

## Architecture

### Tree-walking interpreter, not a bytecode VM

**Recommendation: tree-walking evaluator directly over `ipe_ir`.** Reasons,
in principle order:

- **Correctness.** A bytecode VM is a second lowering — a third semantics
  surface (source → IR → bytecode) whose compiler and loader can each drift.
  A tree-walker evaluates the *same* IR the Rust backend emits from, so the
  differential surface is exactly one artifact. Smaller drift surface wins.
- **Efficiency is already met.** The budget is won by deleting rustc from the
  loop, not by interpreter dispatch speed. `ipe_ir::Expr` is small (~30
  variants) and already heavily pre-digested by the lowerer: tail recursion is
  rewritten to `TailLoop`/`TailRecur` loops, pattern matches are flattened to
  `Match` + arm guards, closures carry explicit capture-clone rewrites
  (`CloneVar`, `SharedLambda`), partial application is eta-expanded. Real
  programs spend their time inside kernels (`ipe_runtime` does the work), and
  the interpreter calls those natively. A hello world interprets in
  microseconds either way.
- **Readability/maintenance.** One engine artifact, no instruction set to
  version.

The bytecode/VM option stays open as a later refinement behind the same
differential gate, adopted only if profiling of real interpreted programs
shows walk overhead dominating — nothing in this design precludes it.

### Crate and evaluation model

A new workspace member `src/compiler/interp` (crate `ipe_interp`).
Dependencies: `ipe_ir`, `ipe_kernels`, `ipe_diagnostics`, `ipe_intern`, and
`ipe-runtime-rust` (the vendored runtime *as a linked crate* — it is already a
workspace member with feature flags). It inherits the workspace lint wall
(`unwrap_used`/`expect_used`/`panic`/`indexing_slicing` all `deny`), so the
runtime's no-panic soundness bar applies to the interpreter structurally, not
by convention.

Evaluation:

- Input is the linked `ipe_ir::Program` — the same value the backend consumes,
  taken from the same pipeline, **in process**. It is NOT taken from the disk
  cache: `src/ipe-cli/src/cache.rs` deliberately does not persist
  `ipe_ir::Program` (it caches the emitted-project strings), because every
  `ipe_intern::Symbol` embedded in the IR is a raw index into the current
  process's interner — meaningless in another process. In-process interpretation
  (P1–P2) needs no serialisation at all. Crossing a process boundary (the jailed
  child, the watch child) requires the **portable-IR encoding** below.
- Functions resolve by `FuncId`; the entry is `Module::entry`. Environments
  are lexical scopes mapping `Symbol → Value`; `Let`/`Destructure` push
  bindings, `Lambda`/`SharedLambda` capture by value (the lowerer's
  `CloneVar` rewrite tells the interpreter exactly where a capture read must
  clone — the same instruction it gives the backend).
- The evaluator `match`es **exhaustively** over `Expr` (no wildcard arm).
  Every variant either evaluates or returns a typed `InterpError::Unsupported`
  that routes to the AOT fallback — never a panic, never a skip.
- Semantic anchors come from the backend, not invented: each `Expr` variant's
  doc comment in `src/compiler/ir/src/ir.rs` states what the backend emits,
  and the interpreter implements that meaning. Load-bearing examples:
  - **Integer arithmetic wraps.** The emitted profile sets
    `overflow-checks = false` in both dev and release
    (`src/compiler/backend/rust/templates/Cargo.toml`,
    `src/compiler/backend/rust/src/project.rs`), so `BinOp::Add/Sub/Mul` on
    `i64` are two's-complement wrapping. The interpreter uses
    `wrapping_add`/`wrapping_sub`/`wrapping_mul` — anything else is a
    differential red on the overflow fixtures.
  - **`BinOp::IntDiv` calls the same total helper** the backend routes
    through: `ipe_runtime::math::ipe_int_div` — no reimplementation.
  - **Float equality is `f64::PartialEq`** (NaN ≠ NaN), matching the derived
    `PartialEq` in emitted code.
  - **`Match` arms try in order**, guards fall through — native Rust `match`
    semantics, which the interpreter reproduces by ordered trial.
  - **`TailLoop`/`TailRecur` evaluate as a loop** rebinding parameters — the
    interpreter gets stack-safe self-recursion for free from the lowerer's
    TCO rewrite, same as the emitted code.
  - **`CallPin` defaults** (`::<i64>`, `::<String, i64>`, `::<IpeError>`) pin
    types that are observably inert (discarded / empty / phantom positions);
    the interpreter's uniform `Value` needs no pin, and the differential gate
    proves the inertness claim rather than assuming it.

### Portable IR — the process-boundary encoding

The jailed-child hook (P3) and the watch restart child (P5) both hand a
`Program` to another process. `cargo build` of nothing does not make that free:
the IR's `Symbol`s are process-local interner indices, so a naive serde dump
deserialises into garbage names in the child. The required piece is a
symbol-relocating encoding — serialise every embedded `Symbol` as its resolved
string; on load, re-intern each string into the loading process's interner and
rewrite every occurrence. That is a total walker over every `Symbol`-carrying
site in `ipe_ir::ir` (`Var`, `CloneVar`, `Access`, record field keys, `FuncSig`
params/generics, `EnumDef`/`TypeDef` fields, …) plus full serde coverage across
the IR types — real, bounded work `src/ipe-cli/src/cache.rs`'s module doc
already sizes honestly, and it is a **named prerequisite of P3 and P5**, not a
corner to cut. It is proven the same way everything here is proven: encode →
decode in a fresh process → the differential suite over the decoded program
must stay byte-identical. Re-interning is linear in distinct symbols —
single-digit milliseconds for real programs, measured (not assumed) in P5's
logged loop metric.

### Value representation

One uniform `Value` enum, concrete and closed (house rule: concrete over
generic, never `dyn Any`):

- Scalars mirror the emitted representations: `Int(i64)`, `Float(f64)`,
  `Bool(bool)`, `Str(String)`, `Char(char)`, `Unit`.
- Composites: `List(Vec<Value>)`, `Tuple(Vec<Value>)`,
  `Record(BTreeMap<Symbol, Value>)`, `Ctor { home, ty, variant, args }` for
  user enums, plus the built-ins at their runtime carriers instantiated at
  `Value`: `IpeMaybe<Value>`, `IpeResult<Value, Value>`,
  `HashMap<Value, Value>` (Dict), the Set carrier likewise.
- `Closure(Rc<ClosureVal>)` — params, body reference into the shared
  `Rc<Program>`, captured environment. Application is a pure function of
  (closure, args), so a closure can be wrapped into a plain Rust
  `move |v: Value| -> Value` closure to hand to a generic runtime kernel.
- `Task(IpeTask<IpeError, Value>)` — the runtime's own alias
  (`Pin<Box<dyn Future<Output = IpeResult<E, A>> + Send>>`,
  `src/runtime/rust/src/core.rs`), instantiated at `Value`.
- `Opaque(OpaqueVal)` — a **closed** enum of the concrete runtime types that
  flow through kernels without structure the IR sees: `Decimal`, `Bytes`,
  `Time`, `Url`, `Json`, `Html<Value>`, `Key`/`Mac`, `FormData`, …. A runtime
  type not yet listed makes its kernels `Unsupported` → AOT fallback. Closed,
  so dispatch stays exhaustive.

Equality and hashing: `BinOp::Eq` uses a structural `ipe_eq` with
float-`PartialEq` semantics. `Value` as a Dict key / Set element needs
`Eq + Hash` **and `Ord`** — the runtime's dict kernels take `K: Ord` and sort
every iteration surface by key (`dict_keys`/`dict_values`/`dict_to_list`/
`dict_foldl`, `src/runtime/rust/src/dict.rs`; the Set carrier is a `BTreeSet`),
so the interpreter's `Value` ordering must reproduce the concrete key type's
ordering exactly or Dict iteration order silently diverges. That is total by
construction, and it is the **compiler front end** — shared by both engines,
before any IR exists — that makes it so, not rustc failing later:

- The type checker admits only the scalar set `Int`/`Float`/`Char`/`String`/
  `Bool` for the Dict-key / Set-element obligation
  (`src/compiler/types/src/lib.rs`, `concrete_super_ok` /
  `emitted_bound_satisfied`); functions, tuples, records, and lists are
  rejected at typecheck, fail-closed on unresolved variables.
- The lowerer then rejects the one checker-admitted-but-unhashable case,
  `Float` keys/elements, with the dedicated `IPE-L0117` diagnostic
  (`reject_float_keyed_collection` and the `ir_type_from_ty` Dict/Set arms in
  `src/compiler/lower/src/lower.rs`) — never deferring to a cargo failure,
  per the SEAL.

So key positions reaching the interpreter are homogeneous scalars whose
`Value` `Ord`/`Eq`/`Hash` delegate to the inner `i64`/`char`/`String`/`bool`
impls — identical order and hash-set semantics to the emitted concrete maps.
The impls still cover every `Value` variant totally (`f64::total_cmp` for the
float arm, discriminant order across variants, closure identity never
compared) so an impossible cross-variant comparison is defined, not a panic —
unreachable by the front-end argument above, total anyway.

Cloning starts naive (deep `#[derive(Clone)]`). Sharing optimisations
(`Rc`-backed lists/records) are Efficiency work admitted later only under the
differential gate; the dev loop budget does not need them on day one.

### Kernel binding — the equivalence lever

**The interpreter never reimplements a kernel.** It links `ipe-runtime-rust`
and calls the *same functions* the emitted code calls. This collapses the
hardest 90% of the dual-semantics problem: every `Io.println`, `String.*`,
`Json.*`, `Crypto.*`, `Db.*` behaves identically in both engines because it
*is* the same machine code.

One boundary on the "same machine code" claim: the runtime is compiled
per-feature-set. A pure emitted crate links the runtime **without** the
`tokio` feature, so the kernels with a `#[cfg(not(feature = "tokio"))]`
counterpart (`task.rs`, `time.rs`, `system.rs`, `file.rs`, `csv.rs`,
`config_decode.rs`) run a *different body* in the pure binary than the one the
tokio-linked interpreter calls. For those kernels the identity argument
degrades to "audited twin implementations", and the authoritative differential
tier is precisely what keeps the twins honest — it runs the interpreter's
tokio-feature bodies against the pure binary's std bodies on every pure
fixture. A registry test enumerates the `tokio`-cfg-gated kernel entry points
so a new twin cannot appear without joining the audited set.

Dispatch is one exhaustive `match` over `KernelFn`
(`ipe_kernels::StdlibKernel`, ~930 wired variants) producing a shim per
kernel:

- **Generic kernels instantiate directly at `Value`.** The runtime is written
  generically: `dict_insert<K: Hash + Eq, V>`,
  `list_foldl<T0, T1>(f: impl Fn(T0, T1) -> T1, …)`, `Html<M>`,
  `task_and_then<E, A, B>` — all take `Value` (and
  `impl Fn(Value) -> Value` closures wrapping interpreter closures) without
  per-element marshalling.
- **Monomorphic kernels marshal at the boundary**: unwrap `Value::Int` to
  `i64`, call, wrap the result. A marshal mismatch is unreachable on
  well-typed IR; it still surfaces as a typed internal error (fail-closed),
  never a panic.
- **Task-returning kernels** produce `IpeTask<IpeError, T>`; the shim maps the
  result into `Value` via the runtime's own `task_map`.
- The exhaustive `match` is the drift tripwire: adding a `StdlibKernel`
  variant without deciding its interpreter story is a compile error. A kernel
  may be decided as `Unsupported` — but a registry test asserts the
  `Unsupported` set equals the documented fallback ledger, so coverage can
  only shrink deliberately, never rot silently.
- The mapping source of truth already exists: `StdlibDecl::emit` names the
  runtime symbol per variant, and each shim is checked against it in review;
  the differential corpus checks it in fact.

### Driving Task futures

The interpreter reuses the runtime's entry drivers verbatim
(`src/runtime/rust/src/task.rs`):

- The program's entry Task is driven by `block_on` on the shared global tokio
  runtime — the same entry the emitted reactor program uses (spawned-thread
  poll, panic mapped to `Err` through the redacting funnel).
- The webview entry shape uses `block_on_current_thread`, mirroring the
  emitted main's main-thread requirement.
- The emitted **pure** program links a tokio-less std park/unpark `block_on`
  instead — a cargo *feature* choice the interpreter binary cannot replicate
  per-program (it links tokio once, for the reactor programs). For the
  observables the oracle defines — stdout, asserted stderr, exit code of a
  well-typed program — the drivers are equivalent: the same sequenced effects
  fire in the same order (a pure future resolves on first poll under either
  driver, `KernelFn::requires_async_runtime` fail-closed guarantees no reactor
  op is reachable), and the same `main` epilogue maps `Ok`/`Err` to the same
  exits. The differential corpus (every pure golden runs under both engines)
  proves it continuously. Two residual differences are real and stated:
  - **Panic path.** The tokio driver polls on a spawned thread and
    `.join()`-maps a panicking future to `Err` through the redacting funnel
    (`src/runtime/rust/src/task.rs`, tokio `block_on`); the std driver polls
    on the calling thread and lets a panic propagate to the entry boundary's
    synchronous-panic classifier. Both exit non-zero; the stderr text can
    differ. A panic is unreachable from well-typed Ipê (the no-panic bar), so
    this divergence is observable only on a compiler/runtime bug — and the
    oracle, which runs well-typed programs, cannot prove it away. It is a
    documented, bug-only divergence, not a gate waiver.
  - **Driver-thread stack.** The tokio driver's spawned thread has a smaller
    default stack than the pure binary's main thread. Since the interpreter's
    evaluation runs under that driver, it spawns its driver thread with an
    explicit `std::thread::Builder::stack_size` and calibrates the evaluator
    depth guard against that configured size — the R3 envelope is set against
    the real stack, never the platform default.
- The `uses_async_runtime` flag (`ipe_ir::Module`, fail-closed: unknown ⇒
  reactor) stays authoritative for the *emitted* entry; the interpreter reads
  it only for parity of shape selection (webview vs default).

## Correctness — the differential oracle

Without byte-exact agreement between engines, the interpreter is a
Correctness violation (two semantics for one language). The oracle is
therefore **mandatory, blocking, and lands in the first phase** — before any
kernel breadth.

**Definition.** For every program in the corpus: same source, same stdin, same
argv ⇒ interpreter and emitted-Rust binary produce **byte-identical stdout,
byte-identical asserted stderr, and identical exit codes**.

**Corpus.**

- All `tests/golden/*` fixtures (512 directories; the ~217 with an
  `expected.txt` form the behavioural tier, and every new golden joins by
  directory discovery — no opt-in list to forget).
- The example suites (`examples/shapes`, `examples/sky/ipe`), which exercise
  whole-program shapes the goldens slice thinly.
- Regression fixtures under `tests/regression` as applicable.

**Mandatory fixture classes** — discovery alone does not guarantee the corpus
exercises the semantic surface the two engines can disagree on; each class
below is asserted present (a corpus-coverage test fails if a class is empty):

- **Wrapping arithmetic** at the boundaries (`i64::MAX + 1`, `MIN - 1`,
  `MIN * -1`, `IntDiv`/mod edge cases through `ipe_int_div`).
- **Dict/Set iteration order** — multi-entry `Dict.toList`/`keys`/`values`/
  `foldl`/`foldr` with `Int` and `String` keys: the sorted-key contract is the
  one place the interpreter's `Value: Ord` must reproduce concrete-key `Ord`
  exactly.
- **Float formatting** — `toString`/`String.fromFloat` over negative zero,
  subnormals, exponent-notation boundaries, NaN/∞ where expressible; both
  engines call the same runtime formatter, and these fixtures pin that it
  stays that way.
- **`CallPin` inertness, per variant.** The gate "proves the inertness claim"
  only if the corpus actually contains pinned calls. A coverage assertion
  lowers the corpus and requires at least one fixture producing each
  non-`None` `CallPin` variant (`DefaultI64`, `DefaultDict`,
  `DefaultResultMapErr`, `ErrIpeError`) — e.g. a discarded `List.head`, an
  empty `Dict.empty` queried for size, a `Result.mapError` whose `Ok` is
  discarded — each printing its surrounding observables. Without this, the
  inertness proof is vacuously green.
- **Match ordering and guard fallthrough** — overlapping arms, guards that
  fail into later arms, nested constructor/list patterns.
- **TCO** — `TailLoop`/`TailRecur` fixtures deep enough to overflow without
  the rewrite, plus deep NON-tail recursion (the R3 envelope probe, via
  `build_and_run_stack_limited`).
- **argv/stdin** — programs reading `System` args and stdin: the interpreter
  must present the program's argv (the `--` tail), never `ipe`'s own.
- **Clone discipline** — captured-then-mutated-copy shapes where a missed
  `CloneVar` would alias.

**Exit codes** compare between the raw engines (interpreter process vs the
emitted binary), not through the `ipe run` wrapper: the wrapper's non-Unix
path collapses a nonzero child exit into its own exit 1
(`src/ipe-cli/src/lib.rs`, `run_run`'s non-Unix branch), which would mask a
code mismatch. On Unix the wrapper `exec`s and propagates exactly; the
interpreter exits with the program's code so the wrapper-level behaviour
matches per platform.

**Harness.** A differential test family beside the existing goldens in
`src/ipe-cli/tests/`, reusing the proven support pieces
(`support::build_and_run_emitted`, `e2e_support::read_expected`,
`build_and_run_emitted_with_stdin`, `build_and_run_stack_limited`). Two
tiers:

1. **Cheap tier — always on** (plain `cargo test`, every PR): run each
   supported fixture under the interpreter, byte-compare against the blessed
   `expected.txt`. No cargo build of emitted projects, so it is fast enough
   to be unconditional.
2. **Authoritative tier — the E2E lane** (`IPE_E2E=1`, same CI lane that
   builds and runs the goldens today): additionally build + run the emitted
   binary and byte-compare **interpreter output against binary output
   directly**. This tier catches a stale `expected.txt` and any engine pair
   drifting together, and is the definition of the gate.

**Gate policy.**

- Any mismatch is red. No waivers, no tolerance windows, no
  normalisation beyond what the existing golden harness already applies.
- A fixture the interpreter cannot run must appear in the explicit fallback
  ledger; the harness asserts the ledger matches the `Unsupported` kernel
  set. An unexplained skip is itself a failure — the corpus cannot silently
  shrink.
- The gate blocks the interpreter becoming (or remaining) the `ipe run`
  default. `--engine=aot|interp` pins an engine explicitly; a mismatch report
  prints the first divergent byte offset with surrounding context from both
  engines.

**Scope of comparison.** Only defined-deterministic observables. Fixtures
already avoid asserting wall-clock, randomness, or unordered concurrent
completion order in `expected.txt`; the oracle inherits that discipline. It
does not compare timing or memory.

## Soundness and Security

**Soundness.** The interpreter is subject to the identical bar as the
runtime: no panic, no unwrap/expect, no unchecked indexing, no
overflow-abort — enforced by the workspace clippy denies it inherits, by the
panic-scan, and by the guardian review that any language-boundary work
already requires. Interpreter-internal failure is a typed `InterpError`
diagnostic. One genuinely new hazard is **evaluator stack depth**: an
interpreter frame is much larger than a native frame, so deep non-tail
recursion could overflow the OS stack where the binary survives. The
evaluator carries an explicit depth guard that surfaces a typed
resource-limit error well before OS stack death (never a SIGSEGV), and the
differential corpus includes deep-recursion fixtures
(`build_and_run_stack_limited` exists for exactly this class) so the
practical envelope is measured, not guessed.

**Security.**

- **No cargo/rustc on the interpret path** — that is the strategy itself.
  Interpreting an untrusted `.ipe` source strictly *shrinks* the attack
  surface versus today's `ipe run`: no emitted-project build, so no
  build-time code execution surface (build scripts, proc-macros) is reachable
  from source text. The FFI fallback is the one path that re-opens the build,
  and it flows through the existing fail-closed consent machinery — never a
  silent build.
- **Jail parity.** The jail is scoped to native-bearing programs
  (`Capability::NativeFfi` / `FfiRaw` — ADR 0040,
  `src/ipe-cli/src/run_sandbox.rs::is_native_bearing`); pure Ipê runs
  directly under its structural capability bound. The engine branch is placed
  **after** capability resolution in `run_run` (`resolve_for_run` → union →
  `is_native_bearing` → *then* choose engine), so the jail decision is
  computed once and applies to whichever engine runs. That placement is a
  re-ordering, not just an insertion: today `run_run` resolves capabilities
  *after* the emit + `cargo build` steps (the jail wraps only the final
  exec), and the interpret branch must skip the build entirely — so
  `resolve_for_run`/`is_native_bearing` hoist ahead of the build.
  `resolve_for_run` depends only on the manifest and entry, so the hoist is
  sound; P3's first failing test pins the new order (capabilities resolved
  before any cargo invocation). Today the two scopes
  coincide: every native-bearing program is FFI-bearing and therefore falls
  back to AOT, so the interpreter only ever runs programs the AOT path runs
  unjailed — and the same structural bound holds, because the interpreter can
  reach effects only through the same kernel implementations behind the same
  capability inference. The invariant is stated and tested, not assumed:
  **dev execution is never less confined than production.** If the jail's
  scope ever widens beyond native-bearing, the interpreter branch re-executes
  itself as a jailed child (`ipe` internal interpret subcommand through
  `run_jail_argv` / `exec_in_run_jail` with the same `SandboxProfile`) — the
  subcommand exists from the watch integration anyway, so the hook point is
  already built (`run_jail_argv`/`exec_in_run_jail` live in
  `src/compiler/sandbox/src/run_jail.rs`; the child consumes the portable-IR
  artifact — its process-boundary prerequisite).
- **`Debug.*`** stays a development affordance exactly as today
  (`production = false` on the run path); the production rejection gate is
  untouched because `ipe build --release` never enters the interpreter.

## Scope and fail-closed fallback

The decision is **whole-program and pre-execution**: before evaluating
anything, one pass over the linked `Program` (every `Func` body) collects
every `Callee` and kernel. If anything is outside the supported set, the
*entire* run uses the AOT path. There is no mid-run fallback — a program that
half-executed under one engine and restarted under another would double-fire
side effects.

Always AOT, permanently or until designed otherwise:

- **`Callee::Ffi`** (derived or asserted) and any `uses_ffi` module — a
  `Rust.` crossing is arbitrary foreign machine code; it is not interpretable
  and must not be stubbed. Fail-closed: fall back to the full AOT pipeline
  (jail included), never skip or fake the call.
- **`--release`** and `ipe build` — AOT is the product for artifacts.
- **wasm targets** — the browser bundle path is a different backend entirely.
- **Static-plan builds** (`+crt-static` targets) — an emit/link concern.

AOT until its phase lands (the shrinking ledger): kernels whose shim is not
yet written — initially the heavy app shapes (Server, TEA/Web, WebView,
Terminal full-screen, Db) — each an explicit `Unsupported` arm, each listed
in the ledger the harness asserts.

Fallback is **loud**: one stderr line naming the engine and the reason
(`falling back to compiled engine: program uses Rust.Http (FFI)`), so the
dev-loop speed cliff is always explained, and CLI docs never promise
interpretation for programs that cannot have it.

## `ipe watch` integration

Today's loop (`src/ipe-cli/src/watch.rs`, the salsa-aware orchestrator over
the `ipe_watch` primitives): coalesced fs events → salsa input mutation →
`compile_prepared` under `salsa::Cancelled::catch` on a worker thread → emit →
`cargo build` → supervisor restart of the binary (INV-3: a failing rebuild
never kills the running app; last-good respawn on a failed readiness probe).
The front half is already incremental; the back half (emit + cargo) is the
non-incremental 2.4 s that swamps it.

The interpreter replaces exactly the back half:

1. File change settles (existing debounce).
2. Salsa recompiles incrementally through lowering — parse → canon → types →
   lower, warm-database milliseconds for a leaf edit; this is where the
   salsa investment finally pays end-to-end, because nothing non-incremental
   follows it anymore.
3. The lowered `Program` is written to the run dir in the **portable-IR
   encoding** (the symbol-relocating serialisation above — the one genuinely
   new infrastructure piece this loop depends on). The write is atomic
   (temp file + rename) and generation-stamped: the supervisor passes the
   exact per-generation artifact path as the child's argv, so a restarting
   child can never load a torn or half-superseded file, and the last-good
   artifact is simply the previous generation's file left in place.
4. The supervisor restarts its child — which is now
   **`ipe` (internal interpret subcommand) pointed at that IR artifact**
   instead of `target/debug/ipe-app`.

The supervisor is deliberately process-shaped and engine-agnostic
(`SupervisorState` supervises "a child process"), so INV-3, readiness gating,
last-good respawn, and SIGTERM forwarding are reused **verbatim** — the
interpreter child is just a different argv. A child process is preferred over
an in-process evaluation thread: a wedged or looping program can be killed
cleanly, a crash cannot take the watcher down, and the jail hook point wraps
a child naturally. Process spawn plus portable-IR load (decode + re-intern) are
expected single-digit milliseconds — an expectation P5 measures and logs, not
assumes; re-interning is linear in distinct symbols.

Expected loop: **edit → settle → front-end (ms, salsa-warm) → restart
interpreter child (ms)** — tens of milliseconds edit-to-running, versus ~2.4 s
today. The last-good artifact for H15/H16 recovery is the last-good
generation's IR file. State leak across restarts: none beyond today's — the
child is a fresh process either way, so restart semantics (model reset,
sockets closed, env re-read) are byte-for-byte the current watch behaviour.

Out of scope here, deliberately: **state-preserving hot-swap** (a long-running
TEA/server child keeping its model while swapping IR). Restart semantics
match today's watch exactly; live state migration is a separate design with
its own correctness story, and nothing in this loop precludes adding it.

## Implementation plan — test-first, each phase landable and green

Each phase states: goal → the failing test written first → minimal
implementation → gate. Every phase ends with workspace build, clippy, and
nextest green; kernel-coverage phases additionally end with the differential
tiers green.

**P1 — evaluator skeleton + the differential harness.**
Goal: prove the loop on the pure-kernel floor, with the oracle in place from
day one.
Failing test first: the differential harness itself — fixture discovery over
`tests/golden`, engine-vs-`expected.txt` compare, and the fallback-ledger
assertion; it fails because no interpreter exists.
Minimal impl: `ipe_interp` crate; `Value`; evaluation of the data/control
`Expr` variants; shims for the pure floor (`Io`, `String`, `Math`, `List`,
`Maybe`/`Result`, `Char`, `Dict`/`Set` basics); the CLI branch behind
`--engine=interp` (default unchanged).
Gate: every pure-floor golden passes both differential tiers; every other
fixture is asserted `Fallback`; no panic paths (clippy wall + panic-scan).

**P2 — Task/async execution.**
Goal: effectful programs — Task chains, `File`, `Time`, `Http` client,
`Random`, `Process`/`System` — under the runtime's own drivers.
Failing test first: async goldens run under `--engine=interp` (they currently
assert `Fallback`).
Minimal impl: `Value::Task`; entry via the shared tokio `block_on` (webview
shape via `block_on_current_thread`); Task-combinator shims instantiated at
`Value`; `TaskSeq` evaluation.
Gate: async goldens differential-green in both tiers; ordering fixtures
(sequenced effects, early-`Err` short-circuit) byte-identical.

**P3 — capability and jail parity.**
Goal: the engine branch provably sits downstream of capability resolution and
can never weaken confinement.
Failing test first: unit tests on the `run_run` branch order (capabilities
resolved before engine choice; native-bearing ⇒ AOT verdict), plus a jailed
child-exec test for the internal interpret subcommand under a
`SandboxProfile`.
Minimal impl: the portable-IR encoding (encode → fresh-process decode →
differential-suite green over the decoded program), the internal subcommand
(portable IR in, jailed-exec-capable), the capability-resolution hoist in
`run_run`, branch plumbing, fallback notice line.
Gate: existing sandbox/admission suites untouched and green; new parity tests
green.

**P4 — FFI fail-closed fallback.**
Goal: `Rust.` programs behave exactly as today, with the fallback visible.
Failing test first: an FFI golden under default `ipe run` asserts AOT
behaviour plus the notice; `--engine=interp` on it asserts a typed refusal
(never a stub call, never a skip).
Minimal impl: the whole-program pre-scan (`Callee::Ffi` / `uses_ffi` /
unsupported kernels) producing the engine verdict.
Gate: all FFI goldens byte-identical to their pre-interpreter behaviour.

**P5 — `ipe watch` on the interpreter.**
Goal: the near-instant edit→run loop.
Failing test first: an orchestrator test (existing watch-test style) asserting
the restart child is the interpreter subcommand and INV-3 holds across a
failing recompile.
Minimal impl: engine plumb into the watch orchestrator; atomic
generation-stamped portable-IR artifact (P3's encoding reused); readiness
probe unchanged.
Gate: full watch suite (including SIGTERM forwarding) green; measured
edit-to-restart — including the decode + re-intern cost — logged into
ADR 0054's measured table.

**P6 — corpus-wide parity and cutover.**
Goal: the interpreter as the `ipe run` default.
Failing test first: the full-corpus differential sweep — all golden fixtures
plus the example suites — which fails while the ledger is non-minimal.
Minimal impl: kernel-shim breadth (Db, Server, TEA/Web, Terminal, WebView —
each shape its own reviewable slice) until the ledger holds only the
permanent exclusions.
Gate: full differential suite blocking in CI; default flip to
`--engine=interp`-equivalent with `--engine=aot` as the escape; CLI help and
docs updated to promise exactly what ships.
Example-suite parity is two-tiered, matching what the examples infrastructure
actually is:
- Examples with a deterministic transcript (cli-shape programs with expected
  output) join the byte-differential harness on the `IPE_E2E` lane — the same
  blocking gate as the goldens.
- App-shaped examples (server/live/webview/tui) have no byte-comparable
  stdout; their RUN checks are boot/probe checks
  (`tools/scripts/examples-sweep.sh` + `tools/scripts/lib/checks.sh`). Their engine
  parity is an **engine matrix dimension in the sweep harness itself** — the
  same per-shape probe must go green under `--engine=interp` — and it
  inherits the examples-sweep lane's status: CI-only (push + nightly, never
  PR-gating) and non-gating until that lane flips gating. The *blocking*
  P6 cutover gate is therefore the golden corpus plus transcript-bearing
  examples; the probe-level sweep is a second, wider net, honestly labelled
  as such.

## Honest risks, costs, and the no-go conditions

**R1 — dual-semantics drift (the top risk, permanent).** Two execution
engines must agree forever; every backend change and every new kernel now has
two consumers. The kernel-binding design removes kernel *bodies* from the
drift surface, but the semantics encoded in **emitted shapes** remain to be
mirrored by hand: wrapping arithmetic, clone/borrow discipline, match order
and guard fallthrough, `CallPin` defaults, `OnFormKind` dispatch, capture
rules. That list is finite and enumerated above, each pinned by differential
fixtures — but a future emitted-shape change that forgets the interpreter is
exactly one missed review away, and only the blocking differential CI stands
between that and a silent dual semantics. This risk never amortises to zero;
it is the standing tax of S6.

**R2 — async observable divergence.** Both engines run the same tokio and the
same combinators, so ordering of *sequenced* effects is safe. Unordered
constructs (`Task.parallel` completion order, timer coalescing) can resolve
differently under interpreter overhead. Contained by comparing only
defined-deterministic observables — but that is a discipline on fixture
authors, not a mechanical guarantee.

**R3 — stack-depth envelope.** Interpreter frames are an order of magnitude
larger; a deep-recursion program can succeed compiled and hit the typed depth
limit interpreted. This is a *visible, typed* divergence (fail-closed, and
the depth limit can be generous), but it is a real behavioural difference the
docs must state and the deep-recursion fixtures must measure. The evaluation
thread is spawned with an explicit stack size and the guard calibrated to it,
so the envelope is a chosen constant, never a platform default.

**R4 — jail-scope coupling.** The parity argument leans on ADR 0040's
"jail scoped to native-bearing". If that posture ever widens, the interpreter
branch must move in lockstep; P3 encodes the coupling as a shared decision
point plus tests, but it remains a cross-subsystem invariant to maintain.

**R5 — compiler-binary weight.** The `ipe` binary now links the runtime with
its interpreted-surface features (tokio included), growing the compiler's own
build time and size. Mitigable by a cargo feature for compiler-dev builds; the
shipped binary pays it.

**Cost estimate.** P1+P2 are the long poles (evaluator, Value design, harness,
~hundreds of pure/async shims); P3 carries the portable-IR encoding (the
symbol-relocation walker over every `Symbol`-carrying IR site — the piece the
build cache deliberately declined to build, sized in
`src/ipe-cli/src/cache.rs`'s module doc); P6 is a long mechanical tail (~930
kernel variants total, macro-assisted, in shape-sized slices). Realistic
total: a multi-week, multi-lane effort, with the differential harness the very
first landable artifact — it has standalone value (it hardens the existing
golden corpus) even if S6 stops there.

**The standing tax, enumerated** (what "permanent second engine" costs every
year it exists, independent of the build cost above):

- every emitted-shape change (arith, clone discipline, match/guard shape,
  `CallPin`, `OnFormKind`, capture rules) now needs an interpreter mirror and
  a reviewer who knows to ask for it;
- the `tokio`-cfg twin-kernel set must stay audited as it grows;
- `Value`'s `Ord`/`Eq`/`Hash` totality and its scalar-delegation parity are
  live invariants;
- the authoritative differential tier is a permanent E2E CI lane (build + run
  the corpus twice);
- the `ipe` binary permanently links the interpreted-surface runtime (R5).

**When S6 is not worth it.** The fair no-go cases:

- If S3 (precompiled runtime crate) + S2 (shared target) land first and bring
  the warm `ipe run` to ~0.5 s, the marginal win is ~0.5 s → ~30 ms. If the
  team judges half a second an acceptable dev loop, a permanent second engine
  with a standing drift tax is a bad trade — the requirement doc says
  milliseconds is a hard budget, and that premise is exactly what a go/no-go
  should re-affirm or drop.
- If the differential gate cannot be kept *blocking* (flaky fixtures, CI
  cost), S6 must not ship at all — a non-blocking oracle is dual semantics
  with extra steps, and Correctness precedence rejects it.
- If interpreted coverage stalls (ledger stops shrinking mid-P6), the value
  proposition inverts: most real programs (server/web apps) would fall back
  anyway, and the interpreter would serve only the programs that already
  build fastest.

**Recommendation.** Conditional go: land P1 (evaluator floor + blocking
differential harness) as the committed spike — it is cheap, reversible (a new
crate plus a non-default `--engine` flag; deleting both restores today
exactly), and its harness pays for itself regardless — and take the
full-breadth decision after P2 on THREE inputs, not one:

1. P2's measured interpreter numbers (is the ms loop real for effectful
   programs?);
2. the observed drift rate — how many backend changes landed while the gate
   was live, and how many the gate caught (the standing tax, measured instead
   of feared);
3. a fresh warm-loop measurement of the AOT path with whatever of S2/S3 has
   landed by then. If that number is at or under ~0.5 s and the team accepts
   half a second, the fair verdict is NO — the milliseconds premise was the
   justification, and re-affirming or dropping that premise is exactly what
   this decision point is for. Stopping after P1 is a designed outcome, not a
   failure: the corpus-hardening harness remains.

## Guardian review — verified and corrected claims

An independent security-soundness review of this design against the cited
code. Resolutions are folded into the sections above; this ledger records the
verdicts so the next reader knows which claims were checked in code, not
trusted.

**Verified in code:**

- The four pin-checked flags: pure-driver equivalence (both `block_on`
  entries in `src/runtime/rust/src/task.rs`; equivalence holds for the
  oracle's observables, with the panic-path and driver-stack caveats now
  stated in the Task-driver section); Dict-key admissibility (checker scalar
  set in `src/compiler/types/src/lib.rs` + the lowering `IPE-L0117` Float
  rejection in `src/compiler/lower/src/lower.rs` — front-end-enforced, pre-IR,
  shared by both engines); the `CallPin` doc contract
  (`src/compiler/ir/src/ir.rs`); the examples-sweep shape
  (`.github/workflows/examples-sweep.yml`).
- Jail scoping and hook points: `is_native_bearing`/`resolve_for_run`
  (`src/ipe-cli/src/run_sandbox.rs`), `run_jail_argv`/`exec_in_run_jail`
  (`src/compiler/sandbox/src/run_jail.rs`), ADR 0040.
- Watch supervisor reuse: INV-3, last-good respawn, readiness bifurcation,
  SIGTERM forwarding all present in `src/ipe-cli/src/watch.rs` as described.
- Corpus counts (512 golden directories, 217 with `expected.txt`), wrapping
  profile (`overflow-checks = false` in the emitted profile), `ipe_int_div`,
  `StdlibDecl::emit`, `uses_async_runtime`'s fail-closed default.

**Corrected by review** (the design above already reflects these):

- The lowered IR is **not** "already cached and serde-portable":
  `src/ipe-cli/src/cache.rs` persists emitted-project strings precisely
  because `Symbol`s are process-local. The portable-IR encoding is a named
  prerequisite of P3/P5, with its own encode/decode differential proof.
- `Value` needs `Ord`, not just `Eq + Hash`: the dict kernels' sorted-key
  iteration contract (`src/runtime/rust/src/dict.rs`) and the `BTreeSet` Set
  carrier make ordering an observable — covered by the new sorted-iteration
  fixture class and the scalar-delegation argument.
- Key admissibility is enforced by the shared front end (typecheck + lowering),
  not by "the emitted Rust would fail cargo" — the latter would itself be a
  SEAL violation and is not what happens.
- The kernel-identity claim is per-feature-set: pure emitted crates link
  no-`tokio` twin kernel bodies. The twin set is enumerated, registry-tested,
  and exercised by the authoritative tier.
- The engine branch requires hoisting capability resolution in `run_run`
  ahead of the build steps (today it runs after them).
- Example-suite parity splits into transcript-bearing (blocking,
  byte-differential) and probe-level (sweep engine matrix, non-gating with its
  lane) — the sweep performs no output comparison today, so P6 could not have
  ridden it as a byte oracle.
- `CallPin` inertness is proven only if pinned calls exist in the corpus:
  the per-variant coverage assertion closes the vacuous-green hole.
- Exit-code comparison must target the raw engines: the `ipe run` wrapper's
  non-Unix branch collapses child exit codes.

**Review verdict.** The conditional go stands, with the go/no-go inputs
sharpened above: P1 is genuinely cheap, reversible, and independently
valuable; the differential oracle, with the added fixture classes and
coverage assertions, is sufficient to hold the dual-semantics line — and if
it ever cannot be kept blocking, the design's own no-go clause governs.
