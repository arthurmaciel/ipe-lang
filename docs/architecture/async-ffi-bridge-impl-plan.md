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
| skyshop-rs (Rust-backend, SHIMMED) | `../sky/examples/rust/skyshop-rs/` — `ipe.toml` binds `sky-firestore-shim` / `sky-stripe-shim` / `sky-firebase-auth-shim` via `file://` git; shim sources under `wrappers/` | reference, READ-ONLY; the de-shim was never done upstream |
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

### Milestone: foreign-type-one-home — the structural model for FFI type identity

**The structural diagnosis.** Every wall so far is one symptom repeated at
four layers: the SAME Rust type, reached via different crate paths (its
definer vs a re-exporter), was assigned a DIFFERENT identity —
- in *visibility* (doc-hidden stripping: the type invisible from every path),
- in *Send-proof* (the consuming crate could not prove a sibling-owned type),
- in *nameability* (two proven-public paths for one type → strict-unique
  fail-closed), and now
- in the *checker's nominal identity* (two `Rust.*` modules each declaring
  their own opaque `type Client` → `IPE-T0001` between values of one type).

The first three were fixed one wall at a time, and each fix converged on the
same key: **the type's canonical DEFINING path — the `doc["paths"]` entry
rustdoc records identically in every crate that can see the item** (the
"defid"). That convergence is the structural answer, not a coincidence:

> **A foreign type's identity IS its defining path.** One defid = one type,
> everywhere: in rendering, in Send-proof, in nameability, and in the Ipê
> type system. Rust itself works this way (a re-export is not a new type;
> rustc unifies by DefId), and Elm works this way on the Ipê side (a type
> has exactly ONE home module; other modules import it, never re-declare it).

**Chosen design (dissolves the nominal wall; pre-empts the next).** Extend
the defid SSOT from the inspector (where walls 1–3 already consult it) down
through the artifact chain to the interface layer, and give every foreign
type ONE Ipê home module — the Elm rule applied to generated modules:

1. **Inspector** emits, per member crate, `foreignTypeIds`: every foreign
   nominal path it rendered into a binding's type strings → that type's
   defid. Both sides already exist in memory (`REACHABLE_PATHS` +
   `EXTERNAL_TYPE_PATH_BY_ID` + `GLOBAL_XC_PUBLIC_PATH_BY_DEFID`, keyed from
   `doc["paths"]`, never reconstructed — the private-module trap stays
   closed). Filtered to paths actually used by emitted bindings.
2. **`PkgInfo`** decodes it; **`crate_interface`** joins it with the
   existing name→path map into `opaque_type_ids` (Ipê name → defid);
   **`consumer.json`** round-trips it (plus the structured binding list it
   already contains, now loaded fully).
3. **Catalog unification at build time** (new `ipe_ffi::unify`), invoked
   from `prepare_ffi` between catalog load and interface injection — the
   single seam `build`/`watch`/`lsp` all share. For each foreign type NAME
   surfaced by ≥2 member modules: unify iff (a) every surfacing member
   reports a defid and all defids AGREE (≥2 distinct defids = genuinely
   distinct same-named types → keep today's distinct nominals, correct
   as-is), (b) the defining crate resolves to ONE version across those
   members (checked against each member's recorded transitive-dep versions;
   disagreement → no unification — never risk an E0308 SEAL break), and
   (c) the induced interface-module import edge creates no cycle (names
   processed in sorted order; an edge that would close a cycle skips that
   name — deterministic, recorded).
4. **One home per unified type**: the member module whose own crate IS the
   definer (rendered-path first segment == defid first segment) becomes the
   type's home; ties/none → lexicographically-first surfacing module. Every
   OTHER member module stops declaring `type X = X` and instead gains
   `import <Home> exposing (X)` — its signatures' bare `X` then
   canonicalises to the home's nominal via the ordinary dep-type injection
   (`inject_dep_type`), so the checker sees ONE `Con` for the type with no
   checker change at all. Interface sources for demoted modules are
   re-rendered from the structured consumer data (never text-patched).
5. **Backend**: demoted modules' `opaque_types` entries drop; `foreign_types`
   then carries exactly ONE `Module.X → rust_path` per unified type (the
   home's own compilable path — the home crate is a direct dep by
   construction), so emission has a single SSOT too.

**Rejected alternatives.**
- *Alias re-export* (`type alias X = Home.X` in demoted modules): dep-alias
  bodies expand against the IMPORTING module's `qual_vars` (tier-1 gate in
  `canonicalise_type`), so a qualified alias body breaks in any user module
  that doesn't import the home itself; and the unqualified-alias-shadowing
  arm expands aliases BEFORE qualified resolution, risking mis-binding of a
  same-named DISTINCT type. Fragile where the import mechanism is exact.
- *Checker-level identity keyed on Rust path* (a `Ty::Foreign(defid)` kind):
  breaks the design's own keystone that FFI signatures flow through the SAME
  annotation→`Ty` path as user annotations (no second scheme table), and
  would special-case foreign types in every checker arm. The Elm one-home
  rule achieves identical unification with zero checker surface.
- *Post-hoc `IPE-T0001` fold* (checker treats equal-rust-path opaques as
  equal): same objection — nominal identity would depend on a lookup table
  outside the type system.

**Consequences.** A unified type is nameable in user annotations ONLY via
its home module (`import Rust.Async_stripe_client_core exposing (Client)`),
exactly as an Elm type is — the demoted modules no longer export it. Values
flow between all member modules' bindings with no annotation friction.
Same-named distinct types keep distinct nominals (correct). Old caches
without `foreignTypeIds` simply see no unification (today's behavior) —
re-running `ipe install` upgrades them.

### Milestone: param-shape-admission — general type-shape rules for rich builder surfaces

**The structural diagnosis.** The binder's param/field admission grew outward
from primitives: a parameter is admitted only when its type reduces to a
closed set of std shapes (numerics, `String`, `Vec`/`Option` of those). But
the same inspection ALSO binds nominal types as opaque handles — structs,
enums, typed-ID newtypes — with constructors, accessors, and one canonical
identity per defining path. The remaining checkout-flow gaps are all one
symptom: **a parameter or field whose type is a nominal the binder already
binds elsewhere is still refused, because the admission tables predate the
opaque-handle surface.** Concretely, on `rustdoc` type structure:

| Gap member | Rust shape | Missing admission class |
|---|---|---|
| `CreateCheckoutSession::line_items` | `impl Into<Vec<LocalCloneStruct>>` param | conversion-bound → Vec-of-bound-nominal |
| `CreateCheckoutSession::mode`, `LineItemsPriceData::new` | `impl Into<CrossCrateEnum>` param | conversion-bound → bound-nominal (identity) |
| `RetrieveCheckoutSession::new` | `impl Into<NewtypeId>` param, `NewtypeId: From<String>` | conversion-bound → String-convertible target |
| `CheckoutSessionMode` variant ctors | `#[non_exhaustive]` enum (enum-level) | enum-level non-exhaustive ≠ non-constructible |
| `CheckoutSession.status` / `.payment_status` | struct field of crate-local Clone enum | enum-typed field in the field closed set |

These are not one crate's quirks — they are the standard shapes of every rich
Rust API (builder setters over `impl Into<T>`, typed-ID newtypes with
`From<String>`, enums as params and as struct fields), so each rule below is
keyed ONLY on rustdoc type structure, never on a crate name.

**Admission classes (the design).**

1. **Identity conversion-bound admission.** A PARAM-position `Into<X>`/`From<X>`
   bound (named generic or anonymous `impl Trait`) whose target `X` is a
   *bindable nominal* — crate-local reachable, or cross-crate
   identity-provable via the defining-path map — or a `Vec` of one,
   substitutes the target itself. Soundness: `X: Into<X>` and `X: From<X>`
   hold for every `X` by std's reflexive `From` + blanket `Into`; the wrapper
   passes the owned Ipê-held value straight through, and every existing
   nameability / bindability / Send gate still applies downstream. RETURN
   position is untouched: an anonymous conversion-bound return produces an
   opaque value that merely *converts to* `X` — claiming `X` in the wrapper
   signature would be a latent type mismatch at cargo time, so it stays
   dropped.
2. **String-convertible target preference.** Checked BEFORE identity: if the
   target `X` carries a proven `impl From<String>` (std `From` resolved by
   id, `String` argument, target = a crate-local defined type; proofs are
   collected per member crate and unioned across the manifest run keyed by
   the target's defining path, same identity discipline as the Send and
   public-path maps), substitute `String`: `String: Into<X>` follows from
   `X: From<String>` via the blanket impl. The Ipê surface takes a plain
   `String` — exactly how typed-ID newtypes are meant to be called. This is
   direction-sensitive: it applies ONLY to `Into<X>` bounds (the wrapper
   *supplies* the String); a `From<X>` bound would need the opposite
   direction and falls through to identity. Fail-closed: no proof → class 1.
3. **Enum constructibility matches the language rule.** `#[non_exhaustive]`
   on an ENUM restricts *exhaustive matching* by downstream crates (already
   honoured: the tag/extractor wildcard) — it does NOT restrict construction
   of its variants. Only a variant-level `#[non_exhaustive]` (or a stripped /
   ineligible field) makes that variant non-constructible. Variant
   constructors are therefore emitted for enum-level non-exhaustive enums
   too; the variant-level suppression and the match-wildcard rule stand
   unchanged. Verified empirically cross-crate before landing (unit + tuple
   variants of an enum-level non-exhaustive foreign enum compile; the
   fixture also covers struct-kind variants).
4. **Enum-typed fields join the field closed set.** A struct field whose type
   is a crate-local `Clone` ENUM that the enum binder surfaces as an opaque
   handle (public, non-doc-hidden, non-generic) is getter/setter-eligible
   under exactly the conditions a `Clone` struct field is. Reading yields the
   opaque handle (dispatch Ipê-side via the tag accessor / `as_str`);
   writing assigns an owned handle by value — no numeric narrowing exists on
   a nominal, so the lossless-setter gate is unaffected.

`Vec`-of-opaque params need no fourth mechanism: the `Vec` arm of
conversion-bound resolution recurses into class 1, and Ipê `List X` already
lowers to `Vec<X>` at the wrapper boundary (exercised today by the bound
`List <opaque>` field setters).

**How this maps beyond the checkout flow.**
- *axum*: `Router`/`MethodRouter` builder methods pass bound nominals by
  value (class 1); config enums as params and fields (classes 1/3/4).
  Handler/extractor trait generics (`impl Handler`, `FromRequest`) are a
  distinct trait-bound class, out of scope and honestly dropped.
- *bevy*: app/plugin builders take Component/Resource structs by value
  (class 1); enums appear pervasively as params and struct fields
  (classes 1, 3, 4).
- Any crate with typed-ID newtypes over strings gets class 2 for free.

**What cannot be made general soundly (stays dropped, recorded).**
- Anonymous conversion-bound RETURNS (identity is unsound there: the callee's
  value is opaque, not `X`).
- Conversion targets carrying borrows (`impl Into<Cow<'_, str>>`,
  `Into<&'static str>`): a runtime-owned value cannot become a borrow.
- Fallible-conversion IDs (`TryFrom<&str>`-style, e.g. `HeaderName`): no
  blanket `Into`, and admitting them would need a new fallible-param surface
  — a recorded follow-up, never a silent admit.

**Filed follow-ups surfaced by this analysis (pre-existing, not blocking):**
- `FromStr` `from_string` bridges are absent from every generated interface
  while the sibling `Display` `to_string` bridges emit — the bridge is
  synthesized but never survives to `.ipei`; root cause not yet located.
- Cross-crate enum-typed FIELD admission (a field typed by a sibling crate's
  enum) needs the defining-path identity map at field-eligibility level;
  crate-local admission (class 4) does not cover it.

### Step: closure
- `cargo fmt --all`; full scoped gates; divergence ledger updated (Δ1, Δ2);
  `AGENTS.md` untouched unless surface changed; final report.

### Documented residual — live-emulator e2e (NOT RUN this session; no emulator/SDK installs)
The SEAL (`ipe` exit 0 ⇒ emitted `cargo build` exit 0) is the acceptance gate this
session pursues; the live round-trips below are an honest residual, to be run only where
the emulator/mock is available, never faked green:
- **firestore**: `gcloud emulators firestore start --host-port=localhost:8080`, then
  `FIRESTORE_EMULATOR_HOST=localhost:8080` + a dev service-account JSON via the
  `TokenSourceType::Json`/`GOOGLE_APPLICATION_CREDENTIALS` path (§4.5). Offline, the
  firestore probe already folds to the structured `Err` (verified: `ForeignError (ref …)`).
- **firebase auth**: `firebase emulators:start --only auth`, then
  `FIREBASE_AUTH_EMULATOR_HOST=localhost:9099`; mint a token via the emulator signUp REST
  endpoint (`Ipe.Http` from the probe or curl) and assert `validate` returns the claims
  JSON (§3.4). The emulator security gate MUST be Ipê-side (`Lib/Auth.ipe`) refusing the
  emulator host outside dev (§3.3) — a Security-principle review item on the transpose.
- **stripe**: `stripe-mock` on `:12111`, bound via `url_from_clientBuilder`
  (`Rust.Async_stripe.url_from_clientBuilder`); offline the send folds to the no-network
  structured `Err`. The probe chain (create customer → create/retrieve checkout session)
  is written at `~/.cache/ipe/ffi-probe-stripe/src/Main.ipe` and awaits the cross-crate
  foreign-type unification wall before it type-checks.

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
| async-stripe build | send now binds for the create/checkout builders (was the last wall). Root cause found + fixed: the `send` Ok-payloads (`Customer`, `CheckoutSession`, …) live in `#[doc(hidden)] pub mod`s of `async-stripe-shared` (re-exported at the root) which DEFAULT rustdoc strips from the JSON entirely — so no crate could name / Send-prove / bind them, and every request builder's `send` silently dropped the async-Send output gate. Fix = (a) `--document-hidden-items` in `run_rustdoc_package` (shared JSON goes 8→17484 fns; `CheckoutSession` now visible with a clean unconditional synthetic Send impl even under `--all-features`), + (b) F3 cross-crate proven-Send set `GLOBAL_XC_SEND_NAMES` (`xc_send_proven`, unique-last-segment, fail-closed) so the sibling-owned payload's Send verdict reaches the consuming crate's C1 gate. STATUS — THREE of FOUR walls fixed; ONE remains (F2 cross-crate re-export ambiguity). The stripe `send` for the create/checkout builders passes through FOUR gates in `parse_fn_item`; each fix exposed the next: (W1) type-visibility — FIXED by `--document-hidden-items` (the payload types `Customer`/`CheckoutSession` live in `#[doc(hidden)] pub mod`s of `async-stripe-shared`, stripped by default rustdoc; the flag surfaces them, shared JSON 8→9150 fns). (W2) C1 async-Send OUTPUT gate cross-crate — FIXED by F3 `GLOBAL_XC_SEND_NAMES` + `xc_send_proven`. (W3) F3 re-export ambiguity — FIXED by `GLOBAL_XC_NONSEND_LASTSEGS` restricted to CRATE-LOCAL (`LOCAL_TYPE_IDS`) definitions (a sibling re-export must not falsely mark a segment non-Send). VERIFIED with a scoped DBG: `[XC-SEND-PROVEN] name="CheckoutSession" matches_send=true nonsend_poisoned=false => true` — the async-Send OUTPUT gate now ADMITS `CheckoutSession`. (W4 — REMAINING) the send STILL drops (authoritative send total = 1, retrieve only) because the RETURN-NAMEABILITY step (`type_to_typeref`, the F2 `GLOBAL_XC_PUBLIC_PATHS` xc_path resolution ~8663) has the SAME re-export ambiguity F3 hit: under doc-hidden, `CheckoutSession` is reachable-public from BOTH `stripe_shared` (definition) AND `stripe_checkout` (re-export), so the bare-last-segment lookup finds TWO proven-public paths → its strict UNIQUE-match fails-closed → the Output can't be named → the send drops. F2 was deliberately LEFT at strict-unique (sound) this session: unlike F3's Send-set (where every member is Send so any match proves Send), F2 must emit a COMPILABLE path, and two same-last-segment proven-public paths could be DIFFERENT types → picking one risks E0308. The proper fix needs a type-IDENTITY check (are the two paths the same defining type, i.e. a genuine re-export, vs two distinct same-named types) before collapsing — a small design that mirrors F3's non-poisoning discipline but keyed on defining-type identity, NOT attempted blind this session. Retrieve's send binds because its Output `RetrieveCustomerReturned` is CORE-LOCAL (nameable directly, no F2 needed). 233 inspector unit tests green (+2 F3, non-Send-guard reflected). GUARDIAN VERDICT on doc-hidden+F3 = SHIP-WITH-REGRESSION-CHECK (firestore 670-SEAL regression + async-stripe emitted-project compile still owed). `List<…>`-Output sends: honest over-drop (not on skyshop's used-set). Backlog item STAYS OPEN — W4 (F2 re-export type-identity) is the single remaining code wall to the create/checkout `send`, then SEAL probe, then skyshop. **W4 NOW LANDED (code + unit-verified):** the F2 strict-unique last-segment match is replaced by a DEFINING-TYPE-IDENTITY check. New `GLOBAL_XC_PUBLIC_PATH_BY_DEFID` maps each type's canonical `doc["paths"]` defining path (identical across a definer and every re-exporter) → its set of proven-public paths; `xc_public_path_for_last_segment` admits a genuine re-export (all candidate paths under ONE defining key → one deterministic path, lexicographically smallest) and fail-closes a real collision (≥2 distinct defining keys → distinct types → drop, no wrong-type pick). Both ambiguity sites (`resolved_path_is_bindable`, `type_to_typeref` xc_path) consult it; site 7289 (`rustdoc_type_to_rust_str` membership check) unchanged. Mirrors W3's non-poisoning discipline keyed on type identity, not Send-ness. Same soundness envelope as F2 (only owning-crate reachable-walk paths enter; defining key read from `doc["paths"]`, never reconstructed → private-module trap stays closed). 235 inspector unit tests green (+2 W4 fixtures: identity re-export admits + distinct-collision fails-closed, at helper and end-to-end `type_to_typeref` sites); scoped clippy clean. Commit `62877534`. END-TO-END `[XC-SEND-PROVEN]` + actual create/checkout `send` binding + stripe SEAL probe: verification pending the 6-crate manifest install completing (in flight). **W4 NOW VERIFIED END-TO-END (commit `41c7a7ff`):** a real 6-crate async-stripe rc.6 install now EMITS the create/checkout `send` bindings — `send_from_customerCreateCustomer : … -> Client -> Task Error Customer`, `send_from_checkout_sessionCreateCheckoutSession : … -> Task Error CheckoutSession`, `send_from_checkout_sessionRetrieveCheckoutSession`, plus Update/Expire/Fund variants (core 345→351, checkout 1211→1215 bindings; the delta is exactly the newly-admitted cross-crate sends). Residual after the first W4 commit: only 2 of the 3 last-segment resolution sites consulted the identity map — the `rust_type` render path (`rustdoc_type_to_rust_str` ~7289, the source the C1 gate + emitter read) still used plain-set membership, so a submodule-defined re-export (`stripe_shared::customer::Customer`, fail-closed out of `EXTERNAL_TYPE_PATH_BY_ID` by `external_type_public_path` rule 5) rendered BARE → emitter absolutized `::Customer` → send dropped while `type_to_typeref` already named it. Fixed: site 7289 falls through to `xc_public_path_for_last_segment` too (all 3 sites agree). Also fixed the multi-crate `assemble_emit` dep-line unification the fully-bound manifest surfaced: (1) feature-union for same-version divergent-feature pins (`async-stripe-shared` bare vs `serialize`/`deserialize`), (2) transitive-version-conflict deferral to Cargo (`syn` 2.0.119 vs 3.0.0 from different member jails → dropped, direct-crate conflict still refused). 236 inspector + 11 ffi unit tests green. **NEXT WALL (blocks stripe SEAL + skyshop): cross-crate foreign-TYPE nominal unification.** `Client` (Rust `stripe::Client`, defined in client-core, re-exported by the umbrella) binds as TWO DISTINCT Ipê opaques — `Rust.Async_stripe_core.Client` (the `send` receiver's 2nd arg) and `Rust.Async_stripe.Client` (`new_from_client`'s result) — because each crate's `crate_interface` (`src/compiler/ffi/src/interface.rs`) declares its own opaque `type X` per module with NO cross-catalog unification. IPE-T0001 type mismatch on the SEAL probe (`Task.fromResult (Stripe.new_from_client …)` feeds `send_from_customerCreateCustomer` whose Client is the CORE nominal). Both map to the SAME `stripe::Client` Rust path, so they'd cargo-unify — the wall is purely the Ipê nominal split. FIX (design, not attempted blind): the interface/emit layer (`assemble_emit` builds `foreign_types` keyed `module.Type → rust_path`; `naming.rs` is the SSOT) must detect foreign types sharing an absolute Rust path across the catalog and canonicalize them to ONE Ipê opaque (declare once in a canonical/shared module + re-export, OR fold all references to a single nominal at type-check). This is the "TWO kernels naming the same foreign type unify nominally" precondition (remaining-spec §2.3). |
| firebase bind | **DONE, SEAL green + live run.** `rs-firebase-admin-sdk` 4.3 binds 304 shim-free. Three root-cause fixes unlocked the ID-token used-set: (1) the nameability retain no longer requires the FOLDED top-level Result Err arm to be nameable (the wrapper never spells it — `reqwest::Error` no longer drops `LiveValidator::new_jwt_validator`); (2) the CONCRETE serde-JSON claims lift — `serde_json::Value`, and a string-keyed `HashMap`/`BTreeMap` of it, becomes the same typed serde-Value node the generic reduction produces (recognised by DEFINING path via the new raw external-defpath index; Ipê surface = claims JSON `String`); (3) the method-level turbofish now comes ONLY from the explicit per-own-generic list (the legacy infer-from-serde-touch fallback stamped an E0107 turbofish on zero-generic concrete methods). Plus: `cargo metadata --filter-platform <host>` so macOS-only conditional deps (`system-configuration`) are no longer exact-pinned into the emitted manifest. Probe: `validate : JwtLiveValidator -> String -> Task Error String`; ipe 0 → cargo 0; RUN live: JWKS fetch, Invalid-token folds to typed `ForeignError`, exit 0; DCE 304 → 2. The EMULATOR validator's `validate` is shadowed by the duplicate-name interface collapse (only the Live impl's is importable) — emulator-path residual noted below. |
| foreign-type-one-home (the structural fix) | **LANDED + VERIFIED END-TO-END.** Design per the named milestone section: identity = rustdoc defining path, one Ipê home module per foreign type. Inspector emits `foreignTypeIds` (rendered path → defid, filtered to used paths, conflicting claims fail closed); `PkgInfo`/`CrateInterface`/`consumer.json` round-trip it; new `ipe_ffi::unify` collapses same-name+same-defid nominals at catalog assembly (guards: missing identity, distinct defids, defining-crate version disagreement among members that RESOLVED it — a member with no resolution saw the type only through the manifest-run xc index and is not evidence of a second type —, import-cycle); demoted modules re-render with `import <Home> exposing (T)` from structured consumer data; `run_build` consolidated onto `prepare_ffi` (its divergent inline copy skipped unification). VERIFIED: 6-crate stripe install → `Client` (defid `stripe::hyper::client::Client`) is ONE nominal; stripe SEAL probe green (ipe 0 → cargo 0, DCE → 5 wrappers) and RUNS (live 401 on a dummy key folds to typed `ForeignError`, exit 0). Firestore SEAL re-verified green. |
| firestore serde surface | **REGRESSION FOUND + FIXED.** The private-path-admission gate had fail-closed `serde::de::Deserialize` (external trait, one intermediate module) OUT of the trait-path map, so `is_serde_trait_bound` no longer recognised serde bounds — the ENTIRE serde document surface (`get_obj`/`update_obj`/`query_obj`/…) silently vanished from re-installs (the "Dropped: none" ledger never saw them — they dropped as `unmodellable-bound Deserialize` on the generic path). Fix: serde-trait identity falls back to the RAW defining path (`EXTERNAL_DEFPATH_BY_ID`) — an identity question, never an emitted path. Fresh install: 841 bindings incl. `get_obj_if_exists : FirestoreDb -> String -> String -> Maybe (List String) -> Task Error (Maybe String)`, `create_obj`/`update_obj`/`query_obj` + the full QueryParams builder. |
| skyshop transpose | **DONE — SHIM-FREE SEAL.** `examples/13-skyshop/` builds shim-free: `ipe build` exit 0 → emitted `cargo build` exit 0; `Lib/Db.ipe` on `Rust.Firestore`, `Lib/Auth.ipe` on `Rust.Rs_firebase_admin_sdk`, `Lib/Stripe.ipe` on the real `Rust.Async_stripe*` surface (create/retrieve checkout-session builders + async sends). Sentinel DCE: 51 wrappers of ~32.5k catalog bindings reach the emitted `src/ffi.rs`. `async-stripe` pins `features = ["default-tls"]` in `ipe.toml` — the all-features inspection surfaces several client concretes, making the unique-impl monomorphisation of the async `send`s ambiguous (blocking-only surface); the explicit default pin restores the tokio-hyper `Client` + `Task`-typed sends. |
| stripe-builder-surface | **FELL — all five member classes bind and land in the authoritative `pkg.json`:** (a) `line_items` (conversion-bound → Vec-of-bound-nominal), (b) `mode` (conversion-bound → cross-crate enum, identity class), (c) `RetrieveCheckoutSession::new` (typed-ID via the `From<String>` preference — Ipê surface takes a plain `String`; the wrapper passes the OWNED `String`, which satisfies `impl Into<Id>` where `&String` does not), (d) the `LineItemsPriceData` ctor (`impl Into<stripe_types::Currency>`, identity class), (e) `status`/`payment_status` field accessors (Clone-enum fields). Emission fixes that completed the SEAL: checked-setter surface carries the `Result` layer the wrapper renders; the generic-instance OK lift recurses through `Option`/`Vec` (container-nested serde payloads re-serialise to JSON text); UI cfg-record kernels hoist arguments in ownership-walk order; fn params flowing into sync-capture kernel args promote to the `Arc` carrier; an inline lambda in an `Input.*` callback slot goes straight into `Arc::new` (one closure boundary). |
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
