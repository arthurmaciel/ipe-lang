# Async-FFI auto-bridge — executable implementation plan

> Companion to the design of record, `async-ffi-bridge-design.md` (AUTHORITATIVE).
> This plan maps that design onto the CURRENT `ipe_ffi`/`ipe_sandbox`/runtime
> state and sequences the remaining work to the acceptance metric:
> **async-stripe + firestore + firebase-auth auto-FFI-bound shim-free, AND
> skyshop transposed to Ipê building with ZERO shims + used-set-only DCE.**

## 0. Located paths (acceptance targets)

| Artifact | Exact path | Status |
|---|---|---|
| skyshop Ipê source (Go-deps original) | `../sky/examples/13-skyshop/` (`src/` with `.sky` modules, `src/Lib`, `src/Page`, `src/Ui`, `static/`) | reference, READ-ONLY |
| skyshop-rs (Rust-backend, SHIMMED) | `../sky/examples/rust/skyshop-rs/` — `sky.toml` binds `sky-firestore-shim` / `sky-stripe-shim` / `sky-firebase-auth-shim` via `file://` git; shim sources under `wrappers/` | reference, READ-ONLY; the de-shim was never done upstream |
| our repo `examples/13-skyshop` | **does not exist** — the `13` slot is free; the transposition target is `examples/13-skyshop/` (new) | to create (skyshop-transpose step) |
| our partial conversion | `examples/39-ffi-skyshop-core/` — domain core only; sync `uuid` auto-bound (checked-in `.ipe/cache/ffi/rust/`), firestore/firebase stdlib-swapped to `Ipe.Db`/`Ipe.Auth`, stripe absent | keep as the sync-ladder example; NOT the acceptance vehicle |
| shim-free target crates (from the shim manifests) | `firestore = "0.49"`; `async-stripe`/`async-stripe-types`/`async-stripe-checkout` (feature `checkout_session`)/`async-stripe-core` (feature `customer`) all `=1.0.0-rc.6`; `rs-firebase-admin-sdk = "4.3"` | to auto-bind |
| used-set (what skyshop actually calls, from the shim surfaces) | firestore: get/set/delete doc, query, query-where(+order) (~8 ops); stripe: create checkout session, create customer, retrieve session (3 ops); firebase: verify ID token (1 op) | drives used-set DCE proof |

## 1. Current-state audit (what is already landed)

- `src/compiler/ffi` (`ipe_ffi`): validated-newtype `PkgInfo` decode
  (`pkginfo.rs`), call-AST decode + `IPE-F4400` (`call.rs`, `typeref.rs`),
  saturating `num_coerce.rs`, the three emitters (`bindings.rs`, `emit.rs`,
  `interface.rs`; naming SSOT + sentinels + `compile_error!` panic-abort
  fence), generic instances (`instance.rs`), driver (`driver.rs`:
  `ipe add/install/remove`, trust gate, `GitSource` parse, cargo dep lines,
  `shake_bindings` sentinel DCE).
- `src/compiler/sandbox` (`ipe_sandbox`): jail landed — two-phase no-egress
  jail, bwrap-or-refuse, caps. **Do not regress; every [STRICTER] point stays.**
- Consumer wiring: `ForeignCall` canon node, `Callee::Ffi` IR, backend FFI
  emission + `src/ffi.rs` injection + `reached_ffi_idents`/`shake_ffi_by_fn_ident`
  used-set slicing (`src/compiler/backend/rust/src/project.rs`).
- Async arms (the reference wrapper port) already emit in
  `bindings.rs::plain_lines`:
  `Box::pin(async move { … tokio::task::spawn(…).await … })`, three-arm
  fallible / two-arm infallible, `Ok(Err(e))` through `ipe_error_from_foreign`
  (the sync-fallible arm too — the recorded strictly-better divergence).
- Baseline: `cargo nextest run -p ipe_ffi -p ipe_sandbox` = 149/149 green.

## 2. Gaps vs the design of record

| Gap | Design § | Where |
|---|---|---|
| Δ2 JoinError funnel — both async arms end in `str_err("foreign async call panicked")`, not `ipe_error_from_foreign(join_err)` | §1.1 | `bindings.rs` async arms + any generic-instance async path in `instance.rs` |
| Δ1 AbortOnDrop cancel guard — absent | §1.1 | emitted preamble helper + both async bodies |
| H1 global tokio runtime — `block_on` builds a fresh `Runtime::new()` per call | §4 | `src/runtime/rust/src/task.rs` |
| Real async-stripe end-to-end build (reference open end 1) | §9 | new fixture/example |
| firestore + firebase direct bind proof in OUR pipeline | §9, §10 | `ipe add firestore` / `rs-firebase-admin-sdk` |
| skyshop de-shim transposition (reference open end 2) | §9 | `examples/13-skyshop/` (new) |

## 3. Steps (each = commit + gate; foreground timeout-wrapped builds in `~/.cache/ipe/lane-2-target`)

### Step: join-error funnel (design Δ2)
- `bindings.rs`: both async arms (`plain_lines` Effect::Effectful) replace
  `Err(_) => …str_err(…)` with `Err(join_err) => IpeResult::Err(ipe_error_from_foreign(join_err))`.
  Same change on the generic-instance async body in `instance.rs` if present.
- Runtime precondition: `ipe_error_from_foreign` accepts any `Debug` type
  (`tokio::task::JoinError` is `Debug`) — verify bound, no runtime change expected.
- Update the in-crate emission tests' expected strings; record the divergence
  row in `docs/divergences-from-sky.md` (reference emits bare `str_err`).
- Gate: `cargo nextest run -p ipe_ffi`; scoped clippy.

### Step: abort-on-drop cancel guard (design Δ1)
- Emit a preamble helper (outside sentinels, kept unconditionally) in
  `<crate>_bindings.rs` when the package has ≥1 effectful fn:
  `struct AbortOnDrop(Option<tokio::task::AbortHandle>)` with
  `fn defuse(mut self)` and `Drop` aborting when armed. ~10 lines, no unsafe,
  no unwrap.
- Async bodies become: bind `handle`, arm guard, `handle.await`, defuse, match.
- Kill criterion honoured per design: none of the three target crates is in a
  corrupt-shared-channel class; no degrade path needed now.
- Gate: emission tests assert guard text; runtime test proving an aborted
  outer task aborts the inner spawn (small tokio test in `runtime`).

### Step: process-global tokio runtime (design H1)
- `src/runtime/rust/src/task.rs`: `static` `OnceLock<tokio::runtime::Runtime>`;
  `block_on` drives on the global runtime through the existing spawned-thread
  panic-isolation wrapper. `block_on_current_thread` untouched (webview rule).
  Runtime-construction failure keeps the existing structured-`Err` path.
- Regression test: construct a reactor-owning value inside one `block_on`, use
  it inside a later `block_on` — passes only with a shared reactor.
- Gate: `cargo nextest run -p sky-runtime-rust` (+ `--features full` locally
  for the touched surface).

### Step: firestore direct bind (de-risk slice; has a reference oracle — the owned-query-builder fixture + upstream direct-bind)
1. `ipe add firestore` (jailed fetch → inspect; `--yes`; timeout-bounded;
   heavy: gcloud-sdk closure) in a scratch project; verify kernel.json carries
   the de-async verdicts (`effect=effectful`, Send tri-gate, `FirestoreDb`
   Clone-opaque, feature set).
2. Probe Ipê program: the async constructor (opaque-returning) + a doc get
   (async method + serde reduction) chained with `Task.andThen`; `ipe build` →
   emitted project `cargo build` (THE SEAL). Live round-trip only if the
   firestore emulator is available; otherwise the structured-`Err` path (no
   network) is the run gate.
3. Compare wrapper semantics against the reference owned-query-builder fixture
   artifacts (Δ1/Δ2 enumerated in the diff filter).

### Step: async-stripe rc.6 end-to-end build (reference open end 1)
- `ipe add` the four crates (versions/features exactly as the shim manifest:
  `=1.0.0-rc.6`; `checkout_session`, `customer`). Multi-crate manifest mode;
  feature visibility (the feature-enumeration class) must surface the
  `Create*` builders + `send`.
- Probe Ipê program covering the skyshop used-set: create customer, create
  checkout session, retrieve session — `Task.andThen` chain, no chain
  synthesis. `ipe build` → `cargo build` green = the never-run-upstream
  end-to-end build done. Runtime smoke: no-network structured `Err` (or
  stripe-mock via `ClientBuilder.url` if bindable and available).

### Step: firebase bind (rs-firebase-admin-sdk 4.3)
- `ipe add rs-firebase-admin-sdk`; surface = ID-token verification (async-trait
  de-async keystone). Probe program + SEAL as above.

### Step: skyshop transposition (`examples/13-skyshop/`, ACCEPTANCE)
- Transpose `../sky/examples/13-skyshop` (`.sky` → `.ipe`, `Sky.*` → `Ipe.*`
  per our stdlib surface) with `[rust.dependencies]` naming the REAL crates —
  zero shim crates anywhere in the tree.
- Judgment residue relocates per the design: `_status` keys die (typed
  `Task Error` + `Maybe`); env/client config in Ipê code; emulator token
  source → Ipê-side/stdlib helper (NOT a shim crate).
- Check in the `.ipe/cache/ffi/rust/` artifacts (the `39-ffi-skyshop-core`
  precedent) so the build is network-free; used-set DCE proof: emitted
  `src/ffi.rs` contains ONLY the reached wrappers, not the thousands bound.
- Gate: `ipe build` → emitted `cargo build` → run; behavior exercised as far
  as local emulators permit; every gap honestly recorded, never shimmed.

### Step: closure
- `cargo fmt --all`; full scoped gates; divergence ledger updated (Δ1, Δ2);
  `AGENTS.md` untouched unless surface changed; final report.

## 4. Standing constraints (from PRINCIPLES/DEVELOPMENT, non-negotiable here)

- Sandbox untouched; inspector runs ONLY jailed; `--yes` documented.
- No background work; every build `timeout`-wrapped; `CARGO_TARGET_DIR=~/.cache/ipe/lane-2-target`.
- No shims, no hacks: a hand-written Rust crate between Ipê and the SDK
  ANYWHERE fails acceptance. Over-drop is the only sanctioned degradation, and
  any drop that blocks the used-set is a root-cause item, not a workaround.
- THE SEAL: `ipe` exit 0 ⇒ emitted `cargo build` succeeds, for every probe and
  for skyshop.

## 5. Progress checkpoint

| Step | Status |
|---|---|
| join-error funnel (Δ2) | done — both async arms (plain + generic-instance) route `JoinError` through `ipe_error_from_foreign`; divergence B-FfiAsyncBridge recorded |
| abort-on-drop guard (Δ1) | done — `AbortOnDrop` in runtime `task.rs`; emitted async bodies arm + defuse; abort-propagation regression in `src/runtime/rust/tests/ffi_async_bridge.rs` |
| global runtime (H1) | done — `OnceLock` global runtime drives `block_on`; cross-entry reactor-handle regression |
| crate version pins (prereq) | done — `CrateSpec`/`VersionPin` (`name@version` → inspector); `ipe install` honours manifest inline-table pins + features + `--allow-build-scripts` |
| firestore bind | done — 670 bindings importable; probe SEAL green (ipe 0 → cargo 0 → run folds to structured Err); used-set DCE 670 → 3 |
| async-stripe build | partial — all four rc.6 crates inspect + bind (17/167/200/1209 bindings; the builder setter/ctor surface incl. `CustomerCreateCustomer.new/email/name` and the full checkout params tree is importable); `send` bound for ONE receiver (`send_from_customerRetrieveCustomer : CustomerRetrieveCustomer -> Client -> Task Error CustomerRetrieveCustomerReturned`) but NOT for the `Create*`/checkout builders — see §6 next steps |
| firebase bind | pending |
| skyshop transpose | pending |
| closure | pending |

## 6. Session notes (exact next steps if resuming)

- Runtime full-features suite 1047/1047 green after H1/Δ1; `ipe_ffi` 141,
  `ipe --lib` 85, seal test 3/3, scoped clippy clean.
- firestore 0.49 auto-bound: 205 → **670 importable bindings** after the
  consumer fixes below; the whole skyshop firestore used-set is importable
  shim-free (`with_options_from_firestoreDb : FirestoreDbOptions -> Task
  Error FirestoreDb`, `get_obj… -> Task Error String` (JSON text),
  `update_obj…`, `delete_by_id… -> Task Error ()`,
  `query_obj… -> Task Error (List String)` + owned QueryParams builder).
- Consumer fixes that unlocked it:
  1. closed (zero-type-param) generic instances — the async-trait surface —
     synthesize at add time into sentinel regions
     (`bindings.rs::closed_instance_lines`);
  2. generic Result-alias see-through in the inspector's CALL-AST builder
     (`type_to_typeref`), so the fallible layer folds instead of surfacing an
     opaque `FirestoreResult`;
  3. surface peel of ONE `Result <err>` layer is error-name-agnostic
     (`emit.rs::peel_result_layer`) — the wrapper always folds the foreign
     error, so the surface never re-states it;
  4. reserved-collision scan: ipe-syntax strings scanned with the ipe-aware
     opaque scan (builtin heads are containers, not foreign nominals); bare
     std carriers (`String`, `Vec`, …) are never foreign nominals;
  5. enum-variant extractor sigs parenthesize multi-word payloads
     (`Maybe (List FirestoreValue)` — a bare application ICE'd the lowerer);
  6. interface skips now land in `coverage.md` (the over-drop ledger was
     blind to the interface layer);
  7. jail caps env-overridable with warning (`IPE_FFI_{RSS_MB,CPU_SECS,
     WALL_SECS,FD_CAP,PROC_CAP,OUT_CAP_MB}`) — SDK-scale closures exceed the
     small-crate defaults.
- FILED FOLLOW-UP (compiler, pre-existing): an ill-formed injected interface
  sig (`Maybe List FirestoreValue`) ICEs the lowerer (`IPE-I0001`, "type
  constructor `List` with empty home") instead of a canon arity/type error —
  root-cause in canon, not FFI.
- Probe DONE end-to-end: `~/.cache/ipe/ffi-probe-firestore/src/Main.ipe`
  (options ctor → `with_options` → `get_obj` chain, `Task.onError` fold) —
  `ipe build` 0 → emitted `cargo build` 0 (THE SEAL) → run prints
  `ForeignError (ref …)` server-side and the Ipê-side structured-Err message.
  Used-set DCE proven: emitted `src/ffi.rs` = 3 wrappers of 670.
- Stripe probe project: `~/.cache/ipe/ffi-probe-stripe/` (four
  `=1.0.0-rc.6` crates via `[rust.dependencies]` inline tables; ONE-shot
  manifest install).
- NEXT STEP (stripe `send` — the ONE remaining wall for the used-set): the
  provided-method projection (`project_trait_default_methods`,
  `tools/ipe-ffi-inspector/src/main.rs` ~11313) bound `send` only for
  `CustomerRetrieveCustomer`. Debug plan: re-run the inspector on
  `async-stripe-core@=1.0.0-rc.6` keeping the rustdoc JSON, and check
  (a) whether `impl StripeRequest for CustomerCreateCustomer` reaches the
  projection arm (`trait_self_concrete` + `trait_node.is_some()` at ~2259),
  (b) whether the drop is `trait-method-default-where-unsatisfied` or a
  downstream `route_concrete_method` / Send-gate drop (add temporary drop
  logging), (c) whether cross-crate `Self::Output = stripe_shared::Customer`
  resolution fails where the core-local
  `CustomerRetrieveCustomerReturned` succeeds — if so, extend
  `impl_assoc_bindings`/nameability to the external-type path map (the
  types crate IS in the manifest run's xc index).
- THEN: stripe probe Main.ipe (client builder → create customer →
  checkout-session create/retrieve via `send`) → SEAL → run (no-network
  structured Err or stripe-mock via `url_from_clientBuilder`).
- THEN: `rs-firebase-admin-sdk@4.3` bind (async-trait de-async; mirrors
  firestore) + probe/SEAL.
- THEN: skyshop transpose into `examples/13-skyshop/` (`.sky` → `.ipe`,
  `[rust.dependencies]` = firestore 0.49 + the four stripe crates +
  rs-firebase-admin-sdk 4.3; check in `.ipe/cache/ffi/rust`; judgment
  residue per design §9: `_status` dies, env/config in Ipê, emulator token
  source needs an Ipê-side answer — `TokenSourceType::ExternalSource`
  is a trait object (unbindable), so the emulator path likely needs a
  runtime-owned token-source helper, which is runtime code, NOT a shim
  crate).
- FILED FOLLOW-UPS (from this session, all pre-existing classes):
  1. lowerer ICE on ill-formed injected sig (canon should reject);
  2. private-trait-path UFCS wrappers (`fluent_api` class) should over-drop
     at admission (currently only unreachable-wrapper DCE keeps builds
     green);
  3. `Maybe`-slot params of synthesised closed-instance wrappers take raw
     `Option<…>` while forwarders pass `IpeMaybe<…>` (E0308 if reached) —
     needs the owned-coercion layer in `render_generic_wrapper` param/ret
     positions (`update_obj`'s `Maybe (List String)` args hit this once
     skyshop reaches them).
