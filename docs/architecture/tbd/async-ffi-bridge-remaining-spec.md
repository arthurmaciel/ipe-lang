# Async-FFI auto-bridge — remaining-work spec + executable impl plan

> Companion to `async-ffi-bridge-design.md` (AUTHORITATIVE design of record)
> and `../async-ffi-bridge-impl-plan.md` (the prior plan; its progress
> checkpoint and session notes are the baseline this spec starts from). This
> document specifies the FOUR remaining items to the acceptance metric and
> sequences them for an implementer. It is a spec: no code has changed under
> it.
>
> **ACCEPTANCE METRIC (unchanged, restated verbatim):** skyshop transposed
> into a NEW `examples/13-skyshop/` building and running with **ZERO shim
> crates** (firestore 0.49 + async-stripe rc.6 + rs-firebase-admin-sdk 4.3
> auto-FFI-bound direct) + **used-set-only DCE** (emitted `src/ffi.rs`
> contains only the reached wrappers), with **THE SEAL holding end-to-end**
> (`ipe` exit 0 ⇒ emitted `cargo build` succeeds) for every probe and for
> skyshop itself. A hand-written Rust crate between Ipê and an SDK anywhere in
> the tree fails acceptance. Over-drop is the only sanctioned degradation; a
> drop that blocks the used-set is a root-cause item, never a workaround.

## 0. Baseline (what is already true when this spec starts)

- Δ2 JoinError funnel, Δ1 `AbortOnDrop`, H1 process-global tokio runtime:
  landed (`src/runtime/rust/src/task.rs`; async arms in
  `src/compiler/ffi/src/bindings.rs::plain_lines` and
  `src/compiler/ffi/src/instance.rs::render_generic_wrapper`).
- firestore 0.49 direct bind: 670 importable bindings; probe SEAL green;
  used-set DCE proven 670 → 3. The whole skyshop firestore used-set is
  importable (`with_options_from_firestoreDb`, `get_obj…`, `update_obj…`,
  `delete_by_id…`, `query_obj…` + owned `QueryParams` builder).
- stripe rc.6: all four crates (`async-stripe`, `async-stripe-types`,
  `async-stripe-checkout` [`checkout_session`], `async-stripe-core`
  [`customer`], all `=1.0.0-rc.6`) inspect + bind (17/167/200/1209); the
  builder setter/ctor surface is importable. `send` is bound for exactly ONE
  receiver (`CustomerRetrieveCustomer`, whose `Output` is the **core-local**
  `CustomerRetrieveCustomerReturned`) and NOT for the `Create*`/checkout
  builders (whose `Output` is **cross-crate**, e.g. `stripe_shared::Customer`).
- firebase bind, skyshop transposition: not started.
- Three filed pre-existing defects (the defects milestone below).
- Probe assets (may still exist; regenerate if pruned):
  `~/.cache/ipe/ffi-probe-firestore/`, `~/.cache/ipe/ffi-probe-stripe/`.

Standing constraints for every milestone (from `PRINCIPLES.md` /
`DEVELOPMENT.md`, non-negotiable): sandbox posture untouched (two-phase
no-egress jail, bwrap-or-refuse, caps; inspector runs ONLY jailed; the F1–F6
posture and every [STRICTER] point stay); no background work; every build/test
`timeout`-wrapped; own `CARGO_TARGET_DIR` under `~/.cache/ipe/`; no `unsafe`,
no `unwrap`/`panic` in emitted glue; every non-admitted shape drops with a
named reason into `<slug>.coverage.md`.

## 1. Milestone order and why

```
canon-arity-gate ────────────┐  (independent, small, closes an ICE class)
private-path-admission ──────┤  (inspector-only, fixture-tested)
maybe-coercion ──────────────┤  BLOCKS skyshop-transpose (update_obj)
stripe-send ─────────────────┤  BLOCKS skyshop-transpose (stripe used-set)
firebase-bind ───────────────┤  BLOCKS skyshop-transpose (auth used-set)
skyshop-transpose (ACCEPTANCE)┘
closure (ledgers, gates, report)
```

The three defect fixes first: all are cheap, independently verifiable, and
two of them (maybe-coercion directly; the other two as trust-in-the-walls)
sit on the skyshop path. stripe-send and firebase-bind are independent of
each other; run stripe-send first (it is the only item with real design
risk). skyshop-transpose consumes everything. Each milestone = its own
commit(s) + cheap gate; the batch certifies on the full gate.

---

## 2. stripe-send — `send` for the `Create*`/checkout builders

**Goal:** `send` bound for every skyshop-used request builder:
`CustomerCreateCustomer` (create customer), the checkout-session create
builder, and the checkout-session retrieve builder (already-bound receiver
class), each as
`send_from_<recv> : <Recv> -> Client -> Task Error <Output'>`.

### 2.1 Mechanism map (exact anchors, `tools/ipe-ffi-inspector/src/main.rs`)

| Piece | Anchor |
|---|---|
| Provided-method projection call site + guard (`!is_inherent_impl && trait_self_concrete && (self_mono_subst.is_some() \|\| trait_node.is_some())`) | ~2243–2314 |
| `project_trait_default_methods` (fail-closed: crate-local trait DEF only via `index.get(&trait_id)`, has-body, dedupe, where-bound gate, numeric-param gate) | ~11333 |
| `impl_assoc_bindings` (impl's `type Output = …` map) | ~9759 |
| `subst_assoc_json` (resolve `Self::Output` projections) | ~8286 |
| `sibling_impl_assoc` (WALL-J cross-impl assoc resolution, id-matched) | ~9813 |
| `route_concrete_method` → `build_trait_ctx` (UFCS qualifier; `TraitUnreachable` drop) | ~9729 / ~9665 |
| External path nameability: `external_type_public_path` — its terminal arm fail-closes ANY non-std external multi-segment path | ~5407 |
| `reachable_external_type_path` / `EXTERNAL_TYPE_PATH_BY_ID` (external ids present in THIS crate's `doc["paths"]`) | ~5460 |
| Cross-crate accumulator precedent: `GLOBAL_XC_IMPLS` + `mirror_into_global_xc_index` (populate pass across the manifest run, then bind-for-real) | ~621–629, ~3013 |

The known asymmetry: the bound receiver's `Output` is core-local (resolved
through `REACHABLE_PATHS`); the unbound receivers' `Output` lives in a
SIBLING crate of the same manifest run (`stripe_shared::Customer`,
`stripe_checkout::CheckoutSession`-class types).

### 2.2 Step 1 — diagnose (do NOT fix blind)

1. In a scratch project (reuse `~/.cache/ipe/ffi-probe-stripe/`), re-run the
   manifest install (`ipe install`, `--yes`, timeout-wrapped) with
   `IPE_FFI_DBG=1`, teeing output to a log file, and KEEP the rustdoc JSON
   (the jail's inspect output) for `async-stripe-core` and
   `async-stripe-checkout`.
2. Read each crate's `<slug>.coverage.md` drop ledger and the DBG log for
   `send`, per receiver. Classify against the decision tree below.
3. Confirm in the retained rustdoc JSON: (a) which crate holds the
   `StripeRequest` trait DEF (`index` vs only `paths`), (b) the
   `impl StripeRequest for CustomerCreateCustomer` node's `Output` binding,
   (c) whether `stripe_shared::Customer`'s id appears in
   `doc["paths"]` of the consuming crate.

### 2.3 Decision tree → fix designs (all fail-closed)

**Branch D1 — projection never runs: trait DEF is cross-crate.**
`project_trait_default_methods` returns `None` at
`index.get(&trait_id)?` when `StripeRequest` is defined in a sibling manifest
crate (rustdoc `index` is local-only). *Fix F1:* mirror **trait DEFS with
their provided-method `fn_data`** into a manifest-run global (new
`GLOBAL_XC_TRAIT_DEFS`, keyed by the same canonical-path normalizer as
`GLOBAL_XC_IMPLS`; populated in the existing populate pass alongside
`mirror_into_global_xc_index`). `project_trait_default_methods` falls back to
it when the local `index` misses — **manifest-member crates only, never std**
(a std trait def is absent from every member's mirror by construction, so the
existing "can't pull in `Iterator::*`" property is preserved). The projected
`fn_data` then flows through the SAME de-async → route → parametric path.

**Branch D2 — dropped inside the projection.** Ledger tag
`DefaultMethodWhereUnsatisfied` or the numeric-param drop. If the offender is
a where-predicate whose subject became CONCRETE via the assoc substitution
(`where Self::Output: …` after `type Output = stripe_shared::Customer`),
extend the trivially-true-predicate stripper (~11442–11453) to also retain
only `{generic:…}`-subject predicates AFTER `subst_assoc_json` — the same
soundness argument as the `Self`-subject case: the impl's existence proves
the bound. A predicate on a genuinely-unresolvable subject still drops.

**Branch D3 — `TraitUnreachable` from `route_concrete_method`.**
`ufcs_trait_path_with_args` cannot resolve the external trait path (same
root cause class as branch D4, applied to the trait instead of the Output
type). Fix is F2 below applied to the trait-path resolution site.

**Branch D4 — the Output type is unnameable (checkpoint hypothesis, most
likely).** `Self::Output` resolves to `stripe_shared::Customer`, but
rendering drops: the external id is either absent from the consuming crate's
`EXTERNAL_TYPE_PATH_BY_ID` or its path fail-closes in
`external_type_public_path`'s terminal arm (non-std external multi-segment).
*Fix F2 — manifest-run cross-crate public-path map:* during a multi-crate
manifest install, record each member crate's OWN verified public paths
(the product of ITS `collect_reachable_paths` walk — root-public re-exports
included) into a global map (`GLOBAL_XC_PUBLIC_PATHS`: canonical
`crate::…::Type` → proven public path, plus id-keyed entries). Consult it:

- in `reachable_external_type_path` / the typeref nameability gate, before
  dropping an external id;
- in `external_type_public_path`, as a lookup BEFORE the terminal fail-close
  (which remains the default for crates outside the manifest run).

Soundness rule: only paths PROVEN public by the owning crate's own
reachable-path computation enter the map — never a def path reconstructed
from `paths` (that is exactly the private-module trap the
private-path-admission fix closes). The opaque-type declaration side needs
no new mechanism: the consuming kernel's signature references the opaque by
its Ipê name and the emitted wrapper by its `::stripe_shared::…` public
path, the same way the cross-crate `Client` param already binds; verify the
interface layer declares the opaque in the consuming kernel's `.ipei` (it
must — the return type is part of the surface) and that TWO kernels naming
the same foreign type unify nominally (naming SSOT:
`src/compiler/ffi/src/naming.rs`).

### 2.4 Failure modes to guard

- **Wrong public path admitted → E0433/E0603 at cargo = SEAL break.**
  Mitigation: map entries only from the owning crate's reachable-path walk;
  add an inspector unit test with a synthetic two-crate fixture (pattern:
  the `fluent_api` fixture at ~13281) where the sibling type is (a)
  root-re-exported → binds with the public path, (b) private-module-only →
  stays dropped.
- **Std/blanket leakage through fix F1.** Test: a manifest fixture whose
  impl's trait is `std::fmt::Display` must NOT gain projected provided
  methods.
- **Send tri-gate regression:** the projected `send` future must still pass
  the output/params/receiver Send gates; `stripe_shared::Customer` Send-ness
  comes from the existing proof machinery (conditional structural Send + the
  synthetic Send derivation) — if unproven, the drop is correct and the
  ledger must SAY so (that would be a real wall to re-evaluate, not to
  bypass).

### 2.5 Verification

1. Inspector unit tests (new fixtures) green:
   `timeout 600 cargo nextest run -p ipe-ffi-inspector` (lane target dir).
2. Re-run the stripe manifest install; assert in `<slug>.kernel.json`:
   `send` present for the create-customer, create-checkout-session and
   retrieve-checkout-session receivers.
3. Stripe probe `Main.ipe` covering the skyshop used-set — client builder
   (`url_from_clientBuilder` for stripe-mock) → create customer → checkout
   session create → retrieve, chained with `Task.andThen` — `ipe build`
   exit 0 → emitted `cargo build` exit 0 (THE SEAL) → run folds to the
   no-network structured `Err` (or a stripe-mock round-trip when
   `stripe-mock` is running).
4. Used-set DCE: emitted `src/ffi.rs` contains only the reached stripe
   wrappers.
5. `cargo nextest run -p ipe_ffi` + scoped clippy stay green.

---

## 3. firebase-bind (`rs-firebase-admin-sdk = "4.3"`)

**Goal:** the skyshop auth used-set bound shim-free: verify a Firebase ID
token and read its claims.

### 3.1 Surface (from the reference shim,
`upstream:examples/rust/skyshop-rs/wrappers/sky-firebase-auth-shim/src/lib.rs`)

| Op | Foreign shape | Expected admission class |
|---|---|---|
| `App::emulated()` | sync ctor → `App` | pure/ctor shape; opaque Clone-gated handle |
| `App::live()` | `async fn … -> Result<App, E>` | async-ctor shape (owned opaque return) |
| `app.id_token_verifier()` | sync (emulated: infallible; live: `Result`) → validator value | pure/fallible shape; opaque handle |
| `TokenValidator::validate(&self, token: &str)` | async-trait method → `Result<HashMap<String, serde_json::Value>, E>` | de-async keystone + closed zero-param generic instance (`closed_instance_lines`) + owned `&str` coercion |

The crate's `tokens` feature is default-on; no `--features` needed. The
provided-method projection machinery was originally built against this SDK's
CRUD surface (`FirebaseAuthService: Send + Sync` supertrait → provably-Send
receiver), so the trait plumbing is expected to hold; the run is
verification plus at most one new wall.

### 3.2 The one expected wall: the claims return type

`validate` returns `HashMap<String, serde_json::Value>` — a CONCRETE
serde-container, not a generic serde-bounded slot, so the existing
serde-reduction (generic slots → JSON-text `String`) does not apply as-is.
Candidates, in order:

1. **Extend the JSON-text lift to concrete `serde_json::Value`-bearing
   containers in RETURN position only** (`HashMap<String, Value>`,
   `Vec<Value>`, bare `Value`): Ipê surface `Task Error String` (JSON text),
   lifted with the existing total `serde_json::to_string` path
   (`instance.rs` `ok_lift` / `bindings.rs` ret-coerce). This is the same
   boundary contract the firestore `get_obj` reduction already established —
   parse-don't-validate happens Ipê-side via `Json.Decode`.
   Param position stays DROPPED (fail-closed; no reverse lift yet needed).
2. If (1) is disproportionate: bind `validate` only if some
   already-admissible sibling surface yields the claims — do NOT invent a
   crate-specific special case. If nothing binds, that is an honest blocker
   for the milestone, not a shim.

### 3.3 Security invariant (relocates from the shim, MUST NOT be lost)

The emulator validator decodes `alg=none` tokens without signature/aud/iss/
exp checks. The shim refused the emulator path outside dev
(`ENV`/`SKY_ENV` gate). Shim-free, that gate becomes **Ipê code** in the
transposed `Lib/Auth.ipe`: read the env via `Ipe.Env`, and refuse
`FIREBASE_AUTH_EMULATOR_HOST` outside dev BEFORE constructing the emulated
app. This is a Security-principle item: the transposition's review checklist
must verify it exists and is unbypassable (the live path is the only
reachable outcome in production).

### 3.4 Verification

`ipe add rs-firebase-admin-sdk@4.3` (jailed, `--yes`, timeout-bounded; bump
`IPE_FFI_*` caps env if the closure exceeds small-crate defaults) →
kernel.json carries the four ops with `effect=effectful`/Send verdicts →
probe `Main.ipe` (emulated app → verifier → `validate` chain, `Task.onError`
fold) → SEAL → run: structured `Err` without an emulator; with
`FIREBASE_AUTH_EMULATOR_HOST` up, mint a token via the emulator signUp REST
endpoint (`Ipe.Http` from the probe itself, or curl outside) and assert the
claims JSON round-trips. Used-set DCE count recorded.

---

## 4. skyshop-transpose — `examples/13-skyshop/` (ACCEPTANCE)

**Source of the transposition:** `upstream:examples/rust/skyshop-rs/src/`
(READ-ONLY; already the FFI-shaped app: `Lib/Db.sky`, `Lib/Stripe.sky`,
`Lib/Auth.sky` are thin wrappers over the three shims), cross-checked
against `upstream:examples/13-skyshop/` (Go original) for `static/`, `e2e.json`
behavior shapes, and anything skyshop-rs diverged on. ~8.2k lines across
Main/State/9 Lib/7 Page/1 Ui modules. Our `examples/13-skyshop/` slot is
free.

### 4.1 Manifest

`sky.toml`: entry `src/Main.ipe`, `[source] root = "src"`, `[live]`
port 8000 + `static = "static"`; `[rust.dependencies]`:

```toml
firestore = "0.49"
async-stripe = "=1.0.0-rc.6"
async-stripe-types = "=1.0.0-rc.6"
async-stripe-checkout = { version = "=1.0.0-rc.6", features = ["checkout_session"] }
async-stripe-core = { version = "=1.0.0-rc.6", features = ["customer"] }
rs-firebase-admin-sdk = "4.3"
uuid = "1"
```

ZERO `file://` git shim deps. The upstream `sky-tailwind` dependency and the
`[live] store = "sqlite"` setting are port decisions resolved at transpose
time against OUR stdlib surface (tailwind: whatever `Ui/Layout` actually
needs from it — port the used classes or swap to our Ui/Css surface;
store: see decision R2). Every intentional difference from skyshop-rs lands
in the example README as a divergence note.

### 4.2 Mechanical transposition

`.sky` → `.ipe`, `Sky.*` → `Ipe.*` imports (case-preserving; precedent:
`examples/39-ffi-skyshop-core/`), `import Rust.Sky_firestore_shim` etc. →
the real auto-bound kernels (`Rust.Firestore`, `Rust.AsyncStripeCore`, … —
exact module names per the kernel naming SSOT of the `ipe add` run). Check
in `.ipe/cache/ffi/rust/` artifacts for all bound crates (the
`39-ffi-skyshop-core` precedent) so the example builds network-free; the
disk-derived `build_set` auto-registers the directory with the sweep.

### 4.3 Decision R1 — Lib API shape: keep sync `Result`, fold with `Task.run`

The direct-bound ops are `Task Error a`; skyshop-rs's Lib modules expose
sync `Result Error a` consumed at ~200 call sites across Main/Page/Lib.
**Decision: the Lib boundary keeps its sync `Result Error` signatures and
folds each op with `|> Task.run`** (surface exists:
`src/stdlib/Ipe/Task.ipe:82`; H1's process-global runtime is exactly what
makes handles sound across repeated `Task.run` entries). Rationale:
faithful-port default; the acceptance metric is shim-free + DCE, not a TEA
reshape; a full `Cmd`-effectification belongs to the `Task.run`-removal
campaign (`drop-task-run-surface-design.md`), whose codemod/patch-queue will
migrate this example along with every other consumer when it lands.

`_status` keys die: Lib functions decode the JSON-text rows/documents into
typed values at the boundary (`Json.Decode` — parse, don't validate) and
return `Result Error (Maybe …)` / `Result Error (List …)`; error legs are
typed `Error` from the funnel, never sentinel strings.

### 4.4 Decision R2 — handle strategy (probe before committing)

The app needs a `FirestoreDb` and a stripe `Client` (both Clone-gated
opaques). Preference order, decided by a 20-line probe EARLY in the
milestone:

1. **Model-held handles** (construct in init, store in `Model`): probe that
   an opaque in a live-app `Model` builds and runs under `store = "memory"`.
   If the session layer requires serde on the model even in memory mode, use
   the runtime's `disconnected_*` reconstruction pattern
   (`src/runtime/rust/src/core.rs`) — serde-skip + structured error —
   before falling back.
2. **Per-request construction inside Lib** (options → ctor → op → `Task.run`
   per call): always sound, costs a gRPC channel per op — acceptable
   fallback (Efficiency yields to Correctness), recorded honestly.

If model-held handles force `store = "memory"` where upstream used sqlite,
record the divergence in the example README.

### 4.5 Firestore emulator token source (the known judgment residue)

firestore 0.49 constructs a token source even against
`FIRESTORE_EMULATOR_HOST` (no anonymous path). The shim solved it with a
hand-written `ExternalSource` impl — not portable (trait object, unbindable)
and not wanted. Candidates, in order; probe decides:

1. **Bind the `TokenSourceType` enum surface**: if the
   `with_options_token_source`-class ctor admits, the `Json(String)` /
   `File(PathBuf→String)` variant ctors are ordinary enum-ctor bindings (the
   `ExternalSource` variant correctly drops — trait-object payload; a
   partially-bindable enum must not drop wholesale, which the enum-ctor
   emitter already supports per-variant). Ipê passes a dev-only throwaway
   service-account JSON.
2. **Config-only**: `GOOGLE_APPLICATION_CREDENTIALS` pointing at a
   throwaway dev key file; zero code.
3. **Last resort**: a GENERATED helper emitted by the generator (never a
   hand-written crate). Requires guardian sign-off; expected unnecessary.

The dev-only nature of (1)/(2) must be gated exactly like the firebase
emulator path (§3.3): production refuses emulator config.

### 4.6 Verification (the acceptance gate)

1. `ipe build` exit 0 → emitted `cargo build` exit 0 (THE SEAL) → app boots.
2. **Behavior, not boot-only**: with the firestore emulator + firebase auth
   emulator + stripe-mock running, exercise the real flows — browse
   products, auth (mint emulator token, verify), cart add/remove,
   checkout-session create + retrieve, admin CRUD — via the e2e harness
   shapes from upstream `13-skyshop/e2e.json` where portable. Every flow an
   emulator cannot cover is recorded as an honest residual in the example
   README, never faked green.
3. **Zero-shim assertion**: no `wrappers/`, no `file://` git dep, no local
   path dep anywhere under `examples/13-skyshop/`; the only Rust the app
   links beyond the emitted project + vendored runtime is the real SDK
   crates from crates.io.
4. **Used-set DCE proof**: emitted `src/ffi.rs` wrapper count per crate
   recorded (firestore ~670 → ≲10; stripe ~1.6k → the used builders +
   setters + `send`s + getters; firebase → ≲6). Also record cached
   `_bindings.rs` sizes against the demand-synthesis escalation trigger
   (>10 MB/crate would re-open extension E7; expectation ~2 MB — a
   measurement, not a risk).
5. Sweep row green (`scripts/equivalence-checks/examples-sweep.sh` — the
   disk-derived build_set picks the example up automatically; run/e2e legs
   gated on emulator availability like other env-gated examples).

---

## 5. The three filed pre-existing defects

### 5.1 canon-arity-gate — canon rejects ill-formed type applications (lowerer ICE)

**Defect:** an ill-formed sig such as `Maybe List FirestoreValue` (un-
parenthesized application: `Maybe` at arity 2, `List` at arity 0) sails
through canon and ICEs the lowerer — `ir_type_from_canon`'s `other =>` arm,
IPE-I0001 "type constructor with empty home"
(`src/compiler/lower/src/lower.rs` ~8791 documents the class). The interface
emitter now parenthesizes its own output, but the CLASS is open for any
source (hand-written or injected).

**Fix (structural, fix-the-cause):** an arity/kind check on type
applications at canon for BUILTIN constructors (`List`/`Maybe`/`Dict`/
`Result`/`Task`/`Set`/… — the closed table canon already knows), where the
home-resolution happens (`src/compiler/canon/src/resolve.rs` — the empty-home
builtin path, ~402): applying a builtin ctor to the wrong arity, or using a
saturated ctor in argument-head position, is a typed `Diagnostic` (new
IPE-C code with an explain page per the kind-teacher rule), never a value
that reaches lower. The lowerer's ICE arm stays as the backstop
(make-invalid-states-unrepresentable at the boundary; the ICE becoming
unreachable is the point).

**Verify:** canon unit test: annotation `x : Maybe List Int` → the new
diagnostic, not IPE-I0001 (precedent harness:
`src/compiler/lower/tests/unsupported.rs`); existing corpus stays green
(parenthesized `Maybe (List Int)` unaffected).

### 5.2 private-path-admission — private-trait-path UFCS wrappers over-drop at admission

**Defect (`fluent_api` class):** a UFCS wrapper can be emitted whose trait
(or self) qualifier is a DEF path threading a private module
(`::krate::private_mod::Trait`) — E0603 if ever reached. Today only
unreached-wrapper DCE keeps builds green; that is luck, not admission.

**Fix:** in the qualifier resolution consumed by `build_trait_ctx` (~9665) —
specifically `ufcs_trait_path_with_args` and `self_path_with_concrete_args`
— every emitted path must be PROVEN public: id-first through
`REACHABLE_PATHS` (the public re-export walk) or the confirmed std canonical
path; a path recoverable only from `doc["paths"]` whose intermediate
segments cannot be proven public → `TraitUnreachable` drop at admission.
(Once fix F2 of stripe-send lands, `GLOBAL_XC_PUBLIC_PATHS` is a third
legitimate source, same proof discipline.)

**Verify:** extend the `fluent_api` synthetic fixture (~13281): (a) trait in
a private module WITH a root `pub use` → binds, qualifier uses the public
path; (b) trait with NO re-export → the binding is ABSENT from the emitted
bindings text at add time (not merely unreached). Re-run the firestore +
stripe adds; diff coverage ledgers — every newly-dropped wrapper must be one
that could not have compiled if reached (spot-check by referencing one
pre-fix wrapper from a probe and confirming the E0603).

### 5.3 maybe-coercion — `IpeMaybe` ↔ `Option` in synthesized closed-instance wrappers

**Defect:** `render_generic_wrapper` (`src/compiler/ffi/src/instance.rs`
~536) declares wrapper params via `call.render_arg_type_at` RAW (~640–651),
so a `Maybe`-slot param renders as `Option<…>` while the backend forwarder
passes `IpeMaybe<…>` → E0308 the moment such a wrapper is reached. The flat
tier (`bindings.rs`) already owns this coercion (~443–446 return lift, ~689
param adapt). firestore's `update_obj…` (`Maybe (List String)` args — the
skyshop `setDoc` path) hits this, which is why this fix BLOCKS
skyshop-transpose.

**Fix:** apply the owned-coercion layer at BOTH boundary positions of the
generic/closed-instance renderer, mirroring the flat tier:
- **param:** declare `IpeMaybe<T>` (nested containers included:
  `IpeMaybe<Vec<String>>`), and adapt to the host's `Option<T>` in a prelude
  line before `body_call` (`match argN { IpeMaybe::Just(v) => Some(v),
  IpeMaybe::Nothing => None }` — through the same helper text the flat tier
  emits, single SSOT for the conversion snippet);
- **return:** in the `ok_lift` chain (~572–594), an `Option<T>` OK lifts to
  `IpeMaybe<T>` (and the `ret_inner` type renders accordingly).
The Ipê-facing signature in `.ipei` already says `Maybe …` — this closes the
wrapper text to match its declared surface (SEAL discipline: the mismatch is
a representable-but-illegal pipeline state; after the fix the renderer
derives both sides from one type mapping).

**Verify:** emission unit tests in `ipe_ffi` (closed instance with
`Maybe String` and `Maybe (List String)` params + `Maybe` return: assert
`IpeMaybe` in the sig, the adapt prelude, the return lift); re-run the
firestore add; extend the firestore probe to CALL `update_obj…` with
`Just`/`Nothing` masks → SEAL → run (structured `Err` offline; emulator
round-trip when up).

---

## 6. Closure

- `cargo fmt --all`; cheap gates per lane throughout; ONE full gate for the
  batch (`cargo nextest run --workspace`; `-p ipe-runtime-rust --features
  full`; doc tests; workspace clippy `-D warnings`; examples sweep).
- Ledgers: any new sanctioned divergence → `docs/divergences-from-sky.md`;
  the prior impl-plan's progress table updated to terminal states; the
  backlog item closes only on the acceptance metric.
- Report: per-crate binding/DCE counts, emulator-gated residuals, and the
  stripe-send diagnosis outcome (which decision-tree branch was real — it
  feeds the inspector's regression corpus).

## 7. Gate summary (every milestone)

| Gate | Command shape (all `timeout`-wrapped, lane `CARGO_TARGET_DIR`) |
|---|---|
| FFI crates | `cargo nextest run -p ipe_ffi -p ipe_sandbox` |
| Inspector | `cargo nextest run -p ipe-ffi-inspector` |
| Runtime (maybe-coercion / transpose touch points) | `cargo nextest run -p ipe-runtime-rust` (+ `--features full` when `live`/task surface touched) |
| Canon/lower (canon-arity-gate) | scoped `nextest` on the touched crates |
| SEAL, per probe + skyshop | fresh `cargo build -p ipe` → `ipe build` exit 0 → emitted `cargo build` exit 0 → run |
| Clippy | scoped `-D warnings` per lane; workspace at the full gate |
