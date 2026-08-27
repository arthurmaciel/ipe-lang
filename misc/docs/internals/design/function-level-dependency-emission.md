# Function-level dependency emission

Design: push the emitted-project dependency gating from module/kernel-family
granularity down to **function reachability**, so an emitted program compiles a
runtime module's crates only when a function of that module is actually
reachable from the program's entry. Companion to
ADR 0054 (this is the completion of S1 and the
finer-grained feature model S3 consumes) and to
`precompiled-runtime-and-shared-target.md`.

## 1. Security first: why this is not an efficiency feature

The precedence (`PRINCIPLES.md`) is Security > Correctness > Soundness >
Efficiency > Completeness > Readability. This design is argued at rank 1, not
rank 4: **every dependency a program does not need but still compiles and
links is attack surface** — supply-chain exposure (one more crate whose
compromise ships into every user binary), latent code an exploit can pivot
into, and `cargo-deny`/audit scope the user pays for a capability they never
invoked.

What a program that never touches `Auth.*` / `Db.*` / `Http.*` still gets
today (base template `src/compiler/backend/rust/templates/Cargo.toml`, all
unconditional):

| Shipped anyway | Subtree | Capability it embodies |
|---|---|---|
| `bcrypt` | ~14 crates (blowfish stack) | password hashing |
| `sha2`, `hmac`, `subtle`, `zeroize`, `getrandom` | ~10 | crypto core + OS entropy |
| `uuid` | ~3 | id generation |
| `chrono` | ~7 | calendar arithmetic |
| `regex` | ~5 | regex engine (ReDoS-relevant surface) |
| `rust_decimal` | ~11 | decimal arithmetic |
| `serde` + `serde_derive` + `serde_json` + `serde_urlencoded` | ~7 | serialization + proc-macro build-time execution |
| `base64`, `hex`, `percent-encoding`, `unicode-general-category` | ~4 | codecs |

The heavy roots (`rsa`, `url`/idna, `jsonwebtoken`, `reqwest`, `sqlx`,
`tokio`, `chrono-tz`) are already usage-gated. This design extends the same
security argument to the residual floor: a `Program`-shape CLI that prints a
report deserves a binary containing a printer, not a password hasher.

The counter-obligation is equally rank-ordered: the reachability analysis must
be **sound** — dropping a dependency the program needs is a Correctness break
(exit-0 emit, failing `cargo build`: THE SEAL breach class). Every
approximation below therefore errs toward *including*; the SEAL machinery
(§7) proves fail-closure mechanically.

## 2. Current model (verified in code)

### 2.1 Granularity today: module-set OR of whole-module scans

- The lowerer scans **every function body of every module** in the program and
  ORs kernel-family flags into `ir::Module.uses_*`
  (`scan_kernel_usage`, `src/compiler/lower/src/lower.rs:6155`; the family
  record `KernelUsage::record`, `lower.rs:6088`; the flag docs on
  `ir::Module`, `src/compiler/ir/src/ir.rs:50`).
- `EmitCtx::build` ORs those per-module flags across all modules
  (`src/compiler/backend/rust/src/lib.rs:1219` ff.), then derives transitive
  runtime-module closures: `reaches_http_client` (lib.rs:1408), `reaches_jwt`
  (:1421), `reaches_crypto_core_heavy` (:1440), `reaches_url` (:1458).
- Consequence: a **dead** helper in any imported module that mentions
  `Http.get` pulls `reqwest` into the binary. Module discovery (the import
  graph from `Main`, `src/ipe-cli/src/project.rs`) already shakes *unimported
  modules*; nothing shakes unreachable *functions*.

### 2.2 The floor: unconditional prelude references

The emitted `main.rs` carries a kernel-wrapper prelude sliced from the golden
(`runtime_bindings`, `src/compiler/backend/rust/src/project.rs:975`) that
hard-references `ipe_runtime::{error, core, task, log, system, time, random,
file, io, crypto_core, json, path}` for **every** program — see the wrapper
block in `src/compiler/backend/rust/templates/main.rs:49-259`. The base
`templates/ipe_runtime/mod.rs` therefore declares ~36 modules unconditionally,
and the base `Cargo.toml` ships the crate floor of §1. Exactly one prelude
section is conditional today: the `Http` block, cut by
`native_runtime_bindings(uses_http_client)` (`project.rs:1000`) — the
existing proof that a conditional prelude works.

### 2.3 Existing attribution infrastructure

- `crate_specs.rs`: crate-version SSOT + manifest drift tripwire.
- The runtime feature-map SSOT `runtime_features(&EmitCtx) ->
  RuntimeFeatureSet` (`src/compiler/backend/rust/src/runtime_features.rs`):
  a closed `RuntimeFeature` enum whose variants ARE the runtime crate's
  declared feature universe, plus the `runtime_featureset_closure` SEAL — an
  exhaustive flag-mask sweep proving every selected feature exists, resolves
  closed, and cfg-satisfies every emitted `ipe_runtime::<mod>::` reference.
  **This SSOT is where function-level attribution plugs in.**
- The only call-graph shaking in the compiler today is the FFI wrapper DCE
  (`src/compiler/ffi/src/instance.rs`). There is no general
  reachable-from-entry analysis; this design adds one.

### 2.4 Shapes and `package.ipe` (verified)

The shape is derived from the **program**, never from config: the app-entry
kernel `main` reaches sets `uses_web` / `uses_tui` / `uses_webview` /
`uses_tea`, which select the emitted entry (`emit_web.rs` / `emit_tui.rs` /
`emit_webview.rs`); a program reaching none is the plain **Program** shape
(the `epilogue()` entry, `src/compiler/backend/rust/src/preamble.rs:81`).
`AppShape` (`src/compiler/diagnostics/src/diagnostic.rs:850`) names the four
TEA entries (Web / TerminalScreen / TerminalLines / WebView) for the Model
gates.

`package.ipe` (`ProjectManifest`, `src/ipe-cli/src/project.rs:40`) carries:
name/version, `Package.database`, static build / allocator knobs,
`Package.wasm`, `Package.dependencies`, `Package.rustDependencies`,
`Package.declares`.

**Confirmed: no manifest field adds a crate the program's code does not
reach.** Config *parameterizes* dependencies the reachable call graph already
demands; it never *introduces* one:

| Field | Effect on dependencies | Active when |
|---|---|---|
| `Package.database` | selects `db-sqlite` vs `db-postgres` alias set | program reaches a `Db.*` kernel (any shape — a Program-shape CLI may use Db) |
| `Package.rustDependencies` | pins version/features of FFI crates | program calls `Rust.*` (`Callee::Ffi`) |
| `Package.allocator` | adds `dlmalloc`/`mimalloc` | explicit opt-in, shape-independent |
| `Package.wasm` | switches target/manifest template; `Wasm.publicEnv` fills `env_public.rs` | wasm target; `Env.public` reachable |
| `Package.declares` | none — verification metadata against the inferred set | — |

This is stronger than the assumed model ("TEA shapes add config-side deps"):
today **no** shape has a config-only dependency channel. The `redis_store`
runtime feature exists in the runtime crate's `[features]` but no manifest
field selects it (session-store selection is a runtime concern). Rule going
forward: **any future capability key (e.g. a session-store driver) must enter
through the `runtime_features` SSOT as an explicit config-parameterized
feature, TEA-shapes only, and be listed in this table** — never as an
unconditional manifest append.

## 3. Attribution model

Dependencies become a pure function of the reachable call graph, composed as
(illustrative dataflow, not a runnable block):

```
entry ─→ reachable user functions ─→ reachable kernels
      ─→ needed runtime modules (kernel→module map ∪ runtime-module dep closure)
      ─→ RuntimeFeatureSet (runtime_features SSOT)
      ─→ crates (manifest features / augmenters)
      ─→ prelude wrappers (emit a module's wrappers only if its module is needed)
```

### 3.1 Reachability over the IR call graph

A worklist pass over `ipe_ir` from `Module.entry` (plus the shape's TEA
callbacks — `update`/`view`/`subscriptions`/route handlers are entry roots for
TEA shapes):

- `Callee::Func(id)` in a `Call` **or** `FuncValue` position adds an edge. A
  function whose value is reified (`FuncValue`, stored in a record/enum
  carrier, passed as an argument) is reachable — no escape analysis, the
  conservative edge is the sound one.
- `Callee::Kernel(k)` on a reachable body records `k` into the usage record —
  the existing `scan_kernel_usage` traversal, now run **only over reachable
  bodies** instead of all bodies.
- `Callee::Ffi` keeps its fail-closed behaviour unchanged (forces
  `uses_async_runtime`, pulls the bound crates).

Precision note: this is call-graph reachability, not liveness — a reachable
function with a dead branch still contributes its kernels. That is the correct
(sound) side to land on; branch-level pruning is out of scope.

### 3.2 Kernel → runtime-module map, total

`KernelFn::required_runtime_module` exists but covers only two exceptional
routings today. It becomes a **total** map (every `KernelFn` names its home
runtime module), exhaustiveness-tested, so "needed modules" = the image of the
reachable kernel set, unioned with:

- the runtime-module dependency closure — module A's `use crate::B` demands B.
  This closure already exists as the modset SEAL's source walk
  (`tests/runtime_modset_closure.rs`); it is promoted from test-only lint to
  an emit-side SSOT table with the SEAL re-aimed as its drift guard;
- the shape's own modules (a Web program needs `web`/`server` regardless of
  direct calls — exactly today's `reaches_*` unions, now derived from the
  reachable kernel set instead of module flags).

### 3.3 Conditional prelude

The monolithic golden-sliced prelude becomes **sectioned by runtime module**,
generalizing the shipped `Http`-section mechanism (`native_runtime_bindings`):
each section (log wrappers, system, time, random, file, io, crypto_core, task
combinators, json aliases) is emitted iff its module is in the needed set.
End state: the prelude is generated per-section from the kernel registry
rather than sliced from one golden, so a section cannot reference a module the
attribution did not select — the wrapper and the manifest derive from the same
set and cannot disagree (the featureset SEAL's cfg-satisfaction check is the
mechanical proof).

### 3.4 Plug into the `runtime_features` SSOT

`runtime_features(&EmitCtx)` keeps its shape. Two changes:

1. The `EmitCtx` predicates it reads become reachability-derived (§3.1)
   instead of whole-module ORs — the SSOT body does not change for this step.
2. The `RuntimeFeature` universe gains floor variants as the runtime crate
   splits its unconditional floor (§5, §6): `Log`, `TimeCore`, `Decimal`,
   `Regex`, `Uuid`, `Encoding`, `CryptoCoreFloor`, … Each new variant pairs
   with a `[features]` entry in `src/runtime/rust/Cargo.toml` and a
   `cfg(feature)` gate on the module — the closed-enum property keeps
   "select a feature the crate does not declare" unrepresentable.

## 4. Shape rules

| Shape | Entry roots for reachability | Dependency formula | Config role |
|---|---|---|---|
| **Program** (non-TEA) | `main` | reachable functions only | parameterizer only (driver alias iff Db reachable, FFI pins, allocator opt-in). **No capability surface.** |
| **Terminal** (`appScreen`/`appLines`) | `main` + `init`/`update`/`view`/`subscriptions` | reachable functions + `tui` shape modules | same parameterizers |
| **WebView** | as Terminal + webview backend | reachable functions + `webview`→`web`→`server` closure | same parameterizers |
| **Web** | as Terminal + route/handler roots | reachable functions + `web`→`server`→`http_client`→`url` closure | same parameterizers; future TEA-only capability keys (e.g. session-store driver) enter here via the SSOT |

Two invariants to encode as tests:

- **Program-shape purity**: a `package.ipe` with `Package.database
  Package.postgres` and no reachable `Db.*` kernel changes nothing in the emitted
  manifest (already true; becomes a pinned invariant).
- **TEA closure exactness**: a TEA shape adds exactly its documented module
  closure beyond the reachable set, nothing else.

## 5. The reducible floor

### 5.1 What collapses

Base-module → crate attribution (from the runtime sources'
external imports):

| Base module(s) | Crate(s) | Gate after this design |
|---|---|---|
| `json`, `stringify`, `core` (JSON half) | `serde`, `serde_json` | a Json/Decoder/stringify-of-structured kernel reachable |
| `log` | `chrono` | a `Log.*` kernel reachable |
| `time` | `chrono` (+ `chrono-tz` already gated) | a `Time.*` kernel reachable |
| `string` (regex half), `regex_kernel` | `regex` | a regex-backed kernel reachable |
| `uuid_kernel`, `random` | `uuid`, `getrandom` | a `Uuid.*`/`Random.*` kernel reachable |
| `encoding`, `bytes` | `base64`, `hex`, `percent-encoding` | an encoding kernel reachable |
| `money`, `decimal` | `rust_decimal` | a `Decimal`/`Money` kernel reachable |
| `crypto_core` | `sha2`, `hmac`, `subtle`, `getrandom` | a crypto-floor kernel reachable (constant-time compare etc. stay in-module, std-only parts split out) |
| `secret` | `zeroize`, `subtle`, `bcrypt` (dummy-hash timing path) | `bcrypt` moves behind auth/db attribution; `zeroize` stays with `secret` |
| `char_kernel` | `unicode-general-category` | a `Char.*` category kernel reachable |
| (base manifest) | `bcrypt` — unconditional today (`project.rs:606`) | `Auth.*`/db-auth reachable only |

### 5.2 The irreducible minimum

What a do-nothing Program genuinely needs: the entry scaffold
(`install_panic_classifier`, `block_on` sync executor), `error` (`IpeError`),
`core` (result/kernel plumbing), `task` (the sync half), `stringify`
(`IpeStringify` derives on user types) — all std-only **once `core`/`error`
shed their serde coupling** (their serde impls become feature-gated with
`json`; today `core`/`error`/`basics` import `serde`, which is the last
obstacle to a zero-external-dep floor).

### 5.3 Projected crate counts

Baseline: hello world measured at ~51 crates after the current usage-driven
floor; hand-trimmed true floor measured at 1 crate / ~0.6 s cold
(ADR 0054, "Measured floor"). Projections below are
estimates from the subtree sizes in that doc; only the bare row is measured.

| Program | Today | Projected |
|---|---|---|
| bare Program (`Io.println`) | ~51 | **1–2** (app crate + runtime crate; zero external deps) |
| Log-only CLI | ~51 | ~10 (`chrono` + serde stack) |
| String/regex tool | ~51 | ~8 (`regex` + floor) |
| Db (sqlite) Program | ~85 | ~45 (sqlx subtree + json; sheds bcrypt/uuid/decimal/regex/chrono it never calls) |
| Web TEA app | ~105+ | ≈ today minus unused optional surfaces — a full app keeps what it uses |

## 6. Composition with S1/S2/S3

This design is S1 completed (the floor becomes usage-driven all the way down)
expressed in S3's vocabulary (features on one runtime crate):

- **S3 crate parity**: the runtime crate's `[features]` must mirror the finer
  floor split — every module in §5.1 gains a feature + `cfg` gate; the
  `full` feature list and the emitted-trimming parity tests extend
  accordingly. The precompiled-runtime cache key (S2/S3: version × toolchain
  × feature set) gets more distinct keys; the shared target amortizes them
  across projects, and feature-set canonicalization (`RuntimeFeatureSet` is
  ordered) keeps keys stable.
- **S6 (IR interpreter)** is orthogonal: `ipe run` may skip cargo entirely,
  but `ipe build` output is what ships — attack-surface reduction must hold
  on the AOT path regardless.

## 7. SEAL: fail-closed obligations

The invariant class to protect: **exit-0 emit ⇒ `cargo build` succeeds**, and
its converse hazard, **a needed module/crate is never dropped**.

- `runtime_featureset_closure` stays the guard and extends: the sweep's
  obligation (c) — every emitted `ipe_runtime::<mod>::` reference is
  cfg-satisfied by the resolved feature set — now also covers the sectioned
  prelude (each emitted wrapper section's references checked against the
  selected features). Dropping any kernel→module map entry or module-dep edge
  must turn the SEAL red (the drop-a-feature fail-closed proof, re-proven per
  new variant).
- Sweep cost: the exhaustive `2^N` mask sweep does not survive N ≈ 30.
  Replace with (a) per-feature closure proofs (each feature's reference set
  closed under its own resolution — linear), plus (b) exhaustive sweeps
  within kernel families, plus (c) the ground-truth `seal_modset` E2E cargo
  gate on representative fixtures. The union preserves the obligation without
  the exponential sweep.
- New obligation, **reachability soundness**: a differential test class —
  for each fixture, emit with function-level attribution and with the
  old whole-module ORs; the new crate set must be a subset, and `cargo build`
  + run must stay green on the new set. Any fixture where the subset relation
  breaks is an attribution bug by construction.

## 8. Implementation plan (test-first, independently landable)

Each phase: failing test first → minimal change → gate (workspace build +
clippy + modset/featureset SEALs + examples sweep). Golden re-bless is
expected at P2/P3/P5 (cheap, automated); the diff itself is the review
artifact.

1. **P1 — IR reachability pass** (`ipe_lower` or a sibling pass).
   *Failing test*: a program importing a module whose **unused** function
   calls `Http.get` asserts `uses_http == false` (today: true).
   Deliverable: `reachable_funcs(program) -> BTreeSet<FuncId>` +
   `scan_kernel_usage` restricted to it; `program_capabilities_scan` moves to
   the same reachable set (capabilities shrink identically — same soundness
   argument, same fail-closed FFI handling). Emitted output changes only
   where flags genuinely shrink; goldens with dead kernel callers re-bless.
2. **P2 — sectioned prelude.** *Failing test*: bare program's emitted
   `main.rs` contains no `log_*` wrapper; log program's does.
   Deliverable: per-module prelude sections behind the needed-module set,
   `native_runtime_bindings` generalized; golden re-bless.
3. **P3 — floor collapse.** *Failing test*: bare program's emitted
   `Cargo.toml` declares no `bcrypt`/`chrono`/`regex`/`uuid`/`rust_decimal`.
   Deliverable: base `templates/Cargo.toml` + `templates/ipe_runtime/mod.rs`
   shed each §5.1 row to a usage-gated append; `crate_specs.rs` absorbs the
   moved versions; modset SEAL `FLAG_COUNT` grows per new gate. Largest
   golden churn; land row-by-row (bcrypt first — highest security value).
4. **P4 — shape-rule formalization.** *Failing test*: the Program-shape
   purity and TEA closure-exactness invariants of §4.
   Deliverable: an `EmitShape` value derived once in `EmitCtx` (Program /
   Web / WebView / Terminal) as the SSOT the entry emitters and TEA closures
   read; the config-parameterizer table of §2.4 encoded as tests.
5. **P5 — SSOT + SEAL extension.** *Failing test*: removing one
   kernel→module map entry turns the featureset SEAL red.
   Deliverable: total `KernelFn -> RuntimeModule` map + module-dep SSOT;
   `RuntimeFeature` floor variants; per-feature closure proofs replacing the
   exponential sweep; the subset-differential test class of §7.
6. **P6 — runtime crate feature split** (with the S3 crate-parity work).
   *Failing test*: runtime crate builds with `--no-default-features
   --features <floor>` for each new floor feature alone.
   Deliverable: `cfg(feature)` gates on §5.1 modules, `core`/`error` serde
   decoupling; re-measure §5.3 and record the numbers in
   ADR 0054.

## 9. Risks and cost

- **Under-inclusion (the one forbidden failure).** Hazard sites: function
  values stored in carriers and called indirectly (covered: `FuncValue` is an
  edge), TEA callbacks not syntactically called (covered: shape entry roots),
  runtime-internal cross-module calls (covered: module-dep closure), FFI
  opacity (covered: unchanged fail-closed). The subset-differential tests and
  the SEAL make a miss loud before it ships.
- **Over-inclusion** is the accepted precision loss (reachable-but-dead
  branches). It only costs efficiency, never security relative to today.
- **Generics/monomorphization**: reachability runs on the IR call graph
  before Rust monomorphization; instantiation adds no call edges, so the
  analysis is unaffected. Generic function-value carriers resolve through the
  same `FuncValue` conservatism.
- **Prelude refactor**: replacing golden-slice anchors with generated
  sections is the riskiest mechanical step; anchors already fail loud
  (`CompilerBug` on drift), and the featureset SEAL's reference check guards
  the end state.
- **SEAL sweep growth**: addressed structurally in §7; do not ship a new
  floor gate without its closure proof.
- **Golden churn**: large across P2/P3/P5; re-bless cost is not a factor —
  correctness of the diff is reviewed, not its size.
- **`core`/`error` serde decoupling** (P6) is the only runtime-logic-adjacent
  change; everything before it is emitter-only, mirroring the property that
  made S1 cheap and principle-safe.
