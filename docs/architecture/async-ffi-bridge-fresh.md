# Async-FFI Auto-Bridge — FRESH design (Rust-first-principles arm)

> **Status:** design deliverable of the async-FFI double-swarm, FRESH arm.
> Written 2026-07-04 under the hard constraint of **zero reference-repo
> access** (`../sky` unread). Sources: our runtime
> (`runtime/src/sky_runtime/{core,task,http_client}.rs`), our vendored
> inspector (`tools/sky-ffi-inspect-rs/src/main.rs`), and the banked
> `docs/architecture/ffi-*.md` suite.
>
> **Goal:** fully-automatic Sky→Rust FFI for ASYNC crates — reqwest/tokio
> SDKs (Firestore, Firebase, Stripe) with builder-pattern fluent APIs —
> every usable async operation surfaced as a Sky function returning
> `Task Error a`, ZERO hand-written shims. Acceptance metric: skyshop-rs's
> firestore + stripe usage auto-bound.

---

## 0. Core mechanism (the whole design in five lines)

1. **`Task Error a` IS `Pin<Box<dyn Future<Output = SkyResult<E,A>> + Send + 'static>>`
   — an async foreign call needs no impedance layer at all**: the generated
   glue is `Box::pin(async move { spawn(foreign(owned_args).await) → map })`,
   exactly the shape our own hand-written reqwest kernel (`http_client.rs`)
   already uses.
2. **We never synthesize multi-await chains.** Every await point becomes ONE
   Sky binding; the builder chain is Sky's own `|>` + `Task.andThen`. The
   generator only ever emits four primitive wrapper shapes (§2).
3. **The crate's futures run on OUR executor** — one process-global tokio
   runtime (§4); `tokio::task::spawn(...).await` inside the pinned future
   gives free panic→`Err` (JoinError) and satisfies internally-spawning SDKs.
4. **Send is proven, not hoped**: the inspector's existing C1/C1b/C1c gates
   (output / captured-params / moved-receiver) stay the admission wall;
   anything unprovable is fail-closed dropped (over-drop, never under-bind).
5. **Clients and builders are opaque nominal handles** with a
   representation-selection algorithm (§6): `Clone` → plain value;
   `!Clone + &self`-only → `Arc<T>` (needs `T: Send + Sync` proof);
   `!Clone` consumed-by-value builders → **affine handle**
   (`Arc<Mutex<Option<T>>>`, take-once, structured `Err` on reuse).

---

## 1. Ground truth this design builds on

### 1.1 Our Task type is already the bridge target

```rust
// runtime/src/sky_runtime/core.rs:17
pub type SkyTask<E, A> = Pin<Box<dyn Future<Output = SkyResult<E, A>> + Send + 'static>>;
```

A `SkyTask` is a boxed, `Send`, `'static` future. A foreign `async fn` is a
future. The bridge is *composition*, not *translation* — the only work is
(a) ownership (owned args in, owned results out), (b) error mapping,
(c) panic containment, (d) executor discipline.

### 1.2 The hand-written precedent to automate

`runtime/src/sky_runtime/http_client.rs` is our own reqwest→SkyTask kernel:

```rust
pub fn http_get<E: From<String> + Send + 'static>(url: String) -> SkyTask<E, HttpResponse> {
    Box::pin(do_request(HttpRequest { /* owned config */ }))
}
async fn do_request<E: From<String> + Send + 'static>(req: HttpRequest) -> SkyResult<E, HttpResponse> {
    // ... reqwest builder chain, multi-await, every foreign Err routed
    // through sky_error_from_foreign (B8 two-level redaction) ...
}
```

Observations that generalize:

- **No spawn is *required*** — the reqwest future is polled inline by
  whatever drives the SkyTask (our `block_on` runtime, the server runtime).
  reqwest/hyper only need an *ambient* tokio context at poll time.
- **Owned-everything at the boundary** — `url: String`, `req: HttpRequest`
  by value. No lifetime ever crosses into Sky.
- **`sky_error_from_foreign`** (core.rs:54) is already the universal,
  security-vetted error funnel: any `Debug`-able foreign error → server-side
  log under a correlation id, generic `Err` to Sky. The generator reuses it
  verbatim (§7).

### 1.3 What the inspector already knows per function

From `tools/sky-ffi-inspect-rs/src/main.rs` (vendored, ours):

| Fact | Field / mechanism |
|---|---|
| Async-ness | `header.is_async` (+ `#[async_trait]` de-sugar WALL-4/#64, RPITIT #81), `is_future_type` on returns; `classify_effect` → `effect = "effectful"` |
| Typed params/returns | `Param { sky_type, rust_type }`, generic block's Call AST with `TypeRef` tree |
| Receiver | `recv_type` / `recv_rust_type`, `Receiver { arg, by: ref\|refmut\|value }` |
| Builder setters | `self_returning` (owned-threading `&mut self`→`Self` setters); by-value `self → Self` methods bind as plain functions |
| Borrowed-str params | `borrowAsRefArgs` (Wall-3b): Sky `String` → `.as_ref()` at the call site |
| serde generics | `SerdeValue` / `SerdeValueRef` reduction → JSON-text `String` + `methodTurbofish` (#72) for multi-generic methods (the firestore `get_obj<T, S>` shape) |
| Send proofs | `PROVABLY_SEND_RECV_NAMES` (explicit `impl Send` ∪ all-fields-Send ∪ Send-supertrait), `SEND_WHEN_ARGS_SEND_NAMES` (#87 conditional generic Send), gates C1 (output), C1b (params), C1c (moved receiver); unprovable → drop `async-future-not-send` |
| Locked deps + features | `transitive_deps` (WALL-B) + `features` (effective set, #100) |

**Conclusion:** the *admission* problem (which async fns are safely
bindable) is largely solved metadata-side. What this doc designs fresh is
the **generated-glue shape**, the **executor contract**, the **handle
representation algebra**, the **error rule**, and the **Stripe-scale DCE
pipeline** — i.e. everything downstream of the JSON.

---

## 2. The four primitive wrapper shapes (Q1 — signature transform)

Every public item the inspector admits maps to exactly one of four shapes.
Multi-await fluent chains **decompose** into sequences of these; there is no
fifth "chain" shape (§3).

### Shape P — pure/sync

`fn f(x: X) -> Y` → Sky `f : X' -> Y'`. Already designed (banked M-A..M-E);
unchanged. Includes by-value builder setters `fn name(self, v: V) -> Self`
→ Sky `name : B -> V' -> B` (owned threading — the handle goes in, the new
handle comes out).

### Shape F — sync fallible

`fn f(x: X) -> Result<Y, E>` → Sky `f : X' -> Result Error Y'`.
Err arm through `sky_error_from_foreign`. Unchanged from banked design.

### Shape A — async (the new spine)

```
async fn f(&self, x: X) -> Result<Y, E>      (or -> Y, or -> impl Future<Output = …>)
        │
        ▼  Sky-visible
f : Recv -> X' -> Task Error Y'
```

Generated glue (real Rust, uses only existing runtime names):

```rust
/// Rust.Crate.f : Recv -> X -> Task Error Y
pub fn rust_crate_f<E: From<String> + Send + 'static>(
    recv: ::krate::Recv,          // opaque handle, admission-proven Send (C1c)
    x: String,                    // owned coercion of X (Wall-3b: &str → String)
) -> SkyTask<E, i64> {
    Box::pin(async move {
        // Lazy spawn: executes at first poll, i.e. inside the driving tokio
        // context — Handle::current() is guaranteed by the executor contract (§4).
        let joined = tokio::task::spawn(async move {
            recv.f(x.as_ref()).await          // borrowAsRefArgs → .as_ref()
        })
        .await;
        match joined {
            Ok(Ok(y)) => ok_res(y),                                   // + NumCoerce if scalar
            Ok(Err(e)) => SkyResult::Err(sky_error_from_foreign(e)),   // B8 redaction
            Err(join_err) => SkyResult::Err(sky_error_from_foreign(join_err)), // foreign panic
        }
    })
}
```

Design decisions inside Shape A:

- **`spawn(...).await`, not inline.** Three reasons:
  1. **Panic containment for free** — a panic inside the foreign future
     surfaces as `JoinError`, mapped to a structured `Err`. This is strictly
     stronger than `futures::FutureExt::catch_unwind` (no `UnwindSafe`
     bound gymnastics, and it also catches `abort`-free `panic!` in any
     poll) and matches the C5 note already in `core.rs`. The
     `catch_unwind` route remains the documented fallback if a profile
     ever needs `spawn`-free wrappers.
  2. **The Send gates are calibrated for it** — C1b/C1c prove exactly
     "everything captured by `async move` into a spawned task is Send".
     Using the same shape the gates model keeps admission and emission in
     lock-step by construction.
  3. **Isolation of runaway foreign futures** — a spawned task that the Sky
     side stops awaiting (e.g. `Task.parallel` early-cancel calls
     `abort()`) is cancellable at the JoinHandle, giving foreign calls the
     same early-cancel semantics `task_parallel` already guarantees for
     stdlib tasks. NOTE: aborting the *outer* SkyTask (dropping it) leaves
     the inner spawned task detached; to preserve `task_parallel`'s
     no-side-effect-after-failure property the glue aborts on drop:

```rust
        // Drop-guard variant (the actual emitted shape): abort the inner
        // spawned task if the outer SkyTask is dropped/cancelled.
        let handle = tokio::task::spawn(async move { recv.f(x.as_ref()).await });
        let abort = handle.abort_handle();
        let guard = scopeguard(abort);           // tiny emitted helper: aborts unless defused
        let joined = handle.await;
        guard.defuse();
```

  (The helper is ~10 lines emitted once per bindings crate: a struct holding
  `AbortHandle` whose `Drop` calls `.abort()` unless defused. No unsafe.)

- **Infallible async** (`async fn f(..) -> Y`): same glue minus the inner
  `Result` match — `Ok(y) => ok_res(y)`, `Err(join) => Err(...)`. Sky type
  is still `Task Error Y'` (the panic/cancel arm needs the `Error` slot; an
  effectful call is never `Task Never`).

- **`impl Future<Output = …>` returns** (non-`async fn` combinator style):
  identical treatment — `is_future_type` classifies them `effectful`; the
  glue awaits the returned future inside the same spawned block:
  `spawn(async move { recv.f(x).await })`. The only inadmissible sub-case
  is a future type we cannot prove `Send + 'static`, which the C1 gate
  already drops.

### Shape C — async constructor

`async fn new(cfg: Cfg) -> Result<Self, E>` → Sky
`new : Cfg' -> Task Error Handle`. Same glue as Shape A with no receiver;
admitted when `Self` passes `is_provably_send_opaque_return` (Wall-3c #61 —
already implemented). This is the Firestore `FirestoreDb::new` shape.

### Scalar/collection coercions (unchanged)

Param and return coercions reuse the banked D2 table + `NumCoerce`
saturating leaf: `&str`→`String` in, `String` out; `Vec<T>` ↔ `List`;
`Option` ↔ `Maybe`; serde-bounded generic slots → JSON-text `String` via
the `SerdeValue` Call-AST node + `methodTurbofish`. Nothing async-specific.

---

## 3. The builder problem — decomposition, not chain synthesis (Q1b)

Stripe's API is fluent: `CreateCustomer::new().name("Jo").email(e).send(&client).await`.
reqwest's is too: `client.post(url).json(&b).send().await?.json::<T>().await`.

**Fresh-design verdict: never synthesize a chain.** Attempting to auto-emit
one Sky function per *chain* is combinatorially explosive (every subset of
setters × every terminal), hides the intermediate types, and duplicates
what Sky's pipeline operator already does natively. Instead:

1. **Each chain step is one binding** in an existing primitive shape:
   - `CreateCustomer::new()` → Shape P (ctor, returns opaque builder handle)
   - `.name(self, v) -> Self` → Shape P (by-value owned threading)
   - `.send(&self/self, &Client) -> impl Future<Result<R,E>>` → Shape A
   - `Response::json::<T>(self)` (serde terminal) → Shape A + SerdeValue
     (Sky sees `Task Error String` of JSON text; `JsonDec` decodes typed).
2. **Sky's `|>` IS the fluent chain**; `Task.andThen` IS the multi-await:

```elm
createCustomer : Client -> String -> Task Error Customer
createCustomer client email =
    CreateCustomer.new
        |> CreateCustomer.email email
        |> CreateCustomer.send client
        |> Task.andThen (\json -> Task.fromResult (decodeCustomer json))
```

Three properties fall out mechanically:

- **Linear binding count** — a resource with N setters + M terminals costs
  N + M bindings, not 2^N chains. Stripe-scale stays tractable (§8).
- **Intermediate values are first-class** — a Sky program can build a
  request, stash it, batch it in `Task.parallel`, retry it with
  `Task.retryWith` (each attempt re-runs the whole Sky-side chain, so the
  re-runnable-thunk contract of `task_retry_with` is preserved because the
  chain is *Sky* code, not a frozen future).
- **DCE prunes per step** — an app that never sets `.metadata` never emits
  that wrapper.

**The one genuine chain-shaped obligation** is handle *ownership* across
steps: `self`-consuming setters mean the handle moves at every step. That
is exactly the affine-handle representation (§6), and Sky's pipeline usage
pattern (`b |> f |> g` — each intermediate used once) makes the take-once
runtime rule invisible in idiomatic code.

---

## 4. Executor contract (Q2 — whose tokio runtime)

### 4.1 The contract

**One process-global multi-thread tokio runtime owns every foreign future.**

```rust
// runtime/src/sky_runtime/task.rs — proposed hardening H1
static SKY_RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();

fn global_runtime() -> Result<&'static tokio::runtime::Runtime, String> { /* OnceLock init, Err on build failure */ }

pub fn block_on<E, A>(future: SkyTask<E, A>) -> SkyResult<E, A> { /* global_runtime()?.block_on via spawned thread as today */ }
```

Today `block_on` builds a **fresh runtime per call** (task.rs:5-18). That
is correct for stdlib kernels (each request is self-contained) but is a
landmine for FFI **client handles**: a `stripe::Client` or `FirestoreDb`
constructed under runtime A holds I/O resources (hyper pools, gRPC
channels, timers) registered with A's reactor; the Sky idiom
`client = Stripe.connect cfg |> Task.run` at module top-level followed by
per-request `Task.run`s would use the handle on runtime B after A dropped
→ hyper "reactor gone" errors. The process-global runtime makes handle
lifetime ≤ reactor lifetime **by construction**, for every app shape (CLI,
server, TEA loop). This is a ~15-line change to `task.rs`, keeps the
panic-isolation `std::thread::spawn(...).join()` wrapper, and is
behavior-compatible for all existing kernels (a shared reactor is strictly
more available than a fresh one). `block_on_current_thread` (the webview
driver) is untouched — Sky.Webview apps that also use FFI clients get the
global runtime via the spawn path inside Shape-A glue (`spawn` targets
`Handle::current()`, which inside `block_on_current_thread` is the
current-thread runtime; the glue's spawn contract is "the driving
context", not "the global runtime" — both satisfy it).

### 4.2 Can generated glue simply `Box::pin(async move { ... })`?

**Yes — that is the design** (§2 Shape A), with the lazy `spawn` inside.
The pinned future does nothing until polled; at poll time it is by
definition inside a runtime context (block_on / server / Cmd.perform all
drive tasks on tokio), so `tokio::task::spawn` cannot panic with "no
reactor". This is the same reasoning our reqwest kernel already relies on.

### 4.3 Crates that spawn internally / need a runtime handle

- **Internal `tokio::spawn` / `Handle::current()`** (connection pools,
  firestore's gRPC channel keepalives, async-stripe's hyper client):
  satisfied automatically — they grab the ambient handle at poll time,
  which our contract guarantees. No detection needed.
- **Crates that construct their OWN runtime internally** (a sync facade
  doing `Runtime::new().block_on(...)` inside): calling one from within our
  runtime panics ("cannot start a runtime from within a runtime") for the
  `block_on`-inside case. This is **statically undetectable** from rustdoc
  JSON (it is a body property, not a signature property). Ruling:
  fail-open at admission (the signature looks sync/pure), fail-closed at
  runtime — the panic is contained by the Shape P/F wrappers' existing
  `catch_unwind` boundary (banked D6) and surfaces as a structured `Err`
  naming the correlation id. Documented limitation; no unsafe, no abort.
- **`!Send` futures** (`Rc`-captured, `?Send` async-trait methods):
  rejected fail-closed by C1/C1b/C1c today. The principled unlock — a
  dedicated **LocalSet driver thread** owning all `!Send` futures for a
  crate, with an mpsc round-trip (`request → oneshot ← response`) — is
  specified as extension **E1** (§10), NOT v1: it adds a per-crate thread,
  an actor protocol, and ordering semantics that deserve their own proof.
  Over-drop until then.

---

## 5. Send / lifetime walls — the classification table (Q3)

Rule zero (inherited, kept): **over-drop is sound; under-bind wrongly is
not.** Every row below is either MECHANICALLY admitted with a named
mechanism, or fail-closed dropped with a named drop reason.

| # | Foreign shape | Verdict | Mechanism |
|---|---|---|---|
| 1 | `async fn f(x: OwnedSend) -> SendPrim/String/Vec<prim>` | **auto** | Shape A; C1/C1b pass |
| 2 | `async fn f(&self, …)` on Send-proven receiver | **auto** | receiver cloned/Arc'd to owned, moved into `async move`; C1c proof |
| 3 | `async fn f(&self, x: &str)` | **auto** | Wall-3b: Sky `String` owned in, `.as_ref()` at call site (clone-into-owned generalizes to any `AsRef` param) |
| 4 | `async fn new(cfg) -> Result<Self, E>`, `Self` Send-proven | **auto** | Shape C (Wall-3c #61 opaque-return admit) |
| 5 | serde-generic async method (`get_obj<T: DeserializeOwned, S: AsRef<str>>`) | **auto** | SerdeValue reduction + `methodTurbofish` (#72); Sky sees JSON-text `String` |
| 6 | by-value builder setter `fn set(self, v) -> Self` | **auto** | Shape P owned threading; handle repr per §6 |
| 7 | `&mut self` setter returning `&mut Self`/`()` | **auto** | `self_returning` rebuild wrapper (`let mut r = h; r.set(v); r`) — requires owned handle (Clone or affine) |
| 8 | async fn with non-`'static` borrow of a NON-receiver param (`fn f<'a>(&'a self, s: &'a Big) -> Fut<'a>`) | **auto iff** param type is in the owned-coercible set (clone-into-owned kills the lifetime); else **drop** `lifetime-not-elidable` | owned coercion |
| 9 | `!Send` future (Rc capture, `?Send` async-trait) | **drop** (`async-future-not-send`) | E1 LocalSet extension later |
| 10 | `impl Stream` / `BoxStream` returns | **drop** in v1 (`impl-trait-return`) | E3 pull-handle extension (§10) — do NOT drain-to-list (unbounded memory) |
| 11 | async fn returning borrowed data (`-> &str`, `-> Cow<'_, str>`) | **drop** (`borrowed-return`) | E2 clone-to-owned extension where return `: ToOwned` |
| 12 | `&mut self` async method on shared handle | **drop** (`aliased-mut`) | no sound sharing story; Mutex-wrap would silently serialize — rejected as under-bind-wrongly |
| 13 | callback/closure params on async fns | **drop** in v1 | closure bridging (Phase 6.2 in inspector) must compose with Send proofs first |
| 14 | async fn on a receiver whose Send-ness is conditional (`Base<T>: Send where T: Send`) | **auto iff** every instantiation arg proven Send | `SEND_WHEN_ARGS_SEND_NAMES` (#87) structural proof |
| 15 | future types not provably `'static` (borrowing futures) | **drop** | subsumed by 8/11; the Call AST simply has no owned rendering |

Row 3/8's principle generalizes: **lifetimes never cross the boundary
because ownership always does.** The generator's only lifetime strategy is
"make it owned before the future is built"; anything it cannot own, it
drops.

---

## 6. Client / state objects (Q4)

### 6.1 Opaque nominal handles

A foreign type reachable in an admitted signature becomes a Sky nominal
opaque `Ty::Con { module: "Rust.Stripe", name: "Client" }` — unifies
nominally, no structure exposed, exactly like `CsvDoc`/`HttpRequest` today
(`runtimeOpaqueTypes` precedent in `http_client.rs`).

### 6.2 Representation-selection algorithm (new, load-bearing)

Sky values are shared (a Sky `let c = client` aliases freely), Rust values
are owned. The generator picks the handle representation per foreign type
`T`, in this order:

```
1. T: Clone                          → repr = T          (clone at each use)
2. T: !Clone, every bound method
   takes &self, T: Send + Sync      → repr = Arc<T>      (Arc-clone at each use;
                                                          &self via auto-deref)
3. T: !Clone, some bound method
   consumes self (builder shape)    → repr = Affine<T>   (take-once cell, §6.3)
4. otherwise                        → type not bindable → every fn touching it drops
```

Rule 1 covers `stripe::Client`, `FirestoreDb` (SDK clients are Arc-backed
`Clone` by convention). Rule 2 covers `!Clone` clients with pure-`&self`
APIs — `Arc<T>: Clone` regardless of `T`, and `Arc<T>: Send ⟺ T: Send+Sync`,
which the existing Send-proof machinery can check (both facts appear in
rustdoc's auto-impl synthesis, the same source #87 reads). Rule 3 covers
`reqwest::RequestBuilder`-style consumed builders.

### 6.3 Affine handles (the one deliberate compile-time→runtime trade)

```rust
/// Emitted once per bindings crate. Total: no unwrap/panic/unsafe.
pub struct Affine<T>(std::sync::Arc<std::sync::Mutex<Option<T>>>);
impl<T> Clone for Affine<T> { fn clone(&self) -> Self { Affine(self.0.clone()) } }
impl<T> Affine<T> {
    pub fn new(v: T) -> Self { Affine(std::sync::Arc::new(std::sync::Mutex::new(Some(v)))) }
    /// Take the value out; second take → structured error (never panic).
    pub fn take(&self, what: &str) -> Result<T, String> {
        match self.0.lock() {
            Ok(mut g) => g.take().ok_or_else(|| format!(
                "{what}: this value was already consumed — foreign builder values are single-use; rebuild the chain from its constructor", )),
            Err(_) => Err(format!("{what}: internal handle lock poisoned")),
        }
    }
}
```

Glue for a consumed-self step: `let b = handle.take("Rust.Reqwest.send")?;
… b.send().await …` (the `?`-shaped early return renders as the usual
`SkyResult::Err` arm). Reuse of a consumed builder is a **deterministic,
typed, actionable `Err`** — a semantic downgrade from Rust's compile-time
affine check, but total (no UB, no panic) and invisible in idiomatic
pipeline code where each intermediate is used exactly once.

**Why not reject `!Clone` builders outright?** reqwest's `RequestBuilder`
and many SDK request types are `!Clone`; rejecting them forfeits the
acceptance goal for large parts of real SDK surfaces. The trade is
explicit, documented, and scoped: `Affine` is selected ONLY for types that
appear in consumed-`self` positions (builder shape), never for clients.
(The conciliation arm should stress-test this ruling — it is the boldest
call in this doc.)

### 6.4 Construction + config structs

- **Sync ctor** (`Client::new(key)`) → Shape P/F returning the handle.
- **Async ctor** (`FirestoreDb::new(project).await`) → Shape C,
  `Task Error FirestoreDb`.
- **Config structs** (`FirestoreDbOptions`, `CreateCustomer` params): if
  all-fields-coercible, the inspector already emits synthetic field
  getters/setters + `Default`-based ctor (existing taxonomy); otherwise
  they are themselves opaque handles built through their own builder
  methods. Either way, no new mechanism.
- **Threading through Sky**: the handle is an ordinary Sky value — stored
  in a Model field, passed as a function arg, captured in a closure. The
  repr (`T`/`Arc<T>`/`Affine<T>`) is `Clone + Send + 'static` in all three
  arms, which is exactly what Model storage and `Cmd.perform` capture
  require. (Session-store persistence of a Model holding a handle is the
  same closure-Model problem `core.rs` already solves with
  `disconnected_*` — handles serde-skip and reconstruct as a
  disconnected-store-style structured error; memory-store-only, enforced
  by the same codegen gate.)

---

## 7. Error mapping (Q5)

**One mechanical, universal rule — already implemented in our runtime:**

> Every foreign `Err(e)` and every `JoinError` routes through
> `sky_error_from_foreign(e)` (`core.rs:54`): the raw `Debug` detail is
> logged **server-side only** under a fresh 4-byte correlation id
> (B8 — SDK errors echo URLs, bearer tokens, API keys in their `Debug`),
> and Sky receives the fixed generic
> `Error.unexpected "external operation failed (ref <id>)"`.

Properties: typed `Error` (never `Result String` / `Task String` — repo
non-regression rule), total (any `E: Debug` accepted — `Debug` is
universal on error types), secret-safe by construction, and the two-level
correlation-id pattern matches the stdlib's own error discipline.

**Deliberately NOT in v1:** per-SDK error taxonomies (mapping
`stripe::Error::RateLimit` → a retryable Error kind). It is mechanically
*feasible* — public error enums already get `is_enum_tag` accessors, so a
generator pass could surface `Stripe.errorKind : Error -> String` — but it
requires exposing the foreign error VALUE to Sky, which conflicts with B8
redaction unless the tag is proven secret-free. Specified as extension
**E4**: bind the error enum's *tag only* (a closed set of variant names —
no payload, no `Debug`, hence no secret channel) alongside the redacted
message, enabling `Task.retryWith (retryOn isRateLimit)`. Sound because
tags are identifiers from the crate's public API, not runtime data.

---

## 8. DCE + Stripe scale (Q6)

### 8.1 Who computes the used set

**The compiler's existing whole-program DCE** — FFI refs are
`FfiRef(kernel, fn)` reachability facts exactly like stdlib kernel refs
(banked design, kept). No new analysis.

### 8.2 Fresh ruling: demand-driven emission, not emit-all-then-prune

The banked plan adopted sentinel-sliced emit-all (`_bindings.rs` written at
add-time, text-sliced at build). At Stripe scale that is the wrong side of
the cost curve — 76k wrappers × ~600 B ≈ **45 MB of dead Rust text**
written at add-time, re-sliced every cold build. This design replaces it:

```
ipe add stripe            (once, sandboxed)
  └── .ipecache/ffi/rust/stripe.kernel.json     ← the FULL catalogue: every
      admitted Function + Call AST (the generator's INPUT, not its output)
  └── .ipecache/ffi/rust/stripe.ipei            ← HM signatures + a
      name→byte-offset index for lazy seeding

ipe build                 (every build, warm)
  ├── HM env: lazy-seed only the qualified names the program mentions
  │   (offset index → O(used) parse, never O(76k))
  ├── DCE: used FfiRef set  U
  └── bindings synthesis: pure fn (kernel.json, U) → stripe_bindings.rs
      containing EXACTLY |U| wrappers; cached by hash(crate-ver, U, gen-ver)
```

- The generator is the same pure JSON→text function either way — this
  changes *when it runs and over what subset*, not what it is. Small
  crates (≤ ~500 symbols) may still eager-emit for byte-diff testing; the
  build path is identical.
- **No untrusted code at build time** — synthesis reads cached JSON
  produced inside the add-time sandbox; the security phase-separation is
  untouched.
- Sentinels remain in the emitted file as a debugging aid, but nothing
  slices them.

### 8.3 Cost model for skyshop-rs

| Cost | When | Estimate |
|---|---|---|
| Inspect async-stripe + firestore (sandboxed rustdoc) | `ipe add`, once per crate-version | minutes (dominated by cargo check of the SDK); cached forever |
| kernel.json size @ 76k symbols | disk, once | ~40-80 MB JSON (acceptable in `.ipecache`; gitignored) |
| HM seeding per build | warm build | O(used) ≈ 30-60 entries via offset index; < 10 ms |
| Wrapper synthesis per build | warm build, cached | |U| ≈ 30-60 wrappers ≈ 2k lines; < 50 ms; cache-hit ≈ 0 |
| cargo build of the SDK dep | first build per lockfile | the real cost (~minutes for async-stripe); amortized by the shared target dir + sccache (repo policy) |
| Incremental rebuild after Sky-only edit | every dev loop | wrapper crate unchanged (U-hash stable) → cargo no-op |

The 76k number never appears on the per-build path.

---

## 9. Acceptance sketch — skyshop-rs auto-bound (Q7)

Inferred from our own docs/inspector comments (not `../sky`): skyshop-rs
needs (a) Firestore document CRUD — the `get_obj`/`create_obj` async serde
trait methods named in `main.rs:200-204`; (b) Stripe checkout/customer
creation — the rc.6 builder-`send` shape named in `main.rs:1475-1481` and
the `Customizable<Resp>` Send proof (#87).

### 9.1 Sky surface (what the app author writes)

```elm
import Rust.Firestore as Fs
import Rust.StripeCheckout as Sc

init : Task Error Ctx
init =
    Fs.firestoreDbNew "skyshop-prod"                      -- Task Error FirestoreDb
        |> Task.map (\db -> { db = db, stripe = Sc.clientNew stripeKey })

fetchProduct : Ctx -> String -> Task Error Product
fetchProduct ctx id =
    Fs.getObj ctx.db "products" id                        -- Task Error String (JSON text)
        |> Task.andThenResult (JsonDec.decodeString productDecoder)

checkout : Ctx -> Cart -> Task Error String
checkout ctx cart =
    Sc.createCheckoutSessionNew
        |> Sc.withMode "payment"
        |> Sc.withSuccessUrl cart.successUrl
        |> Sc.send ctx.stripe                             -- Task Error String (JSON text)
        |> Task.andThenResult (JsonDec.decodeString sessionUrlDecoder)
```

Every step above is one of the four primitive shapes; zero shims.

### 9.2 Generated glue, end-to-end for ONE call (`Fs.getObj`)

Foreign signature (firestore crate, `#[async_trait]`-desugared by WALL-4):

```rust
async fn get_obj<T: DeserializeOwned + Send, S: AsRef<str> + Send>(
    &self, collection_id: &str, document_id: S,
) -> Result<T, FirestoreError>
```

Inspector facts driving emission: `is_async=true` → `effect="effectful"`;
receiver `FirestoreDb` ∈ `PROVABLY_SEND_RECV_NAMES` (C1c ok); `T` serde-
reduced → `SerdeValue`; `S` AsRef-mono'd → `String` (WALL-2);
`methodTurbofish = [SerdeValue, Prim String]` (#72); `collection_id` ∈
`borrowAsRefArgs`. Emitted wrapper:

```rust
// ── AUTO-GENERATED — Rust.Firestore.getObj ──
// getObj : FirestoreDb -> String -> String -> Task Error String
pub fn rust_firestore_get_obj<E: From<String> + Send + 'static>(
    db: ::firestore::FirestoreDb,      // repr rule 1: Clone client, moved in
    collection: String,
    id: String,
) -> SkyTask<E, String> {
    Box::pin(async move {
        let handle = tokio::task::spawn(async move {
            db.get_obj::<serde_json::Value, String>(collection.as_ref(), id).await
        });
        let guard = AbortOnDrop::new(handle.abort_handle());   // §2 cancel-propagation
        let joined = handle.await;
        guard.defuse();
        match joined {
            Ok(Ok(v)) => match serde_json::to_string(&v) {
                Ok(s) => ok_res(s),                                   // JSON text to Sky
                Err(e) => SkyResult::Err(sky_error_from_foreign(e)),
            },
            Ok(Err(e)) => SkyResult::Err(sky_error_from_foreign(e)),   // FirestoreError, redacted
            Err(join_err) => SkyResult::Err(sky_error_from_foreign(join_err)),
        }
    })
}
```

Compiler side: `Fs.getObj` canonicalises to `KernelId::Ffi(fid)`; lowering
emits `Call { callee: Kernel(Ffi fid), args }`; backend renders
`rust_firestore_get_obj::<SkyCoreErrorError>(m_db.clone(), s_coll, s_id)`
and adds `firestore = "=<locked>"` (+ effective features) to the emitted
`Cargo.toml`. No new IR nodes, no new match arms.

### 9.3 The Stripe builder chain, glue shapes only

- `Sc.createCheckoutSessionNew` → Shape P ctor → handle repr per §6.2
  (Clone params struct → rule 1; `!Clone` → `Affine`).
- `Sc.withMode` / `Sc.withSuccessUrl` → Shape P owned threading
  (`fn(b, v) -> b`).
- `Sc.send client` → Shape A on the builder handle;
  Ok payload `Customizable<CheckoutSession>` passes the #87 structural
  Send proof; serde-reduced → JSON text.

---

## 10. Extensions (specified, NOT v1)

| Id | What | Sketch | Unblocks |
|---|---|---|---|
| E1 | `!Send`-future actor thread | per-crate dedicated thread running a `LocalSet`; glue sends `(args, oneshot)` over an mpsc, awaits the oneshot (the *channel* future is Send even though the foreign future is not) | `?Send` async-trait SDKs, Rc-internal crates |
| E2 | borrowed-return clone-out | `-> &T where T: ToOwned` → call, `.to_owned()`, return owned | accessor-heavy crates |
| E3 | `impl Stream` pull handles | `BoxStream<Item=Result<T,E>>` → opaque `StreamHandle`, `next : StreamHandle -> Task Error (Maybe a)` over `Arc<tokio::sync::Mutex<…>>`; mirrors our `http_stream` forEachChunk shape | LLM token streams, Firestore listeners |
| E4 | error-tag surfacing | bind public error enums' variant TAG only (secret-free identifier set) alongside the B8-redacted message → `retryOn` predicates | typed retry policies on SDK errors |

---

## 11. De-risk plan — the ONE vertical slice

**Prove Shape A + Shape C + repr rule 1 end-to-end on ONE real crate
before building anything else.** Chosen slice: **`firestore::FirestoreDb::new`
(Shape C) + `get_obj` (Shape A + SerdeValue + turbofish + borrowAsRef)** —
it exercises every novel mechanism of this doc in two functions:
async ctor with Send-opaque return, moved Send receiver, serde reduction,
method turbofish, owned-coercion of borrows, JoinError panic path, B8
error path, cancel-propagation guard, and the executor contract (gRPC
channel spawned internally). (If network-free CI is required for the
slice, the same shapes are exercisable against a hand-pinned
`reqwest::get` + `Response::text` pair with a local hyper test server —
but firestore is the acceptance-shaped target.)

Steps (each independently verifiable, no builds implied by this doc):

1. **H1** — process-global runtime in `task.rs` (§4.1) + regression test:
   handle created in one `Task.run`, used in a later one, still works.
2. **Hand-write the two wrappers** exactly as §9.2 emits them, in a
   scratch bindings crate; drive from a 10-line Rust main via `block_on`.
   Gate: live round-trip + panic-injection → structured `Err` + abort test.
3. **Teach the generator** (ipe_ffi M-G) to emit those two wrappers from
   the crate's real kernel.json; byte-diff against the hand-written pair.
4. **Wire one Sky program** through canon/lower/backend to call them.
   Gate: `fetchProduct`-shaped Sky code builds + runs.
5. Only then: scale out (stripe builder chain → affine handles → E-track).

Kill criteria (honest failure modes to watch): (a) firestore's channel
outliving expectations under the drop-guard abort — if aborting mid-gRPC
corrupts the shared channel state, cancel-propagation degrades to
detach-with-warning for that crate class; (b) `Affine` proving confusing
in practice — fallback is rejecting `!Clone` builders (over-drop) and
accepting a smaller bound surface.

---

## 12. Soundness checklist (self-audit against repo principles)

- No `unsafe`, no `unwrap`/`expect`/panic in any emitted glue; every
  failure arm is a structured `SkyResult::Err`.
- Foreign panics: contained (JoinError → Err; sync via catch_unwind;
  `compile_error!` fence against `panic=abort` profiles retained from the
  banked design).
- Secrets: every foreign error routes through B8 redaction; error tags
  (E4) are the only foreign error data ever surfaced, and only as
  identifier constants.
- Fail-closed: every non-admitted shape has a named drop reason (§5);
  no `Ty::Any`, no stringly fallback, no dynamic dispatch (the two
  panicking polyfills remain the refusal wall).
- Cancellation: `Task.parallel` early-cancel semantics extend to foreign
  calls via the abort-on-drop guard.
- The ONE deliberate semantic trade (affine take-once, §6.3) is explicit,
  total, scoped to builder types, and flagged for conciliation review.
