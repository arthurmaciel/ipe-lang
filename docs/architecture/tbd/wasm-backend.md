# A direct WebAssembly backend for Ipê

> **Note:** the shipped client-WASM target this document contrasts itself with —
> the IR → Rust → `wasm32` route that *rejected* a direct IR → WASM backend — is
> now recorded in `docs/adr/0042-wasm-client-target.md`. In-text references below
> to `wasm-target.md` mean that ADR.
>
> **Status:** design proposal, pre-implementation. Sibling to the shipped
> client-WASM target, which chose IR → Rust → `wasm32` and *rejected* a direct
> IR → WASM backend. This document argues for building that rejected backend as
> an *additional, selectable* target, on the six principles, and specifies it in
> full. It does not retire the Rust-via-wasm path; it sits beside it.

## Executive summary

Lower the existing typed `ipe_ir::Program` **directly** to a WebAssembly module,
with **no rustc, no cargo, no wasm-bindgen, no wasm-opt** in the user's build.
The backend is a second `impl Backend` beside `RustBackend`, selected by a new
`Target::Wasm`. The whole front-end (parse → canon → typecheck → lower) is
reused unchanged; one new mandatory pass — monomorphization — precedes codegen.
The runtime is **reused, not reimplemented**: the ~52k-line Rust runtime is
compiled once at compiler-release time into a vendored `runtime.wasm` the
emitted module imports; only a small core is authored directly in WAT. Rust-crate
FFI is dropped and fenced by a default-deny target gate. Memory is **WASM-GC**.
Correctness is protected by a differential-conformance gate against the Rust
backend; validity by in-process module validation before any byte is written —
the direct analogue of THE SEAL.

## Why this exists — and why it is not the existing "wasm" work

Three distinct things are called "wasm" in this repo. Keep them apart:

1. **Compiler-to-wasm (`src/ipe-wasm`).** The compiler *front-end* compiled to
   wasm32 so parse→lower→emit runs in a browser and returns emitted **Rust
   text**. It cannot *run* Ipê programs — the emitted Rust still needs a Rust
   toolchain. Target of compilation = the compiler.
2. **Program-to-wasm via Rust (`Target::WasmClient`, `bundle_wasm`).** An Ipê
   program compiled to a browser bundle by emitting a Rust crate and shelling
   out to `cargo build --target wasm32-unknown-unknown` + `wasm-bindgen` +
   `wasm-opt` — three external processes. Target = wasm; **route** = IR→Rust→rustc.
3. **This proposal.** Target = wasm; **route** = IR→wasm **directly**. No rustc
   in the user's pipeline.

They compose: run (1) — front-end in wasm — together with *this* backend also
compiled to wasm, and you get a fully in-browser, offline, instant
compile-**and-run** playground, superseding both the server-compile and the
IR-interpreter options previously proposed. Shared across all three: the
front-end crate graph, the security target-gate, the DOM-sink semantics
(`Html<M>` → `diff() -> Vec<Patch>` → apply), and the Cmd/Sub browser bridge.

### The principled case

- **Security (1) — strengthened.** The artifact is eval-free
  (`script-src 'self' 'wasm-unsafe-eval'`, no JS `unsafe-eval`), `wasm2wat`-
  auditable, and capability-gated: a server effect or an FFI-bound crate is
  **unrepresentable**, not merely linted.
- **Correctness (2) — protected, not risked.** We do not re-derive kernel
  behaviour; the vendored `runtime.wasm` *is* the existing runtime, and a
  differential harness proves byte-parity with the Rust backend.
- **Soundness (3) — protected.** WASM-GC removes the use-after-free / leak / UB
  class a hand-rolled allocator would reintroduce; in-process validation makes
  an invalid module an unrepresentable output.
- **Efficiency (4).** No LLVM, no crate-graph compile: builds in seconds, and the
  true in-browser run needs no server round-trip.
- **Completeness (5) — a knowing, fenced regression.** Rust-crate FFI cannot
  exist here; the gap is a compile error, never a silent hole.
- **Readability (6).** Two emission paths cost maintenance — acknowledged
  honestly under Tradeoffs.

## Architecture

```
source → parse → canon(+target gate) → typecheck → lower → ipe_mono → WasmBackend.emit
                                                                          |
                                                        in-proc validate (wasmparser)
                                                                          |
                                                     link-closure check (imports satisfied)
                                                                          |
                                     write app.wasm / app.wat / runtime.wasm / app.js / index.html
```

`WasmBackend` implements the existing `Backend` trait
(`src/compiler/backend/src/lib.rs`) — the sole backend-agnostic contract — so it
depends only on `ipe_ir` and `ipe_diagnostics`, never the front-end.

### Monomorphization (`ipe_mono`)

There is no rustc to monomorphize generics for us, so we do it. A pass
specializes every generic `Func` at each concrete instantiation reachable from
`entry`, emptying all `Func::type_params` and replacing every `IrType::Generic`
with its concrete binding. This is the repo's concrete-codegen principle carried
to its endpoint. Bounded type variables (`BoundSet` = `Number` / `Comparable` /
`Appendable`) resolve to concrete kernel selection, so instantiation terminates
for the surface language. **Polymorphic recursion** — an unbounded instantiation
set — is detected and **rejected fail-closed** with a diagnostic; a
dictionary-passing escape hatch is deferred, never a silent fallback.

### Type lowering

Every value is monomorphic before codegen. Memory is **WASM-GC** (typed
`struct`/`array`, `i31ref`, reference types, subtyping, `externref`).

| Ipê / `IrType` | WASM representation |
| --- | --- |
| `Int` | `i64` |
| `Float` | `f64` |
| `Bool`, `Char` | `i32` (0/1; Unicode scalar) |
| `Unit` | empty result |
| `Str`, `Bytes` | `(ref (array i8))`, UTF-8 |
| `Tuple` | monomorphized GC `struct` |
| `Record` | GC `struct`, fields sorted by name (matches the IR `BTreeMap`) |
| `Enum` / `Maybe` / `Result` | variant-subtype family: `(struct (field i32))` tag supertype + one payload subtype per constructor |
| `List a` | immutable `(ref (array T))` snapshot; `Cons` builds a fresh array (O(n), matching the runtime's move-only `ipe_list_cons`) |
| `Fun` (closure) | `(struct funcref env…)`, called via `call_ref` on `(env, args…) -> ret` |
| `FnOnceChain` | nested one-shot closures, one struct per curry level |
| `Dict`, `Set`, `Task`, `Cmd`, `Sub`, `Decoder`, `Db`, `Json` | opaque handles backed by `runtime.wasm` / host |
| `Generic` | **erased** by `ipe_mono` — never reaches codegen |

`Match` lowers to `struct.get` of the tag, `br_table`, and `ref.cast` to the
arm's subtype. Closures use closure conversion; the abstract arrow at a use site
is `(ref $closureBase)` and each body downcasts its own env. `Expr::SharedLambda`
(the `Arc`-carrier case) is just a GC struct shared by reference — GC handles the
aliasing the Rust backend needed `Arc` for.

### Tail calls and the no-trap residual

`Expr::TailLoop` / `TailRecur` (the lowerer's existing TCO) → a wasm `loop` with
params as locals and `br`. Mutual tail recursion → the tail-call proposal
(`return_call` / `return_call_ref`) when the engine has it, else a trampoline.
**Residual:** non-TCO deep recursion can exhaust the wasm stack and trap — caught
and classified (`StackOverflow` diagnostic before instance death), not prevented.
This is the same residual `wasm-target.md` already records.

### Memory model — why GC

Ipê is a GC language; its object graph has cycles (closures capturing closures,
self-recursive ADTs). A linear-memory allocator plus a hand-written collector
would reintroduce the use-after-free / leak / UB class the no-panic contract
forbids — a Soundness (3) regression — and a cycle collector is a large,
error-prone subsystem. WASM-GC gives none of that: no manual free, cycles handled
by the engine, refs opaque and unforgeable. **Hybrid:** linear memory is used
*only* as a bump-arena scratch for marshalling byte buffers across the host
boundary (`(ptr,len)`), never for the managed graph. The rejected alternative —
linear memory + custom allocator + refcount/tracing — is universally supported
but buys portability with a soundness regression and months of work; it survives
only as the marshalling scratch. Cost: a minimum-engine floor (the GC proposal:
current browsers, `wasmtime`, recent Node) and `externref` glue for host handles
(a DOM node cannot be a GC struct).

## Runtime strategy — the dominant cost center

~52k lines of Rust across ~90 files under `src/runtime/rust/src/` back ~700
`StdlibKernel` variants (numeric/string/list/dict/set/json/decimal/regex
formatting, the Html render surface, TEA, …). Hand-porting to WAT would be a
multi-year Correctness disaster — byte-identical formatting (`Str.fromFloat`,
decimal, JSON) is guaranteed only by the existing impls. Layered strategy:

- **Authored WAT core (small).** GC-type registry, the linear-memory bump arena,
  checked-arithmetic helpers reproducing Rust's overflow/wrap semantics, the
  closure-dispatch trampoline, trap-guard shims. A few thousand lines.
- **Vendored `runtime.wasm` (bulk).** The existing Rust runtime compiled **once,
  at compiler-release time** — not in the user's pipeline, exactly as the Rust
  backend vendors runtime *source* compiled per build today — and shipped as a
  data asset. The emitted module imports its functions; an in-process linker
  (`wasm-encoder` / `wasmparser`) wires the two, or they ship as a two-module
  bundle the host links.

This keeps "no rustc/cargo in the pipeline" literally true while inheriting
proven semantics. **Honest residual:** `runtime.wasm` is rustc-*origin* — the
artifact is Rust-free in *pipeline*, not in *provenance*. A genuinely Rust-free
runtime (WAT reimplementation, kernel-by-kernel behind the differential harness)
is a deferred, opt-in track, never blocking MVP; the browser can also source some
security-critical kernels (crypto) from host imports (SubtleCrypto) instead.

## FFI — dropped loudly, by construction

`src/compiler/ffi/` binds **arbitrary Rust crates**: it inspects a crate, decodes
`PkgInfo`, and emits `<crate>_bindings.rs` wrappers plus `[dependencies]` lines
only rustc/cargo can resolve. There is no meaning for "link libpq / ring /
reqwest" without a Rust compiler. Therefore FFI is **dropped for this backend**,
but never silently:

1. The target gate (`src/compiler/canon/src/target_gate.rs`) makes `Callee::Ffi`
   **unrepresentable** under `Target::Wasm` — reaching an FFI-backed import is a
   typed compile error naming the crate and the incompatibility. Same
   default-deny structure as the existing `WasmClient` effect gate: absent proof
   an import is wasm-safe, reject.
2. The capability FFI provided is replaced, where a browser/WASI analogue exists,
   by **host imports** surfaced as ordinary kernels (crypto→SubtleCrypto/getrandom,
   http→fetch/WASI-http, storage→IndexedDB/WASI-fs) — never open-ended crate
   binding.
3. `Module::uses_ffi` and `Callee::Ffi` lowering are simply never produced for
   this target.

Net: Rust-crate FFI is a documented, enforced limitation; the Rust backend
remains the answer when a program needs it.

## Effects and the host boundary

No effect runs inside the module; every effect is an imported host function.
Two profiles, selected at build time.

- **Browser.** Imports for DOM mutation (create/remove/setAttribute/appendChild,
  delegated event-listener registration returning `externref` node handles),
  `console`, timers (`setTimeout`/`requestAnimationFrame`), `fetch`, `WebSocket`,
  `localStorage`/IndexedDB. The TEA loop reuses existing semantics: `init/update`
  produce `(Model, Cmd Msg)`; the runtime holds `Html<M>`, computes
  `diff() -> Vec<Patch>`, and the wasm patch-applier walks that `Vec<Patch>`
  calling DOM imports — one update+diff+patch per `requestAnimationFrame`.
  `Cmd.perform`→microtask; `Sub.every`/`Time.every`→timer imports;
  `Cmd.publish`/`Sub.subscribeTopic`→the in-tab broker.
- **CLI.** WASI (Preview 1, then Preview 2 / component model). `System` / `Io` /
  `File` / `Time` / `Random` / `Env` map to `fd_write`, `args_get`,
  `clock_time_get`, `random_get`, `path_open`.

**Crossing values:** scalars by value; strings/bytes copied into the linear-memory
bump arena as `(ptr,len)`; DOM/WebSocket handles as `externref` in a small glue
table so the host owns the real object and the module holds an opaque token.
Effect kernels compile **iff** a host analogue exists for the chosen profile;
otherwise unrepresentable, never a runtime stub.

## Tooling

`ipe build --target wasm` runs the full pipeline in a **single process** with no
external tool: parse → canon (target gate active) → typecheck → lower →
`ipe_mono` → `WasmBackend::emit`, then, before writing a byte:

1. **Validate** the module with `wasmparser` (full GC + reference-types
   validation). Failure => `CompilerBug` diagnostic, nothing written.
2. **Link-closure check**: every declared import is satisfied by the shipped
   `runtime.wasm` + host-glue surface. A dangling import cannot be emitted.
3. **Write** artifacts. Browser: `app.wasm`, `runtime.wasm`, generated `app.js`
   host-glue, `index.html`, `app.wat`. CLI: `app.wasm` (`wasmtime app.wasm`) +
   `app.wat`.

`wasm-opt` is optional and, if wanted, run in-process via the `binaryen` crate —
never a shelled binary. The existing Rust-via-wasm path is renamed
`--target wasm-rust` and retained as a fallback; the new direct path becomes the
default `--target wasm`.

## Correctness contract — the SEAL analogue

**If `ipe --target wasm` exits 0, the emitted `app.wasm` is a valid, link-complete
WebAssembly module whose observable behaviour, over the covered feature set,
matches the Rust backend byte-for-byte.** Three obligations, each enforced by
construction:

1. **Validity.** The module passes in-process `wasmparser` validation before any
   byte is written. Exit-0-then-invalid-wasm is the forbidden class — the direct
   analogue of exit-0-then-cargo-fail.
2. **Link-completeness.** Every import resolves against `runtime.wasm` + host-glue;
   a kernel without a wasm denotation is rejected at canonicalisation
   (default-deny gate), so no dangling import can be emitted.
3. **Behavioural conformance.** A differential harness runs `app.wasm` under
   `wasmtime` (CLI) / headless V8 (browser) against the Rust-backend binary on the
   examples corpus, comparing stdout / DOM-patch traces. Any divergence is a
   blocking red row.

The no-panic→no-trap contract carries for guarded kernels (checked arithmetic
reproduces Rust overflow semantics; guarded indexing keeps the trap class
unreachable), with the single honest residual: stack exhaustion from non-TCO
recursion, caught-and-classified, not prevented.

## Phased plan

- **Phase 0 — plumbing.** `Target::Wasm`; generalize `EmittedProject` to carry
  binary artifacts; `ipe_mono` skeleton; the in-process validator gate; target
  gate denies FFI + server effects for the new target.
- **Phase 1 — pure scalar core.** Funcs, arithmetic with Rust-parity overflow,
  `if`/`let`/`match` on nullary enums, TCO loops. Module validates and runs under
  `wasmtime`.
- **Phase 2 — heap types.** ADTs, records, tuples, strings, lists,
  closures/`SharedLambda` via WASM-GC. Monomorphization complete.
- **Phase 3 — runtime linking.** Build vendored `runtime.wasm`; in-process linker;
  wire pure + fallible-pure kernels; differential harness green on pure examples.
  **MVP (pure + CLI/WASI) lands here.**
- **Phase 4 — host and effects.** WASI CLI profile, then the browser DOM sink +
  TEA scheduler + Cmd/Sub bridge.
- **Phase 5 — hardening and polish.** Full capability matrix, `--target wasm`
  UX, optional in-process `wasm-opt`, docs, security review.

## New and changed modules

- `src/compiler/backend-wasm/` (new crate `ipe_backend_wasm`) — the `WasmBackend`
  impl: IR→wasm codegen, GC-type registry, closure conversion, import wiring.
- `src/compiler/mono/` (new crate `ipe_mono`) — monomorphization; polymorphic-
  recursion rejection.
- `src/compiler/backend/src/lib.rs` — generalize `EmittedProject` file values
  from `String` to a text-or-bytes artifact (or add a parallel `binaries` map),
  preserving the `RelPath` safety newtype.
- `src/compiler/kernels/src/lib.rs` — add `Target::Wasm`; extend
  `available_on` / `wasm_client_available` into a per-target capability matrix;
  attach a wasm-import descriptor per kernel.
- `src/compiler/canon/src/target_gate.rs` — deny `Callee::Ffi` and server effects
  under `Target::Wasm`.
- `src/runtime/wasm/` (new) — authored WAT core, the `runtime.wasm` build recipe,
  browser host-glue (`app.js` template), WASI host-glue.
- `src/ipe-cli/src/lib.rs`, `build_plan.rs` — route `--target wasm` to
  `WasmBackend`; drop the cargo/wasm-bindgen steps; add the validate + write path;
  rename the old path to `--target wasm-rust`.
- `docs/architecture/` — this document.

## Risks and mitigations

- **WASM-GC engine availability / cross-engine determinism** → pin a GC + tail-call
  feature floor; validate on `wasmtime` + V8; document the minimum-engine
  requirement; a clear diagnostic on unsupported engines.
- **Monomorphization blow-up / non-termination** → bound the instantiation set;
  reject polymorphic recursion fail-closed with a diagnostic; dictionary-passing
  deferred, never silent.
- **Runtime reimplementation correctness (the 52k-line trap)** → do NOT hand-port;
  vendor `runtime.wasm` and gate every future WAT kernel on the differential
  harness.
- **Behavioural drift vs the Rust backend** → a wasm lane in the existing examples
  sweep; divergence blocks.
- **no-panic→no-trap gaps** → checked arithmetic matching Rust semantics; the
  stack-exhaustion residual documented and classified.
- **Formatting parity** (float/decimal/JSON) → route through `runtime.wasm`, never
  hand-written.
- **Two emission paths to maintain** → the `Backend` trait keeps them isolated;
  the differential harness keeps them honest; the Rust path stays the default for
  native/FFI/opt-critical work.

## Tradeoffs — honest ledger

**Gains:** no rustc/cargo/wasm-bindgen/wasm-opt dependency; near-instant builds;
true in-browser compile-and-run when paired with the front-end-in-wasm; a smaller,
eval-free, capability-gated, auditable artifact (Security win); a hermetic
single-process pipeline. **Losses:** no Rust-crate FFI; we forgo rustc/LLVM's
optimizer (hot numeric code may lag until we invest in passes or run in-process
`wasm-opt`); a second codegen and no-panic contract to maintain — the fork
`wasm-target.md` warned against. **When the Rust backend still wins:** native
server binaries; any program using Rust-crate FFI; optimizer-critical workloads;
and the mature `wasm-rust` browser path while this backend's kernel coverage
climbs.

## Must-fix review

> Added by an independent adversarial architecture review. The specification
> above is preserved verbatim. The proposal is **NOT APPROVED** for
> implementation as written. This is not a rejection of a direct WASM backend in
> principle — the front-end reuse, the `Target::Wasm`-vs-`WasmClient` split, the
> FFI drop via the existing target gate, the WASM-GC choice *for the app*, and
> the validate-before-write SEAL analogue are all individually sound. It is a
> rejection of the plan's **central cost-bounding mechanism**, which is
> architecturally impossible as specified and takes the effort estimate, the
> Correctness-by-reuse thesis, and the MVP scope down with it.

### Blocker 1 (critical) — the vendored `runtime.wasm` cannot interoperate with the WASM-GC app

The plan independently chooses two memory models that **cannot exchange heap
values**:

- the app is **WASM-GC** (`(ref (array i8))` strings, GC-struct records/ADTs,
  GC-array lists, GC-struct closures), and
- the "bulk of kernels" come from a **vendored `runtime.wasm` compiled from the
  existing Rust runtime**, i.e. rustc → `wasm32-unknown-unknown`, which is
  **linear-memory only**. rustc/LLVM has no WASM-GC backend; a Rust runtime
  necessarily compiles to a private linear-memory heap managed by Rust's own
  allocator with unstable, unspecified struct layouts.

WASM-GC references cannot be stored in or passed through linear memory, and a
rustc-compiled function operating on a Rust `Vec<T>` / `HashMap<K,V>` /
`String` / `Arc<…>` in *its own* linear memory cannot receive a GC
`(ref (array …))` or GC struct. Therefore **every kernel that takes or returns a
heap value cannot be serviced by `runtime.wasm`** — which is the structural
majority of the kernel set. Verified against the tree: `pub enum StdlibKernel`
has **908 variants** (not ~700), and the runtime is **52,270 lines across 90
files**.

Marshalling does not rescue the general case. It works only for **flat scalar-in
/ bytes-out** kernels (e.g. `Str.fromFloat`: copy the runtime's linear-memory
result bytes into a fresh GC `array i8`). It fails for any kernel whose argument
or result is a *structured* Ipê value (List, Dict, Set, record, ADT, `Task`,
`Decoder`, `Json`), because that value would have to be deep-copied into the
runtime's private Rust layout and back — a layout that is not an ABI.

The spec's own claims collapse under this:
- "Dict/Set → opaque handles backed by `runtime.wasm`" is incoherent: a handle to
  a Rust `HashMap` living in the runtime's linear memory whose keys and values are
  themselves Ipê GC values cannot be populated — you cannot insert a GC-managed
  key into a linear-memory map.
- "route all formatting through `runtime.wasm`" works for `Str.fromFloat` but
  fails for `Json.encode` of a structured value, `List`/`Dict` formatting, etc.

**Required fix.** Either (a) reimplement the structured runtime in GC-compatible
WAT (the multi-year effort the spec explicitly disavows), or (b) abandon WASM-GC
for the app and adopt a **single shared linear-memory ABI** so the app and the
runtime live in one memory (the spec rejected linear memory on soundness
grounds, and two independently-compiled artifacts cannot share a Rust allocator
without a stable ABI and wasm-ld-level linking). Pick one explicitly and re-cost
the whole proposal around it. The "small hand-written core + vendored bulk"
middle path does not exist.

### Blocker 2 (critical) — higher-order and generic kernels cannot be prebuilt or called across the boundary

Independently of memory model, verified runtime signatures show the kernels are
**rustc-monomorphized generics that take Rust closures**:
`ipe_list_cons<T>(x: T, xs: Vec<T>) -> Vec<T>`,
`list_filter_map<A,B>(f: impl Fn(A) -> B, xs: Vec<A>)`,
`ipe_maybe_map<T,U>(m, f: impl FnOnce(T) -> U)`,
`dict_map<K,V,W,F>(f: F, d: HashMap<K,V>)`, `task_map`, `decode_map`, and many
more.

Two consequences the plan never reconciles:

1. **No symbol to import.** A prebuilt `runtime.wasm` can only export
   *monomorphic* functions for types known at *runtime-build* time. But `ipe_mono`
   invents fresh monomorphic types at *app-build* time (the app's records/ADTs).
   `ipe_list_cons::<AppGcStruct>` cannot exist in a release-time artifact — rustc
   never saw that type, and it is a GC type rustc cannot represent. So the
   monomorphization pass (the plan's own centerpiece) and the "import from a
   prebuilt generic runtime" strategy are **mutually contradictory**.
2. **No way to call back.** Higher-order kernels must invoke an Ipê closure. In
   the app that closure is a **GC funcref struct**; rustc-compiled `runtime.wasm`
   has no notion of the app's closure calling convention or GC element types and
   **cannot call it**. So `List.map`, `Dict.update`, `Maybe.map`, `Task.andThen`,
   decoders — the higher-order core — are unimplementable via the vendored
   runtime regardless of marshalling.

**Required fix.** State honestly which kernels are flat-scalar/bytes pure
(serviceable by a vendored runtime via arena marshalling) and which require
in-language (GC-WAT) reimplementation, and move the entire structured + higher-
order kernel surface into the *authored* column. That is the multi-year cost the
spec disowns.

### Blocker 3 (major) — effort estimate and MVP scope are mis-costed on the back of Blockers 1–2

"Large but bounded BECAUSE the runtime is reused, not reimplemented … MVP …
~4-5 months" is not real. Because the structured and higher-order kernels cannot
come from `runtime.wasm`, MVP-grade programs need them authored in GC-WAT first.
Worse, the **"pure + CLI/WASI" MVP is itself mis-scoped**: nearly every real Ipê
program — even pure ones — uses `List`/`Dict`/records/ADTs, whose kernels do not
cross the boundary. So the "differential harness green on pure examples"
milestone is only reachable for flat-scalar programs, not the pure corpus.

**Required fix.** Re-baseline the estimate around whichever resolution of
Blocker 1 is chosen, with the structured/higher-order kernel reimplementation on
the critical path and its own differential-conformance sub-plan, kernel family by
kernel family. Do not present runtime reuse as the thing that keeps MVP at
4-5 months.

### Minor corrections (non-blocking, fix in any revision)

- Kernel count is **908**, not ~700; runtime is **52,270 lines / 90 files**. Use
  the real numbers.
- The existing gate is `check_wasm_client` over the **canon AST**
  (`Expr_::ForeignCall` / `Expr_::VarKernel`), not over IR `Callee::Ffi`. The
  FFI-drop story is sound and already proven, but name the actual mechanism and
  layer.
- `EmittedProject.files` is `BTreeMap<RelPath, String>`; the String→bytes
  generalization is real and small, and must preserve the hand-written
  `RelPath` `Deserialize` (the cache trust boundary). Call that out.

### What would flip this to APPROVED

A revision that (1) picks one coherent memory story for the app↔runtime boundary
and proves a single heap value (a `List` of records, and one higher-order call
like `List.map`) can cross it end-to-end; (2) reclassifies the kernel surface
into "vendor-serviceable flat" vs "must author in-language" with counts; and
(3) re-costs the phased plan and MVP around that reclassification, with the
structured/higher-order reimplementation on the critical path. Until the
app↔runtime interop is demonstrated on one non-flat value, the proposal is not
buildable as scoped.
