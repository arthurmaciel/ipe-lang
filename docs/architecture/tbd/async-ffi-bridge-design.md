# Async-FFI Auto-Bridge — AUTHORITATIVE design (conciliation of the double-swarm)

> **Status:** the ONE design of record for the async-FFI auto-bridge, produced
> 2026-07-04 by conciliating the two swarm arms (a reference-blind
> first-principles arm and a reference-mining arm). Supersedes both for
> implementation purposes; the two swarm-arm inputs are preserved in git
> history. This design owns async emission as part of the base generator; the
> milestone DAG it amends lives in `ffi-port-spec.md` (§C) and the architecture
> in `ffi-subsystem-design.md` (D1–D8).
>
> **Conciliation discipline applied:** faithful-port DEFAULT — where the
> reference (upstream Sky @ feat/runtime-rust) has a *proven* async mechanism we
> port it; fresh-arm inventions are adopted ONLY where they beat the reference
> on a named principle or fill a verified reference gap. Every divergence is
> recorded (§8) per the sanctioned-divergence policy and belongs in
> `docs/divergences-from-sky.md` when implemented.
>
> **Corrected premise (verified in-repo 2026-07-04):** the reference DOES bind
> async natively since #44 (2026-06-23) — firestore 0.49 binds direct and
> shim-free (fixture `104-ffi-owned-query-builder`), every stripe mechanism is
> proven on synthetic fixtures 93/94/95/96 (WALL-I/J/K). The skyshop-rs shims
> are a pre-#44 fossil. The reference left exactly TWO ends open: (1) the real
> `async-stripe` end-to-end build was never executed
> (`wall-i…md`: "pure real-crate verification, no remaining mechanism gap");
> (2) skyshop-rs was never migrated off its three shim crates. **We port the
> campaign and finish those two ends. Exceeding the reference = skyshop-rs with
> ZERO shims (firestore + firebase + stripe) + used-set-only DCE** — the
> acceptance metric, unchanged.

---

## 0. Verdict in five lines

1. **Mechanism = port of the reference's #44 wrapper** (`Box::pin(async move {
   spawn(...).await })` three-arm match, `Ffi.hs:997-1006`), upgraded with two
   small fresh-arm improvements: JoinError routes through the B8 error funnel,
   and an abort-on-drop guard preserves `Task.parallel` cancel semantics (§1).
2. **Admission = the reference's tri-gate Send discipline** (C1 output / C1b
   params / C1c receiver, `Clone ≠ Send`) + `#[async_trait]`/RPITIT de-async —
   all ALREADY VENDORED in our inspector superset; nothing to invent (§3).
3. **Runtime = one process-global tokio runtime** (fresh-arm H1, a verified
   reference gap); never per-call `block_on` (both arms agree — the reference
   rejected it explicitly in the #44 design) (§4).
4. **Handles = reference parity: Clone-gated opaques, fail-closed drop for
   `!Clone`**; the fresh Affine handle is banked as an extension, NOT v1 —
   the acceptance targets don't need it (§5).
5. **DCE = port the reference's sentinel-sliced emit-all + FULL-EMIT
   fail-safes** (proven at stripe scale: 3,534 bound wrappers, not 76k);
   the fresh demand-synthesis is banked as a measured-trigger escalation (§7).

---

## 1. Mechanism — the generated glue (ported, with two upgrades)

### 1.1 The wrapper body (Shape A, fallible async)

Ported from `upstream:src/Sky/Build/Rust/Ffi.hs:997-1006` (shipped #44), with
the two adopted fresh-arm deltas marked:

```rust
// ── AUTO-GENERATED — Rust.<Crate>.<fn> : Recv -> X' -> Task Error Y' ──
pub fn rust_crate_f<E: From<String> + Send + 'static>(
    recv: ::krate::Recv,   // owned handle — ownRefIdx strips `&` (E0521 escape), re-borrow at call site
    x: String,             // owned coercion (Wall-3b: &str param → String, `.as_ref()` at call)
) -> SkyTask<E, i64> {
    Box::pin(async move {
        let handle = tokio::task::spawn(async move { recv.f(x.as_ref()).await });
        let guard = AbortOnDrop::new(handle.abort_handle());   // Δ1 (fresh): cancel propagation
        let joined = handle.await;
        guard.defuse();
        match joined {
            Ok(Ok(v)) => ok_res(/* retCoerce */ v),
            Ok(Err(e)) => SkyResult::Err(sky_error_from_foreign(e)),        // B8 redaction (port)
            Err(join_err) => SkyResult::Err(sky_error_from_foreign(join_err)), // Δ2 (fresh): funnel, not bare str_err
        }
    })
}
```

- **Ported verbatim:** lazy `Box::pin(async move { … })` (no work before first
  poll); `tokio::task::spawn(...).await` for panic containment (JoinError arm =
  the reference's C5 discipline — no `catch_unwind` bound gymnastics);
  owned-everything at the boundary (`ownRefIdx`, `Ffi.hs:757-800`: async
  `&Opaque` params would escape the `'static` spawn → declared owned,
  re-borrowed at the call site); serde prelude spliced INSIDE the async block
  for generic instances (`FfiInstance.hs:820-825`); infallible-async two-arm
  variant (`Ok(v)/Err(join)`) selected by the `Result<` return-prefix test;
  Ipê type is always `Task Error a` (the panic/cancel arm needs the Error
  slot — never `Task Never`).
- **Δ1 — AbortOnDrop guard (adopt fresh; fills a reference gap).** The
  reference's inner spawned task detaches if the outer SkyTask is dropped
  (e.g. `task_parallel` early-cancel aborts the outer JoinHandle), leaking
  side effects after failure. The guard is ~10 emitted lines (a struct holding
  `AbortHandle` whose `Drop` aborts unless defused), no unsafe. Kill
  criterion: if aborting mid-gRPC corrupts a shared channel for some crate
  class, degrade to detach-with-warning for that class (recorded).
- **Δ2 — JoinError through `sky_error_from_foreign` (adopt fresh; strictly
  better on operator traceability).** The reference emits bare
  `str_err("foreign async call panicked")` — no correlation id. Routing the
  `JoinError` through the funnel logs its `Debug` server-side under an err-id
  and returns the same generic message shape. Recorded divergence.

### 1.2 Multi-await chains — decomposition, not synthesis (both arms agree)

Never synthesize a chain wrapper. Each chain step is one binding in a
primitive shape; Ipê's `|>` IS the fluent chain and `Task.andThen` IS the
multi-await. This is simultaneously the fresh arm's first-principles ruling
and the reference's shipped practice (per-method bindings + owned threading;
`self_returning` setters). Properties: linear binding count (N setters + M
terminals, never 2^N), first-class intermediates (`Task.retryWith` re-runs the
Ipê-side chain — the re-runnable-thunk contract holds because the chain is Ipê
code, not a frozen future), and per-step DCE.

### 1.3 Error mapping (port — already implemented in our runtime)

One universal rule: every foreign `Err(e)` AND every `JoinError` routes
through `sky_error_from_foreign` (`src/runtime/rust/src/core.rs:54` —
verified present): raw `Debug` logged server-side under a fresh correlation
id (B8 — SDK errors echo URLs/bearer tokens/API keys in their `Debug`), Ipê
receives `Error.unexpected "external operation failed (ref <id>)"`. Typed
`Error` always (repo non-regression rule: no `Result String` / `Task String`).
The shims' verbatim-`Display` embedding is the LESS safe fossil pattern; do
not port it.

**Extension E4 (specified, not v1):** bind public error enums' variant TAG
only (identifier constants — no payload, no `Debug`, no secret channel) so
`Task.retryWith (retryOn isRateLimit)` becomes possible on SDK errors.

---

## 2. Shape taxonomy (adjudication d — reference semantics, fresh names)

The reference's mechanism is the `effect` field (pure/fallible/effectful) ×
`Result<`-prefix fallibility × `self_returning` × receiver metadata. The
fresh arm's four-shape framing maps 1:1 onto it and is adopted as the
DOCUMENTATION taxonomy (it names what the matrix emits); the emitter itself
ports the reference's matrix — no fifth shape, no chain shape.

| Shape | Foreign form | Ipê form | Reference mechanism |
|---|---|---|---|
| **P** pure/sync | `fn f(x) -> Y`; by-value setter `fn s(self, v) -> Self`; `&mut self` setter | `f : X' -> Y'`; owned threading | `effect=pure`, `self_returning` rebuild wrapper |
| **F** sync fallible | `fn f(x) -> Result<Y,E>` | `f : X' -> Result Error Y'` | `effect=fallible` (upgrade: Err arm through `sky_error_from_foreign`, not `format!("{:?}")` — the reference's sync arm still embeds Debug text; recorded strictly-better divergence, same B8 rationale) |
| **A** async | `async fn f(&self, x) -> Result<Y,E>` / `-> Y` / `-> impl Future` | `f : Recv -> X' -> Task Error Y'` | `effect=effectful` + §1.1 body; `is_future_type` unifies the three return sugars (#44 recognition) |
| **C** async constructor | `async fn new(cfg) -> Result<Self,E>` | `new : Cfg' -> Task Error Handle` | Shape A with no receiver + Wall-3c opaque-return admit (#61); shipped as WALL-5 (the `FirestoreDb::new` shape) |

Scalar/collection coercions unchanged from the banked D2 table (`NumCoerce`
saturating leaf; `Vec`↔`List`; `Option`↔`Maybe` — never a `_status` key;
serde-bounded generic slots → JSON-text `String` via `SerdeValue` +
`methodTurbofish` #72). Nothing async-specific.

---

## 3. Send / lifetime gates (adjudication e — reference walls folded in)

Rule zero (both arms): **over-drop is sound; under-bind wrongly is not.**
Everything below is already implemented in our VENDORED inspector (18,500-line
superset of the reference checkout, carrying WALL-K); the port obligation is
the *generator side consuming these verdicts*, not the analysis.

### 3.1 The keystone the fresh arm under-weighted: de-async + tri-gate

- **`#[async_trait]` / RPITIT de-async (WALL-4 #64, #81)** — `de_async_clone`
  unwraps `Pin<Box<dyn Future<Output=T>+Send>>` → `T` + `is_async=true`. This
  is the KEYSTONE: firestore's and firebase's entire trait-method surface is
  async-trait sugar; without de-async there is nothing to bind. A `?Send`
  shape returns `None` → drop `async-future-not-send`.
- **Tri-gate Send, `Clone ≠ Send`** (the shipped #44 divergence-from-its-own-
  design): C1 output gate (Ok-type provably Send — TIGHTER than Ipê-coercible,
  which admits `Clone + !Send`); C1b every `async move`-captured param; C1c
  the by-move-captured receiver (`recv_provably_async_send`: explicit
  `impl Send` ∪ all-fields-Send ∪ Send-supertrait; #87 conditional structural
  Send for generics like `Customizable<Resp>`).
- **Feature visibility (#89 + #100)** — `--all-features` is rejected for
  external deps, so feature-gated APIs (ALL stripe `Create*` builders +
  `send`) are invisible without `cargo metadata` feature enumeration +
  dep-table injection (stripe surface: 92 → 20,358 symbols). Features
  propagate inspector → kernel.json → emitted `Cargo.toml`. Never judge an
  SDK "unbindable" before checking feature visibility.
- **Cross-crate trait machinery (WALL-G/H/I/J/K)** — provided-method
  projection, `<Self as Trait>::Output` sibling-impl resolution, external-
  trait 3-crate triangles — the exact stripe `send<C: StripeClient>` shape.
  Vendored; the generator consumes the resolved verdicts from kernel.json.

### 3.2 Classification table (merged)

| # | Foreign shape | Verdict | Mechanism |
|---|---|---|---|
| 1 | `async fn f(x: OwnedSend) -> SendPrim/String/Vec<prim>` | **auto** | Shape A; C1/C1b |
| 2 | `async fn f(&self, …)`, Send-proven receiver | **auto** | ownRefIdx owned receiver moved into `async move`; C1c |
| 3 | `async fn f(&self, x: &str)` | **auto** | Wall-3b owned-in, `.as_ref()` at call site |
| 4 | `async fn new(cfg) -> Result<Self,E>`, Self Send-proven | **auto** | Shape C (Wall-3c #61) |
| 5 | serde-generic async method (`get_obj<T: DeserializeOwned, S: AsRef<str>>`) | **auto** | SerdeValue reduction + methodTurbofish (#72); JSON-text `String` |
| 6 | by-value builder setter `fn set(self, v) -> Self` on Clone-opaque | **auto** | Shape P owned threading |
| 7 | `&mut self` setter | **auto** | `self_returning` rebuild wrapper |
| 8 | non-`'static` borrow of a non-receiver param | **auto iff** owned-coercible; else drop `lifetime-not-elidable` | clone-into-owned kills the lifetime |
| 9 | borrowing fluent builder (`fluent(&self) -> Builder<'_>`) | **drop + route to the owned API** | WALL-6 guardian ruling: auto-binding the borrow is UNSOUND (foreign covariance unverifiable) AND costs zero capability — the fluent layer is sugar over an owned params-struct API that binds (fixture 104). The fresh arm missed this routing insight; folded in. |
| 10 | `!Send` future (Rc capture, `?Send` async-trait) | **drop** `async-future-not-send` | E1 LocalSet extension later |
| 11 | `impl Stream` / `BoxStream` returns | **drop** v1 | E3 pull-handle extension — never drain-to-list |
| 12 | async fn returning borrowed data | **drop** `borrowed-return` | E2 clone-out where `ToOwned` |
| 13 | `&mut self` async method on shared handle | **drop** `aliased-mut` | Mutex-wrap would silently serialize = under-bind-wrongly |
| 14 | callbacks/closures on async fns | **drop** v1 | closure bridging must compose with Send proofs first |
| 15 | conditionally-Send generic receiver | **auto iff** every instantiation arg proven Send | #87 structural proof |
| 16 | `!Clone` opaque (builder or client) | **drop** v1 (reference parity) | E5/E6 Arc/Affine extensions (§5) |

Principle (fresh arm, kept as the summary rule): **lifetimes never cross the
boundary because ownership always does.**

---

## 4. Runtime contract (adjudication c)

Both arms agree on the invariant — **NEVER per-call `block_on`** (the
reference's #44 design rejected the shim pattern explicitly: "tokio block_on
inside an already-running runtime PANICS"; native `.await` inside a Task has
no such hazard). The concrete mechanism is decided as:

1. **Port:** wrappers are lazy pinned futures; at poll time they are inside a
   runtime context by construction (block_on entry / server / Cmd.perform),
   so the inner `tokio::task::spawn` targets `Handle::current()` — internally-
   spawning SDKs (hyper pools, gRPC keepalives) are satisfied automatically.
2. **Adopt fresh H1 — process-global runtime (verified reference gap).** Both
   runtimes today build a FRESH `Runtime::new()` per `block_on`
   (`task.rs:5-18`, verified identical in ours). A client handle constructed
   under runtime A (module-level `client = Stripe.connect cfg |> Task.run`)
   holds reactor-registered I/O; using it under runtime B after A dropped →
   hyper "reactor gone". The reference never hit this because its fixtures
   live inside one `Task.run`; the skyshop CLI/dev shapes will. Fix: a
   `OnceLock<tokio::runtime::Runtime>` in `src/runtime/rust/src/task.rs`;
   `block_on` drives on the global runtime via the existing spawned-thread
   panic-isolation wrapper (~15 lines; behavior-compatible — a shared reactor
   is strictly more available than a fresh one). `block_on_current_thread`
   (webview main-thread rule) untouched; the spawn contract is "the driving
   context", which both satisfy.
3. **Crates that construct their own runtime internally** (sync facades doing
   `block_on` inside): statically undetectable (body property). Fail-open at
   admission, fail-closed at runtime — the panic is contained by the wrapper
   boundary and surfaces as a structured `Err` with correlation id.
   Documented limitation (fresh ruling, adopted — the reference is silent).

---

## 5. Handle rules (adjudication a — the affine handle is NOT v1)

- **Opaque nominal handles** (both arms): a foreign type reachable in an
  admitted signature is a Ipê nominal opaque (`Rust.Stripe.Client`), unifying
  nominally, structure unexposed — the `runtimeOpaqueTypes` precedent.
- **v1 representation = reference parity: Clone-gated.** Verified: the
  reference's opaque admission requires Clone (`is_clone_opaque_name`;
  "`&T` for non-Clone T STAY DROPPED"). Call sites `.clone()` the handle per
  use; Ipê-side aliasing is sound because every use owns its copy. The
  acceptance targets pass this gate — stripe rc.6 `Create*` params structs
  and `FirestoreDb` are Clone (fixtures 93/95/96/104 prove it); SDK clients
  are Arc-backed-Clone by convention.
- **Fresh rules 2-3 banked as extensions, not v1:**
  - **E6 `Arc<T>`** for `!Clone` + all-`&self` + `Send+Sync`-proven types —
    sound, cheap, adopt when a target crate needs it (provable from rustdoc
    auto-impls, same source #87 reads).
  - **E5 `Affine<T>`** (take-once `Arc<Mutex<Option<T>>>`, structured `Err`
    on reuse) for `!Clone` consumed-`self` builders (reqwest
    `RequestBuilder` class). REJECTED for v1 under faithful-port + the
    fresh arm's own flag ("boldest call"): it trades a compile-time affine
    guarantee for a runtime error, and NO acceptance-path crate needs it.
    Over-drop is sound; the spec is banked in `async-ffi-bridge-fresh.md`
    §6.3 and re-opens on demonstrated need with a guardian gate.
- **Model/session storage:** handles are ordinary Ipê values (Clone + Send +
  'static); session-store persistence follows the existing
  `disconnected_*` reconstruction pattern (serde-skip + structured error;
  memory-store-only gate).

---

## 6. What the fresh arm invented vs what the port supplies (summary)

| Fresh invention | Disposition |
|---|---|
| `Box::pin` glue + spawn + owned boundary | Convergent rediscovery of shipped #44 — **port** (the reference form is proven; byte-diffable) |
| AbortOnDrop cancel guard | **Adopt** (Δ1 — fills gap; soundness of `task_parallel` contract) |
| JoinError → B8 funnel | **Adopt** (Δ2 — strictly better traceability) |
| Global tokio runtime (H1) | **Adopt** (fills verified gap; 15 lines) |
| No-chain-synthesis / per-await bindings | **Adopt framing** — identical to reference practice |
| 4-shape taxonomy | **Adopt as docs layer** over the reference's effect matrix (§2) |
| Affine handle | **Bank as E5** — not v1 (§5) |
| Demand-synthesized DCE | **Bank as E7 escalation** — not v1 (§7) |
| Runtime-inside-runtime fail-closed ruling | **Adopt** (reference silent) |
| Missed: de-async keystone weight, #89 feature visibility, WALL-6 owned-API routing, WALL-G/H/I/J/K consumption, never-run stripe E2E | **Folded in from the learnings arm** (§3.1, table row 9, §9) |

---

## 7. DCE pipeline (adjudication b — sentinel port; demand-synthesis banked)

**v1 ports the reference's shipped pipeline unchanged in substance:**

- **Program-level:** FFI refs are `Dce.FfiRef kernelName wrapperRefName`
  reachability facts, exactly like stdlib kernel refs; `IPE_DCE=0` / empty
  set → keep-everything.
- **Wrapper-level (S4):** `ipe add` writes ALL admitted wrappers into the
  cached `<slug>_bindings.rs`, each bracketed by
  `// IPE-FFI-WRAPPER BEGIN <ref>` / `END`; build time slices unreached
  regions. Port the load-bearing discipline verbatim
  (`Ffi.hs:222-258`, `Project.hs:499-540`):
  - **R-D SSOT keying** — ONE `wrapperRefName` function feeds kernel.json
    names, `.ipei` entries, sentinels, and the reached-set keys, making
    key/item divergence impossible by structure;
  - **R-3 FULL-EMIT fail-safes** — copy verbatim on DCE-off, empty reached
    set, missing/unparseable kernel.json, or sentinel↔kernel.json bijection
    failure (never drop on doubt);
  - **R-4 staleness rule** — the filter runs EVERY build from
    (sentinels ∩ reached), never from a source-hash cache.
- **Generic instances stay demand-driven** (already the reference's shape:
  only REACHED generic FFI fns synthesize; an unreachable unmodellable fn
  never blocks the build).

**Why not the fresh demand-synthesis:** its 45 MB dead-text estimate assumed
76k wrappers; the measured reality is 3,534 bound wrappers for the full
stripe surface (20,358 visible symbols post-#89) — ~2 MB of cached text,
sliced by a linear text pass. The sentinel pipeline is shipped, has proven
fail-safe discipline, and the demand path would add build-time codegen +
cache-invalidation complexity for a cost that doesn't exist at measured
scale. **Banked as E7** with a measured trigger (cached `_bindings.rs`
> 10 MB per crate OR add-time emit > 30 s): the generator is a pure
(kernel.json, used-set) → text function either way, so the escalation changes
*when it runs*, not what it is. HM seeding of a 20k-symbol `.ipei` gets a
name→offset lazy-seed index IF profiling shows it matters (measure first).

---

## 8. Adjudications table (the record)

| # | Conflict | Chosen | Why |
|---|---|---|---|
| a | Fresh Affine handle vs reference Clone-gate + owned-API routing | **Reference** (Clone-gate; `!Clone` drops fail-closed; WALL-6 owned-API routing for borrowing builders). Affine banked as E5. | Faithful-port default; acceptance path needs no Affine (stripe/firestore types are Clone — fixtures 93/95/96/104); Affine trades compile-time affinity for runtime errors — adopt only on demonstrated need |
| b | Fresh demand-synthesized DCE vs reference sentinel slicing | **Reference** (sentinel emit-all + R-D/R-3/R-4 discipline). Demand-synthesis banked as E7 with measured trigger. | Proven at real scale (3,534 wrappers ≈ 2 MB, not 45 MB — fresh estimate off ~20×); fail-safe FULL-EMIT discipline already designed; escalation is a scheduling change, not a redesign |
| c | Global-runtime hardening vs reference runtime handling | **Fresh H1** (OnceLock global runtime) on top of the ported never-per-call-block_on invariant. | Verified gap: both runtimes build a fresh `Runtime::new()` per `block_on`; cross-`Task.run` client handles hit "reactor gone"; reference fixtures never exercised the shape; 15-line fix, behavior-compatible |
| d | Fresh 4 primitive shapes vs reference shape taxonomy | **Reference semantics, fresh names.** P/F/A/C adopted as documentation over the effect × fallible × self_returning matrix. | The matrix is what ships and what byte-diffs; the 4-shape frame is a lossless, clearer presentation |
| e | Fresh omissions | **Folded in from learnings:** de-async keystone (WALL-4/#81), tri-gate `Clone ≠ Send`, #89/#100 feature visibility, WALL-6 owned-API routing, WALL-G/H/I/J/K trait machinery, the two open ends. | All vendored inspector-side; the port is generator-side consumption |
| — | JoinError arm | **Fresh Δ2** (through `sky_error_from_foreign`) | Correlation-id traceability; strictly better; recorded divergence |
| — | Cancel propagation | **Fresh Δ1** (AbortOnDrop guard) | Preserves `task_parallel` no-side-effect-after-failure; kill-criterion documented |
| — | Sync-fallible Err arm | **Divergence from reference:** route through `sky_error_from_foreign`, not `format!("{:?}", e)` | Reference's sync arm still embeds Debug text (secret channel); same B8 rationale it applied to async |

---

## 9. Milestone plan — re-slotted into the umbrella's P-phases

The umbrella's "P7 async bridge LAST" is dissolved: async is not a trailing
milestone but part of the base emitter (the reference's own `Ffi.hs` emits
sync and async arms from one body function). M-G ceases to exist as a
separate module; its content lands inside M-D/M-E.

```
P0 inspector hardening ──────────────┐
P1 decode+coerce (M-A/M-C/M-B,       ├─► P5 driver + consumer wiring ─► P6 E2E ladder ─► P7 ACCEPTANCE
   async metadata fail-closed)       │        ▲ (needs M4 registry)      (sync 10 +        (skyshop-rs
P2 emitters M-D INCL. async arms ────┤        │                          firestore +       zero-shim +
   + H1 runtime + DE-RISK SLICE      │   P4 sandbox                      REAL stripe       used-set DCE)
P3 generics M-E (turbofish,          │        │                          E2E)
   WALL-G/H/I/J/K consumption) ──────┘────────┘
```

| Phase | Async-relevant deliverables (delta vs umbrella §6) | Gate |
|---|---|---|
| **P0** | Unchanged — the vendored inspector already carries every wall through WALL-K; hardening only (pin, lints, fuzz) | unchanged |
| **P1** | Wire decode now includes fail-closed `effect`/`is_async`/Send-verdict/feature-set fields (closed enums; unknown → hard error) | corpus incl. async fixtures' kernel.json |
| **P2** | M-D emitters WITH Shape A/C arms (port `Ffi.hs:997-1006` + `ownRefIdx` + `FfiInstance.hs:820-825` serde-in-async) + Δ1/Δ2 + AbortOnDrop helper + **H1 global runtime** + `compile_error!` panic-abort fence. **Contains the de-risk slice (§10).** | byte-diff vs reference artifacts for an async fixture (44-class) with Δ1/Δ2 enumerated in the diff filter; H1 regression test (handle across two `Task.run`s) |
| **P3** | M-E generics incl. `methodTurbofish` + consumption of WALL-G/H/I/J/K resolved verdicts | byte-diff vs fixtures 93/95/96 artifacts |
| **P4** | Sandbox — unchanged | unchanged |
| **P5** | Driver + consumer wiring — E2E rung 1 gains one async crate alongside semver | `ipe add` an async crate → build → run |
| **P6** | E2E ladder: 10 sync shim-free crates + **firestore direct (fixture-104 parity)** + **the REAL async-stripe end-to-end build — reference open end #1, never run upstream** (heavy multi-crate: async-stripe-core + facade + client-core + hyper/tokio; disk-check + timeout-bounded per repo rules) | stripe checkout-session + customer round-trip green |
| **P7** | **ACCEPTANCE — reference open end #2:** migrate skyshop-rs off all three shim crates (firestore path proven upstream; firebase mirrors firestore's async-trait shape; stripe proven in P6). Judgment residue relocates per the learnings verdict: `_status` keys die (typed `Task Error` + `Maybe`), env/client config moves to Sky code, `EmulatorTokenSource` (user trait impl) → Sky-side/stdlib helper. Verify used-set-only DCE (S4 slicing on all three crates; `IPE_DCE=0` residual documented, not a blocker) | **skyshop-rs: ZERO shims + used-set-only DCE = DONE** |

---

## 10. De-risk slice (FIRST, inside P2)

**Chosen: the fresh arm's FirestoreDb slice — `FirestoreDb::new` (Shape C) +
`get_obj` (Shape A + SerdeValue + turbofish + borrowAsRef)** — over
finishing the stripe E2E first. Reasons:

1. **It has a reference oracle; stripe doesn't.** Fixture 104 + the firestore
   direct-bind are green upstream, so our generator port byte-diffs against
   real artifacts. The stripe E2E was never run upstream — there is nothing
   to diff against, and its risk is cargo-build-weight, not mechanism.
2. **Two functions exercise every novel element of this doc:** async ctor
   with Send-opaque return (C), moved Send receiver, serde reduction +
   methodTurbofish, owned-coercion of borrows, JoinError path, B8 funnel,
   Δ1 abort guard, and the H1 executor contract (firestore's gRPC channel
   spawns internally and outlives a single `Task.run`).
3. Stripe then lands in P6 as pure verification on already-proven mechanism —
   exactly the reference's own residual framing.

Steps (each independently verifiable; no builds implied by this doc):
1. **H1** in `task.rs` + regression test (handle created in one `Task.run`,
   used in a later one).
2. Hand-write the two wrappers exactly as §1.1 emits them in a scratch
   bindings crate; drive from a 10-line Rust main. Gate: live round-trip +
   panic-injection → structured `Err` + abort test.
3. Teach the generator (P2 emitters) to emit them from the crate's real
   kernel.json; byte-diff against the hand-written pair AND against the
   reference's cached firestore artifacts (Δ1/Δ2 in the diff filter).
4. Wire one Ipê program through canon/lower/backend. Gate:
   `fetchProduct`-shaped Ipê code builds + runs.
5. Scale out: P3 generics → P6 stripe E2E → P7 skyshop.

---

## 11. Extensions register (specified, NOT v1)

| Id | What | Source | Trigger |
|---|---|---|---|
| E1 | `!Send`-future LocalSet actor thread (mpsc + oneshot) | fresh §10 | a target crate with `?Send` async-trait surface |
| E2 | borrowed-return clone-out (`ToOwned`) | fresh §10 | accessor-heavy crates |
| E3 | `impl Stream` pull handles (`next : StreamHandle -> Task Error (Maybe a)`) | fresh §10 | LLM token streams / Firestore listeners |
| E4 | error-tag surfacing (variant identifiers only; secret-free) → typed retry | fresh §7 | retry-policy demand |
| E5 | Affine take-once handle for `!Clone` consumed builders | fresh §6.3 | a needed crate drops on `!Clone` builder shape; guardian gate |
| E6 | `Arc<T>` repr for `!Clone` all-`&self` `Send+Sync` types | fresh §6.2 | same, for client-shaped types |
| E7 | demand-synthesized wrapper emission (used-set → text at build) | fresh §8.2 | cached `_bindings.rs` > 10 MB/crate or add-time emit > 30 s |

## 12. Soundness checklist (carried)

No `unsafe`/`unwrap`/panic in emitted glue — every failure arm a structured
`Err`. Foreign panics contained (JoinError → funnel; sync via catch_unwind;
`compile_error!` fence vs `panic=abort`). Secrets: B8 funnel on EVERY foreign
error arm, sync and async (this doc extends it to the sync-fallible arm the
reference left raw). Fail-closed: every non-admitted shape has a named drop
reason (§3.2); no `Ty::Any`; dynamic dispatch stays refused (panicking
polyfills = the wall). Cancellation: `Task.parallel` semantics extend to
foreign calls via Δ1.
