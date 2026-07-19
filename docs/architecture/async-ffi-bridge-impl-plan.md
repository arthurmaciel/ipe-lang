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
| firestore bind | done — 670 bindings importable; probe SEAL green (ipe 0 → cargo 0 → run folds to structured Err); used-set DCE 670 → 3. REGRESSION RE-CONFIRMED under W1 `--document-hidden-items` + W4: `ipe install` binds 832/836 shim-free (4 honest drops on `FirestoreTransaction<'a>` lifetime-parametric — NOT on skyshop used-set); probe SEAL re-run GREEN (ipe 0 → emitted `cargo build` 0), used-set DCE 832 → 2. |
| async-stripe build | send now binds for the create/checkout builders (was the last wall). Root cause found + fixed: the `send` Ok-payloads (`Customer`, `CheckoutSession`, …) live in `#[doc(hidden)] pub mod`s of `async-stripe-shared` (re-exported at the root) which DEFAULT rustdoc strips from the JSON entirely — so no crate could name / Send-prove / bind them, and every request builder's `send` silently dropped the async-Send output gate. Fix = (a) `--document-hidden-items` in `run_rustdoc_package` (shared JSON goes 8→17484 fns; `CheckoutSession` now visible with a clean unconditional synthetic Send impl even under `--all-features`), + (b) F3 cross-crate proven-Send set `GLOBAL_XC_SEND_NAMES` (`xc_send_proven`, unique-last-segment, fail-closed) so the sibling-owned payload's Send verdict reaches the consuming crate's C1 gate. STATUS — THREE of FOUR walls fixed; ONE remains (F2 cross-crate re-export ambiguity). The stripe `send` for the create/checkout builders passes through FOUR gates in `parse_fn_item`; each fix exposed the next: (W1) type-visibility — FIXED by `--document-hidden-items` (the payload types `Customer`/`CheckoutSession` live in `#[doc(hidden)] pub mod`s of `async-stripe-shared`, stripped by default rustdoc; the flag surfaces them, shared JSON 8→9150 fns). (W2) C1 async-Send OUTPUT gate cross-crate — FIXED by F3 `GLOBAL_XC_SEND_NAMES` + `xc_send_proven`. (W3) F3 re-export ambiguity — FIXED by `GLOBAL_XC_NONSEND_LASTSEGS` restricted to CRATE-LOCAL (`LOCAL_TYPE_IDS`) definitions (a sibling re-export must not falsely mark a segment non-Send). VERIFIED with a scoped DBG: `[XC-SEND-PROVEN] name="CheckoutSession" matches_send=true nonsend_poisoned=false => true` — the async-Send OUTPUT gate now ADMITS `CheckoutSession`. (W4 — REMAINING) the send STILL drops (authoritative send total = 1, retrieve only) because the RETURN-NAMEABILITY step (`type_to_typeref`, the F2 `GLOBAL_XC_PUBLIC_PATHS` xc_path resolution ~8663) has the SAME re-export ambiguity F3 hit: under doc-hidden, `CheckoutSession` is reachable-public from BOTH `stripe_shared` (definition) AND `stripe_checkout` (re-export), so the bare-last-segment lookup finds TWO proven-public paths → its strict UNIQUE-match fails-closed → the Output can't be named → the send drops. F2 was deliberately LEFT at strict-unique (sound) this session: unlike F3's Send-set (where every member is Send so any match proves Send), F2 must emit a COMPILABLE path, and two same-last-segment proven-public paths could be DIFFERENT types → picking one risks E0308. The proper fix needs a type-IDENTITY check (are the two paths the same defining type, i.e. a genuine re-export, vs two distinct same-named types) before collapsing — a small design that mirrors F3's non-poisoning discipline but keyed on defining-type identity, NOT attempted blind this session. Retrieve's send binds because its Output `RetrieveCustomerReturned` is CORE-LOCAL (nameable directly, no F2 needed). 233 inspector unit tests green (+2 F3, non-Send-guard reflected). GUARDIAN VERDICT on doc-hidden+F3 = SHIP-WITH-REGRESSION-CHECK (firestore 670-SEAL regression + async-stripe emitted-project compile still owed). `List<…>`-Output sends: honest over-drop (not on skyshop's used-set). Backlog item STAYS OPEN — W4 (F2 re-export type-identity) is the single remaining code wall to the create/checkout `send`, then SEAL probe, then skyshop. **W4 NOW LANDED (code + unit-verified):** the F2 strict-unique last-segment match is replaced by a DEFINING-TYPE-IDENTITY check. New `GLOBAL_XC_PUBLIC_PATH_BY_DEFID` maps each type's canonical `doc["paths"]` defining path (identical across a definer and every re-exporter) → its set of proven-public paths; `xc_public_path_for_last_segment` admits a genuine re-export (all candidate paths under ONE defining key → one deterministic path, lexicographically smallest) and fail-closes a real collision (≥2 distinct defining keys → distinct types → drop, no wrong-type pick). Both ambiguity sites (`resolved_path_is_bindable`, `type_to_typeref` xc_path) consult it; site 7289 (`rustdoc_type_to_rust_str` membership check) unchanged. Mirrors W3's non-poisoning discipline keyed on type identity, not Send-ness. Same soundness envelope as F2 (only owning-crate reachable-walk paths enter; defining key read from `doc["paths"]`, never reconstructed → private-module trap stays closed). 235 inspector unit tests green (+2 W4 fixtures: identity re-export admits + distinct-collision fails-closed, at helper and end-to-end `type_to_typeref` sites); scoped clippy clean. Commit `62877534`. END-TO-END `[XC-SEND-PROVEN]` + actual create/checkout `send` binding + stripe SEAL probe: verification pending the 6-crate manifest install completing (in flight). |
| firebase bind | pending |
| skyshop transpose | pending |
| closure | pending |

### 5a. Remaining-spec milestones (`async-ffi-bridge-remaining-spec.md`)

| Milestone | Status |
|---|---|
| maybe-coercion | done — `render_generic_wrapper` declares `IpeMaybe<T>` at Maybe-slot params (nested containers preserved) + shadows to host `Option<T>` via `ipe_maybe_to_option` before the call; `Option` OK lifts to `IpeMaybe` in `ok_lift`. `ipe_ffi` emission test + full suite green |
| canon-arity-gate | done — new IPE-N0031 + explain page; canon rejects a mis-arity built-in container (`List`/`Maybe`/`Set` arity 1, `Dict`/`Result` arity 2) at the empty-home resolution point, ahead of the lowerer ICE. `ipe_canon` + `ipe_diagnostics` + stdlib/negative/parametric goldens green |
| private-path-admission | done — `collect_external_trait_paths` routes the recorded external-trait def-path through the `external_type_public_path` proven-public / fail-closed gate; a non-std path threading a private module is dropped (→ `TraitUnreachable`), std + root-public kept. Inspector suite green (228) |
| stripe-send | TRUE root cause found (two prior diagnoses were WRONG); combined fix F3 + doc-hidden landed, live verification in flight. Diagnosis chain: (1) `send` is an INHERENT method on every request builder (`is_inherent=true`, NOT trait-projected) — flows through normal `parse_fn_item`. (2) The drop is the **C1 async-Send OUTPUT gate** (`is_provably_send_opaque_return`): `parse_fn_item("send", CreateCheckoutSession) => None`, `send(RetrieveCustomer) => Some`; the ONLY difference is the Ok-Output — retrieve returns core-local `RetrieveCustomerReturned`, create/checkout return `Customer` / `CheckoutSession` rendered BARE, unprovable-Send in the consuming crate. (3) **THE REAL WALL** (proven by generating `stripe_shared`'s rustdoc JSON directly): `Customer`/`CheckoutSession`/`PaymentMethod`/… — every `send` Ok-payload — are defined in `#[doc(hidden)] pub mod`s of `async-stripe-shared` and re-exported via `#[doc(inline)] pub use ::*`. **Default rustdoc STRIPS doc-hidden modules and ALL their structs from the JSON** (`stripe_shared.json` has ZERO structs; `CheckoutSession` substring count = 0). So the payload types are invisible to EVERY crate's inspection — not nameable, not Send-provable, not bindable from any manifest member. Neither F2 (nameability) nor a Send-proof-propagation alone can help: the type simply isn't in any JSON. FIX (two parts, both sound): (a) **doc-hidden** — pass `--document-hidden-items` to `run_rustdoc_package`; `#[doc(hidden)]` is a doc-visibility attr, not a privacy boundary, and these `pub` types are genuinely reachable. Verified: with the flag, shared's JSON gains 2053 structs incl. `CheckoutSession` (id 7796, attrs = `CfgAttrTrace` only — NOT individually doc-hidden, so the per-item `doc_hidden` belt-and-braces gate leaves it; synthetic POSITIVE Send impl present; path `stripe_shared::checkout_session::CheckoutSession`). (b) **F3** — manifest-run cross-crate proven-Send set `GLOBAL_XC_SEND_NAMES` (union of every member's own synthetic-/all-fields-/explicit-Send full public paths), consulted by `is_provably_send_opaque_return` + the generic-instantiation arg check via `xc_send_proven` (UNIQUE last-segment match, fail-closed) — needed because the payload type is owned by shared while the `send` is in core/checkout, so the Send verdict must cross crates. 231 inspector unit tests green. VERIFY IN FLIGHT: 6-crate manifest with both fixes → assert create/checkout `send` bind → SEAL probe. RISK/REVIEW: `--document-hidden-items` is a global posture change (guardian sign-off warranted — see review note); the per-item `doc_hidden` gate is the retained safety net. NOTE: `List<…>`-Output sends stay an honest over-drop (generic base `List`/`SearchList` cross-crate — separate larger fix, NOT on the skyshop used-set: skyshop uses only create-checkout-session + retrieve-session). |
| firebase-bind | pending — `ipe add rs-firebase-admin-sdk@4.3`; expected wall = the `HashMap<String, serde_json::Value>` claims return (extend the JSON-text lift to concrete serde-container returns); emulator security gate relocates into Ipê `Lib/Auth.ipe` (§3.3). |
| skyshop-transpose (ACCEPTANCE) | pending — new `examples/13-skyshop/` from `../sky/examples/rust/skyshop-rs/src/`; manifest per §4.1 PLUS `async-stripe-shared` + `async-stripe-client-core` members (stripe-send precondition); Lib boundary keeps sync `Result Error` + `|> Task.run` (R1); handle strategy probe (R2); firestore emulator token-source (§4.5); behavior e2e vs emulators. |

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
