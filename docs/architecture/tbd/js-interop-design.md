# JS interop: one typed boundary, two transports, one named hatch

Status: design proposal. The reserved type, its seal, and the fail-closed
emission gate are **shipped**; the transports, codec, glue, capability axis, and
package wiring are **not**. Every fenced block below is an illustrative sketch of
the intended surface — exact spellings are fixed by the golden tests of each
increment, not by this doc. Bare prose only: the issue reference lives in the
commit subject, never here.

A JS boundary is an injection and trust surface. **Every increment that touches
it MUST pass a security-soundness-guardian review before its implementation
merges.** Security > Correctness governs every decision below.

## 1. The one discipline

Ipê talks to JavaScript through exactly one discipline: every value crossing the
Ipê↔JS seam carries a **concrete-ADT seal** — a closed, declared *down-type*
(Ipê → JS) and *up-type* (JS → Ipê), decoded on the way in and encoded on the
way out, with no untyped hole. What varies between mechanisms is the
**transport**, never the type discipline. Two typed concepts, plus one named
escape hatch that is deliberately *not* a transport:

1. **Declarative typed transport — custom elements.** A JS widget behind the
   reserved `CustomElement down up` type: sealed state down as a
   property/attribute, typed events up as a `CustomEvent` (wasm client) or a
   posted event (server-driven). Discrete, encapsulated, reusable; the shim-free
   auto-binding analogue of native FFI.
2. **Raw typed transport — ports** (`Js.send` / `Js.subscribe` / `sync`, no new
   syntax — Cmd/Sub + a cfg field). Streaming, programmatic; imperative effects
   and out-of-band data, not view nodes. Same seal rules; ships after custom
   elements. Full surface, trust/ordering flags, and per-target lowering in §6.
3. **Escape hatch (not a transport).** `Ui.html` + `Html.unsafeRaw` (the sole
   un-escaped render surface, already shipped) plus a served static script, with
   a typed `onSubmit` record decode for progressive enhancement. The sanctioned
   JS-eval seam lands as `Ipe.Js.Unsafe.unsafeEval` behind the `unsafe`
   capability — see `unsafe-escape-convention-design.md`, which owns that hatch;
   this doc does not restate it. No unsanitised eval-style primitive ever
   appears on a plain surface: Security > Completeness.

This mirrors the native Rust FFI split (`ffi-to-rust-design.md`): two typed tiers
plus one named escape. Prior-art custom-element / port disciplines exist in other
functional-web languages; §5 records where Ipê re-derives idiomatically and where
it deliberately diverges (typed, not stringly; fail-closed Ipê-side, not a JS
exception).

## 2. Where the compiler already stands (verified EXISTS)

| Piece | State | Where |
| --- | --- | --- |
| `CustomElement` reserved name | EXISTS — a user `type CustomElement …` is rejected like any reserved builtin | `RESERVED_BUILTIN_TYPES` + `is_reserved_builtin_type_name`, `src/compiler/canon/src/resolve.rs`; IPE-N0026; test `canon_custom_element_definition_reserved` (`src/ipe-cli/tests/negative_suite.rs`) |
| Typed acceptance at canon | EXISTS — `CustomElement down up` **type-resolves**: arity gate (IPE-N0031, exactly two params) then the plain-value seal gate (IPE-N0039) both pass for a legal seal; canon assigns the empty-home builtin | `canonicalise_type` + `boundary_seal_rejection` (`resolve.rs`), `SealRejection` enum (`src/compiler/diagnostics/src/diagnostic.rs`); tests `canon_custom_element_{use_resolves,plain_user_seal_resolves,arity_too_few,arity_too_many,seal_rejects_function}` |
| The seal predicate | EXISTS — `SEAL_PLAIN_PRIMITIVES` (`Int Float Bool String Char Bytes`), `SEAL_UNARY_CONTAINERS` (`List Set Maybe`), `SEAL_BINARY_CONTAINERS` (`Dict Result`); effect carriers / view types / functions rejected | `resolve.rs`; IPE-N0039 explain page |
| Fail-closed at **emission**, not canon | EXISTS — a `CustomElement` value that type-resolves is rejected fail-closed at lowering with IPE-L0133 (`Feature::CustomElementTransport`) until the transport ships; this proves canon accepted the annotation and no untyped value reaches codegen | `lower.rs` (`"CustomElement" => Err(unsupported(…, Feature::CustomElementTransport))`), `CustomElementTransport` variant + render (`diagnostic.rs`, `render.rs`), explain page IPE-L0133.md |
| Literal-constructor precedent | EXISTS — `Ffi.kernel "…"` must be a call on a single string literal, read at compile time; the exact shape discipline the `customElement` constructor reuses | `resolve.rs` (kernel-alias literal path); tests `unknown_kernel_alias_is_rejected_at_compile_time`, `registered_kernel_alias_resolves_and_builds` |
| Path-literal seal precedent | EXISTS — a build-time path literal is cleaned + in-project-checked | `path_literal_gates.rs`, `src/runtime/rust/src/path_core.rs` |
| Reserved-sink family | EXISTS — `Secret`, `SqlFragment`, `Url`, `Regex`, `PubSub` topic: reserved, un-shadowable, parse-only constructor | `resolve.rs`; `unsafe-escape-convention-design.md` §1 enumerates them |
| Server-driven wire (both directions) | EXISTS — browser POSTs a typed event body to `/_ipe/event`; SSE patch envelope down, attribute deltas first-class | `EventBody` / `PatchEnvelope` (`src/runtime/rust/src/web/mod.rs`), `Patch.attrs` (`dom/diff.rs`) |
| Content-addressed + SRI script serving | EXISTS — `/_ipe/client.<hex16>.js` with `integrity="sha256-…"`; the pattern generated glue reuses | `render_page_full` / `client_js_hashes` (`web/mod.rs`) |
| Typed hydration island (the flags analogue) | EXISTS — `HydrationState` serialised into an `island_escape`d `<script type="application/ipe-model+json">`, parsed with `serde_json`, never evaluated; must be a plain value | `render_page_hydrate` (`web/mod.rs`), project emit (`src/compiler/backend/rust/src/project.rs`) |
| Escape hatch (shipped half) | EXISTS — `Ui.html`; `Html.unsafeRaw` → `HRaw` is the ONLY un-escaped render surface; every other text path is entity-escaped | `lower.rs`, `src/runtime/rust/src/html.rs` |
| Capability spine | EXISTS — closed `Capability` vocabulary; `ipe capabilities` report; manifest `[capabilities]` must EQUAL the inferred set; index-admission CI re-runs the gate | `Capability` enum (`src/compiler/kernels/src/capability.rs`), `ipe capabilities` (`src/ipe-cli/src/help.rs`), audit EQUALS-check (`src/ipe-cli/src/audit.rs`), ADR 0044 |
| Header / CSP safe defaults | EXISTS — safe-by-default response headers; `frame-ancestors` response-splitting-sanitised; inline bootstrap still needs `'unsafe-inline'` (nonce tightening deferred) | `src/runtime/rust/src/telemetry.rs`, `web/mod.rs` |

So the reserved name, typed acceptance, the seal predicate, the emission gate,
both wire directions, the SRI serving pattern, and the capability/audit spine all
**exist today**. What this design adds is the constructor, the seal *codec*, the
`Ui.widget` view node + emission, the generated glue + serving, and the
capability/package wiring.

## 3. The boundary model

### 3.1 The seal (EXISTS as a predicate; codec PROPOSED-NEW)

A boundary type (down or up) must be a **plain, closed, declared value type**:
primitives, records, lists, tuples, `Maybe`, `Result`, `Set`, `Dict`, and
user ADTs over those — transitively. The predicate is already enforced at canon
(`boundary_seal_rejection`, §2). Excluded and rejected at the type level:
functions, `Cmd` / `Task` / `Sub`, view / `Element` values, open rows / type
variables (the seal is monomorphic — prefer-concrete codegen, one lowering per
concrete type, no reflection, no `dyn`), and — a **PROPOSED-NEW seal tightening**
— `Secret` and every reserved sink type (`SqlFragment`, CSS-safety markers): a
secret or sink-privileged value must never be serialisable across the seam.
Confirm whether the shipped seal predicate already excludes `Secret`/sink types;
if not, that exclusion is the first change of increment WP2.

**Encode/decode substrate.** The seal codec is JSON, sharing the canonical-JSON
conventions already used by the two shipped crossings (`EventBody` args and the
hydration island). The native FFI boundary is consolidating its own doubly-owned
wire onto a single-owner `ipe_ffi_wire` crate (`ffi-wire-schema-design.md`),
which explicitly names this design as inheriting the same "one typed contract, no
drifting twins" discipline. **Recommendation (resolve ambiguity):** the JS seal
does **not** reuse `ipe_ffi_wire` verbatim — that crate is the *native
inspection document* contract (crate metadata, foreign type decls), a different
shape from a per-widget seal. What the JS seal inherits is the *pattern*: one
codec module, generated per concrete seal type, emitting both the Rust decoder
(in the emitted program) and the JS decoder (in the generated glue) from the
**same** type definition, so the two sides cannot drift independently of the
compiler. Ports and custom elements share this one codec module (§4, §6).

**Decoding is total and fail-closed.** A value arriving from JS that does not
decode to the declared type is dropped at the boundary with an observable
diagnostic (dev console / server log); no partial value is constructed, and there
is no `Value`-style typed hole letting an undecoded value travel inward. This is
the same posture the shipped IPE-L0133 gate enforces at emission and the FFI
decode boundary enforces at the wire.

### 3.2 The three surfaces

1. **`CustomElement down up`** (§4) — declarative; state down as a decoded
   property/attribute, events up as an encoded typed event.
2. **Ports** (later tier, §6) — `Js.send : a -> Cmd msg` /
   `Js.subscribe : Decoder a -> (a -> msg) -> Sub msg` / a `sync` cfg field, in the
   wasm client, same seal, same codec, a generated typed shim on the JS side.
3. **`Ui.html` + `Html.unsafeRaw` + served static script** (+ `Ipe.Js.Unsafe.
   unsafeEval`) — the named unsafe tier. Nothing new ships here; its role in
   this design is only to be *documented as the non-typed tier* so no ad-hoc
   hatch is ever justified by its absence. Owned by
   `unsafe-escape-convention-design.md`.

## 4. The `CustomElement` reserved type

### 4.1 Surface (constructor + view node PROPOSED-NEW)

```ipe
codeEditor : CustomElement EditorState EditorEvent
codeEditor = customElement "js/code-editor.js"

view model =
    Ui.widget codeEditor (stateOf model) EditorChanged
```

- `CustomElement down up` — exactly two seal-legal params. Wrong arity
  (IPE-N0031), a non-concrete param, or a seal-illegal param (IPE-N0039) is
  already rejected today. **EXISTS.**
- `customElement` (**PROPOSED-NEW**) — a reserved constructor, legal only as the
  entire body of a `CustomElement`-annotated binding, applied to a **single
  string literal**, the same shape discipline as `Ffi.kernel "…"` (§2). The
  literal names the author's widget-hook JS file, resolved at build time: cleaned
  and required inside the project root (reuse `path_core.rs`, no `..` escape),
  required to exist, and content-hashed. A non-literal argument, a bare
  `customElement` value, or a missing file is a compile error. **Ambiguity flag
  (for the user):** bare-string-literal vs typed `Path`. Recommendation — ship
  the bare string literal now (matches `Ffi.kernel`, no dependency); when typed
  `Path` values land, the constructor takes a `Path`, keeping the literal
  requirement. Note the two type params are the seal *only*; the JS source is a
  *value* argument, never a type param.
- `Ui.widget : CustomElement down up -> down -> (up -> msg) -> Element msg`
  (**PROPOSED-NEW**) — the one view node that places a widget. The
  `CustomElement` value is opaque: not comparable, not serialisable, not storable
  in the `Model` (it fails the existing plain-Model gate exactly like a function
  value).

### 4.2 Typing (flip the emission gate, keep canon acceptance)

Canon **already** accepts a legal `CustomElement` annotation (§2). The change is
at **emission**: the IPE-L0133 fail-closed arm in `lower.rs`
(`Feature::CustomElementTransport`) flips from `Err(unsupported(…))` to real
lowering once the transport lands. The reservation (IPE-N0026) and the seal gates
(N0031 / N0039) stay. The constrain layer gains the `customElement` and
`Ui.widget` kernel signatures. Confirm/extend the seal predicate with the
`Secret`/reserved-sink exclusion (§3.1).

### 4.3 Codegen (PROPOSED-NEW)

**Element identity.** Each binding registers one custom element with a generated,
content-addressed tag `ipe-ce-<hash-of(binding, seal, js-hash)>`. The tag never
contains user input; `customElements.define` is called only on generated names —
element-registration injection is impossible by construction.

**Generated glue (JS).** The compiler emits the mechanical shell per binding
(sketch):

```js
class extends HTMLElement {
  static observedAttributes = ["state"];
  attributeChangedCallback(_n, _o, v) { /* decode_down(v) -> hook.onState */ }
  set state(v)                        { /* decoded property path (wasm client) */ }
  connectedCallback()                 { /* hook = mount(this, emit) */ }
}
```

`decode_down` is the generated total decoder for `down` (unknown field / wrong
shape / wrong tag → drop + console diagnostic; the hook never sees it); `emit` is
the generated encoder for `up` that wraps the encoded value in a `CustomEvent`
(wasm) or posts it through `/_ipe/event` (server-driven). The author hand-writes
only the irreducible hook, in the file the literal names (sketch):

```js
export function mount(host, emit) {
  return { onState(state) { /* … */ } };
}
```

**Server-driven (Web shape) transport.** The view renders
`<ipe-ce-… state="{escaped json}">`; the state attribute goes through the
standard attribute entity-escaper (never `HRaw`). State changes ride the existing
attribute-delta patches (`Patch.attrs`); `attributeChangedCallback` decodes and
forwards. **Ambiguity resolved:** up-events post through the existing
`/_ipe/event` wire with the encoded `up` value as the event payload; the server
resolves the handler by ipe-id exactly as for clicks (`EventBody`), then the
generated **Rust** decoder for `up` parses the payload — fail-closed drop on
mismatch — and dispatches the typed `msg` to `update`. The up-type
`CustomEvent.detail` is decoded by the *same* generated decoder on the JS side
before it becomes the posted payload, so one decoder governs both hops.

**Wasm client transport.** Same glue file, property path instead of attribute:
the `hydrate` / `spa` renderer assigns the decoded value via the `state` setter
and listens for the typed `CustomEvent`. One glue, two adapters; the codec is
byte-identical in both modes.

**Serving.** Glue + author hook bundled per widget, served content-addressed with
SRI exactly like `/_ipe/client.<hex16>.js`. The build-time hash is what the
page's `integrity` pins — a swapped file fails to execute.

No dynamic reflection anywhere: every decoder/encoder is concrete generated code
for the concrete seal type, on both sides of the seam.

## 5. Prior art: re-derived, and where Ipê diverges

| Aspect | Prior-art functional-web norm | Ipê |
| --- | --- | --- |
| Two port directions (out / in), message-passing not function-calling | yes | followed |
| Custom elements as the declarative widget boundary | yes, but **stringly** (attributes hand-stringified, event decoders hand-written) | **typed**: the codec is generated from `CustomElement down up` |
| Flags typed at init | yes; mismatch errors **on the JS side** | followed via the `HydrationState` island, but mismatch is a **fail-closed Ipê-side decode** (clean-init fallback), never a JS exception |
| An untyped `Value` as an allowed boundary type | yes (the recommended escape) | **diverged: rejected.** No undecoded value crosses inward; the failure branch is typed at the boundary |
| Ports forbidden in packages | yes | **diverged: allowed but disclosed** — a JS-touching package carries a mandatory capability (§7) and its scripts are content-hash-pinned |
| Unused-port dead-code elimination | yes | followed: an unused binding and its glue are DCE'd with the rest of the program |

## 6. Ports (raw typed transport — later tier)

Ports carry imperative effects and out-of-band data that have no place in the
view tree (custom elements own visual-in-view; §4). Same seal (§3.1), same
generated codec module — **one wire codec for both typed transports** (resolves
the shared-codec ambiguity: yes). No new language construct: ports reuse the
`Cmd`/`Sub` machinery Ipê already has. Ships after the custom-element tier is green.

### 6.1 Surface

- **Outbound command** — `Js.send : a -> Cmd msg`, a one-shot imperative effect
  (play a sound, fire analytics). The per-ADT wire encoder is derived; the
  compiler generates per-variant senders (`playChime = Js.send PlayChime`) so the
  closed-type discipline keeps per-function ergonomics.
- **Inbound intent** — `Js.subscribe : Decoder a -> (a -> msg) -> Sub msg`, wired
  in `subscriptions`. The decoder **is** the security gate — parse-don't-validate
  at the trust boundary; the core never sees a raw blob.
- **State mirror** — `sync : Model -> JsState`, a cfg field alongside `view`: a
  pure projection with its own encoder, diffed and streamed on the same channel as
  the DOM patches, re-emitted only on change. A second `view`, for JS-mirrored data.

### 6.2 The closed-ADT seal, pointed at the boundary

Inbound/outbound types MUST be concrete declared ADTs, never `Json.Value` or an
opaque passthrough — `Decoder Value` is rejected, so the untyped channel *cannot
be spelled* (make-invalid-states-unrepresentable at the boundary type itself). One
closed `JsMsg`/`JsCmd` per direction is the whole attack surface as a single
auditable object, and the inbound `case` is exhaustive — a published-but-unhandled
variant is unrepresentable. A genuinely opaque payload is expressed by *naming* it
(`type RawJson = RawJson String`), never left an untyped hole.

- **`JsMsg` is a narrow published type, never the internal `Msg`.** The browser is
  attacker-controlled; publishing `Msg` would hand an attacker every transition the
  state machine can reach. `JsMsg` is a small separate type the runtime maps into
  `Msg`.
- **`sync` ships a projection, never the raw `Model`** — the same discipline `view`
  has. Secrets cannot leak because they are never in `JsState` (server-side, a
  secret boundary; client-side, an encapsulation boundary against other page scripts).

### 6.3 Two orthogonal flags (never conflated)

- **Trust → decode gate.** Untrusted far side (browser JS, sandboxed Rust) → inbound
  decode gate ON. Trusted (pure in-process Rust) → direct typed binding, no gate.
  Keyed on trust, **never on transport**: in-process ≠ trusted (browser JS is
  attacker-controlled whether over a network or in the same page).
- **Ordering → staleness layer.** Network/async-unordered → optimistic-concurrency
  handling. In-process-ordered → none. Keyed on transport latency, never on trust.

### 6.4 Per-target lowering

| | Server-driven Web | Client-WASM |
| --- | --- | --- |
| TEA loop runs | server | browser (WASM) |
| Transport | network stream | in-process (wasm-bindgen) |
| Inbound gate (`Js.subscribe`) | ON | **ON** — in-process ≠ trusted |
| `Js.send` delivery | encode → stream → `ipe.onReceive` | direct dispatch, same tick |
| `sync` mirror | encode → diff → stream deltas | direct in-memory handoff |
| Staleness handling | present | absent (single-threaded, ordered) |
| Developer surface + JS runtime API (`ipe.send`/`onReceive`/`onSync`) | identical | identical |

One port declaration; `--target` picks the transport; the two flags decide what
the lowering emits. Two runtime implementations of the `ipe` object (one posts over
the network, one calls into the WASM instance); handlers and port declarations are
byte-identical across both — ADR-0042's "one backend, inherit don't fork".

### 6.5 Staleness is typed application logic, not framework machinery

Only the server-driven transport has the distributed hazard (JS acts on a `JsState`
replica, then sends an intent the server folds against a moved-on Model). Default:
fold-against-current (an intent is an event). Where blindly folding a stale intent
would be *wrong*, the precondition is named as an explicit typed field in the intent
(compare-and-swap) — parse-don't-validate on the intent itself, stronger than an
opaque framework version token. No framework-forced whole-state lock is built
speculatively (YAGNI); most apparent staleness dissolves once intents reference
stable identities rather than snapshot positions.

### 6.6 Stable browser APIs are first-party kernels, not packages

Granting a browser capability (Geolocation, storage/IndexedDB, WebAudio) *widens
the client host surface* — a compiler+runtime decision, not something a downloaded
package may do (client-side FFI is denied; the client's only host surface is the
fixed `web-sys` allowlist, ADR-0042). So, driven by Security #1: primitive host
access → first-party target-gated `Ipe.Browser.*` kernels (as `Http`/timers/`Random`
already live); ergonomic wrappers → pure `.ipe` packages composing over them. A
capability not yet kernel-backed is reached through the typed port (your own audited
JS handler) until it graduates.

## 7. Package sharing

JS-interfacing packages are **allowed** (a deliberate divergence, §5) under three
bounds:

1. **Capability disclosure (PROPOSED-NEW enum variant).** A new axis in the
   closed `Capability` vocabulary (`src/compiler/kernels/src/capability.rs` — the
   enum today holds `Network Filesystem Database Env Subprocess Clock Random
   NativeFfi FfiRaw Unsafe`; **no JS axis exists yet**): add `CustomElement`
   (wire name `custom-element`), inferred for any module whose reachable code
   contains a `CustomElement` binding (later a `JsPort` axis for ports;
   `Html.unsafeRaw`-reaching code already discloses via `Unsafe`). The existing
   manifest EQUALS-check then forces every package to declare it, `ipe
   capabilities` surfaces it transitively, and index-admission CI re-verifies it
   — one new enum variant plus a capability-inference tagging point, no new
   machinery. **Depends on the capability-inference pass reaching client-JS
   code** (see the DAG, §9).
2. **Content pinning (PROPOSED-NEW).** The package index entry records the
   content hash of every shipped widget JS file; `ipe package publish` computes
   it, admission CI re-verifies it, the consuming build refuses a mismatch, and
   the same hash is what SRI pins in the served page — one hash from index to
   browser.
3. **Honest limit (MUST be stated plainly — no overclaim).** The native sandbox
   and native-code audit protect the **server build and run**. They do **not**
   make third-party **browser** JS safe: a widget script runs with full DOM
   authority in the page. The shipped guarantee is exactly: (a) the boundary is
   typed and fail-closed, (b) the capability is disclosed and auditable, (c) the
   served JS is SRI-pinned and CSP-constrained. Beyond that it is **declared
   trust** in the package author, and every user-facing doc must say so. The
   sandbox is never marketed as covering client JS.

## 8. Security

Precedence Security > Correctness. Trust edges, each with its single fail-closed
enforcement sink:

| # | Edge | Threat | Enforcement (fail-closed) |
| --- | --- | --- | --- |
| 1 | down-state → page | XSS via state serialisation | state crosses only as an entity-escaped attribute or decoded property — never spliced into a script / `HRaw`; the `island_escape` precedent governs any future inline carrier |
| 2 | up-event → server | forged / malformed browser input | existing session + CSRF gates unchanged; then the generated total decoder — mismatch drops the event, no partial value; body size already capped (`IPE_WEB_MAX_BODY_BYTES`) |
| 3 | widget JS itself | arbitrary code with DOM authority | SRI content-addressing (tamper fails to load), build-time hash from the index (§7.2), CSP `script-src 'self'`; NOT sandboxed — declared trust, documented (§7.3) |
| 4 | element registration | user string into `customElements.define` | impossible by construction: tag names are compiler-generated |
| 5 | `customElement "path"` literal | path traversal at build | path-seal clean + in-project check at compile time (reuse `path_core.rs`) |
| 6 | secrets in the seal | `Secret` / sink types exfiltrated to JS | rejected at the type level (§3.1 tightening) |
| 7 | glue serving | header / response splitting | reuse the shipped header sanitisation; glue routes are static, no user input in the path |
| 8 | `Ipe.Js.Unsafe.unsafeEval` | arbitrary eval | behind the `unsafe` capability, disclosed; owned by `unsafe-escape-convention-design.md`; never on a plain surface |

Standing rules: absent proof a crossing value is safe, the conservative branch
wins; a decode failure is observable but never constructs a value; the escape
hatch stays the *only* un-escaped surface and gains no new power.

**Mandatory gate.** The custom-element surface, the port surface, and the
`unsafeEval` hatch are language boundaries. A **security-soundness-guardian**
review of every implementation increment that touches this boundary is required
before merge — not optional, not batched away.

## 9. Work-package DAG (sequenced by dependency)

Foundations this design ties to: the seal codec pattern
(`ffi-wire-schema-design.md`), the `Ipe.Js.Unsafe` hatch
(`unsafe-escape-convention-design.md`), typed encode/decode combinators
(`codec-and-store-design.md`), and — for the playground that would exercise a
widget end-to-end — `Ipe.Process` (`playground-design.md`; the `Ipe.Process`
stdlib module is **partially landed** at `src/stdlib/Ipe/Process.ipe`).

```
WP0  Reserved name + typed acceptance + emission gate          [LANDED]
       │  (IPE-N0026 / N0031 / N0039 / L0133; §2)
       ▼
WP1  Seal-predicate tightening: exclude Secret + sink types    [START NOW]
       │  (leaf edit to boundary_seal_rejection; §3.1)
       ▼
WP2  Constructor + view node signatures                        [START NOW]
       │  customElement "lit" (Ffi.kernel-shaped) + Ui.widget kernel sig;
       │  path-literal seal reuse; §4.1/4.2. No transport yet — still L0133.
       ▼
WP3  Seal codec (Rust side)                                    [START NOW after WP1]
       │  generated per-type total encode/decode; adversarial malformed tests;
       │  shares canonical-JSON conventions; no JS yet; §3.1
       ▼
WP4  Server-driven emission (flip L0133 → real lowering)       [needs WP2+WP3]
       │  <ipe-ce-…> escaped-attr render; attribute-delta patch; /_ipe/event
       │  round-trip with Rust up-decoder; §4.3
       ▼
WP5  Glue generation + SRI serving + JS decoder               [needs WP4]
       │  content-addressed tag, bundled hook, playwright E2E; §4.3
       ▼
WP6  Capability axis + package pinning                         [BLOCKED on
       │  custom-element Capability variant + inference tagging;      cap-inference
       │  index SCHEMA hash field; audit EQUALS wiring; §7          reaching client JS]
       ▼
WP7  Later tier: wasm property adapter, then ports             [needs WP5; ports
          same codec (§6); each its own guardian review + addendum]  reuse WP3 codec]
```

**Start-now (no unlanded foundation):** WP1, WP2, WP3 — all leaf edits on shipped
machinery (the seal predicate, the constructor-literal precedent, the
canonical-JSON codec). WP4/WP5 are next once WP2+WP3 land.

**Blocked / gated:** WP6 is gated on the capability-inference pass being taught to
reach client-JS-bearing code and on the index SCHEMA gaining a hash field; the
`custom-element` `Capability` variant does not exist yet. WP7 (ports, wasm
adapter) waits on WP5. The playground end-to-end demo additionally leans on
`Ipe.Process` completing (partially landed).

**Independence from `ffi-wire-schema`:** WP1–WP5 do **not** block on the
`ipe_ffi_wire` extraction — the JS seal codec is its own module (§3.1). They
share a *pattern*, not a crate, so they may proceed in parallel.

## 10. Scope and non-goals

**Ships first (minimal typed boundary):** the seal tightening, constructor +
`Ui.widget`, the Rust seal codec, server-driven `Ui.widget` emission, generated
glue served with SRI, the `custom-element` capability + package hash pinning.

**Ships later:** ports; the wasm property adapter; per-widget CSP / nonce
tightening; finer capability axes.

**Non-goals:** an untyped `Value` crossing; synchronous Ipê→JS calls
(request/response belongs to native FFI, not the browser seam); executing JS
server-side; sandboxing third-party browser JS; any second escape hatch.

## 11. Open ambiguities for the user

1. **Constructor source type** — bare string literal (recommended now, matches
   `Ffi.kernel`) vs typed `Path` (adopt when `Path` values land). §4.1.
2. **Seal `Secret`/sink exclusion** — confirm whether the shipped seal predicate
   already excludes them; if not, WP1 adds it. §3.1.
3. **Codec sharing scope** — recommended: ports and custom elements share one
   generated codec module (§6); they do **not** reuse the native `ipe_ffi_wire`
   crate verbatim, only its single-owner pattern (§3.1). Confirm.
4. **Capability granularity** — one `custom-element` axis now, a separate
   `js-port` axis later, or a single coarse `js-interop` axis. §7.1.
