# JS interop: one typed boundary discipline

Ipê talks to JavaScript through exactly one discipline: every value that
crosses the Ipê↔JS seam carries a **concrete-ADT seal** — a closed, declared
*down-type* (Ipê → JS) and *up-type* (JS → Ipê), decoded on the way in and
encoded on the way out, with no untyped hole. What varies between interop
mechanisms is only the **transport**, never the type discipline:

- **Declarative transport — custom elements.** A JS widget behind the reserved
  `CustomElement down up` type: sealed state down as a property/attribute,
  typed events up. Discrete, encapsulated, reusable; works in the server-driven
  Web shape and in the wasm client.
- **Raw transport — ports** (`send` / `receive`). Streaming, programmatic;
  primarily for the wasm client. Ships after custom elements.
- **Escape hatch (not a transport).** `Ui.html` with `Html.unsafeRaw` plus a
  served static script — the already-named "use at your own risk" surface.
  Stringly by nature; a home-made widget graduates to a `CustomElement` when
  it stabilises. No new primitive is added here.

This mirrors the Rust FFI split: two typed tiers plus one `unsafe`-style named
escape. Elm's ports / flags / custom-elements discipline is the reference
prior art; §2.3 records where Ipê follows it and where it deliberately
diverges.

A JS boundary is a language boundary and therefore an injection and trust
surface. **Every increment of this design that touches the boundary MUST pass
a security-soundness-guardian review before its implementation merges.**
Security > Correctness governs every decision below.

All Ipê/Rust/JS snippets in this document are illustrative sketches of the
intended shape, not verified compiler output or runnable commands; exact
spellings are fixed by the golden tests of each increment.

## 1. Where the compiler already stands

| Piece | State | Where |
| --- | --- | --- |
| `CustomElement` reserved name | shipped — user `type CustomElement …` rejected like any reserved builtin (IPE-N0026) | `src/compiler/canon/src/resolve.rs:160` (`RESERVED_BUILTIN_TYPES`), `:266` (`EXTRA_BUILTIN_TYPE_NAMES`), `:316` (`is_reserved_builtin_type_name`) |
| Fail-closed use rejection | shipped — annotating `CustomElement d u` (bare or qualified) is rejected with IPE-N0037 until the transport ships; deliberately no untyped fallback | `src/compiler/canon/src/resolve.rs:4166–4180` (`canonicalise_type`), tests `src/ipe-cli/tests/negative_suite.rs:463,476,492`, explain page `src/compiler/diagnostics/explain/IPE-N0037.md` |
| Literal-constructor precedent | shipped — `Ffi.kernel "…"` must be a call on a single string literal, resolved at compile time | `src/compiler/canon/src/resolve.rs:4444` |
| Sink-side reserved-type gates (pattern) | shipped — `SqlFragment` builder sink (`src/runtime/rust/src/db.rs:2036`); CSS marker sinks re-validate fail-closed at the render sink | `src/runtime/rust/src/web/style_inject.rs` (module header) |
| Server-driven wire (events up) | shipped — browser POSTs `{sessionId, seq, msg, args, handlerId}` to `/_ipe/event`; server resolves handlers by ipe-id + event; `args` are parsed JSON values (form submit already carries a typed record `{name: value}`) | `src/runtime/rust/src/web/mod.rs:398–427` (`EventBody`) |
| Server-driven wire (patches down) | shipped — SSE `{globalSeq, patches}`; attribute deltas are first-class patches | `src/runtime/rust/src/web/mod.rs:391` (`PatchEnvelope`), `src/runtime/rust/src/dom/diff.rs:8` (`Patch.attrs`) |
| Content-addressed + SRI script serving | shipped — `/_ipe/client.<hex16>.js` with `integrity="sha256-…"`; the pattern any generated glue reuses | `src/runtime/rust/src/web/mod.rs:279–343` (`render_page_full`, `client_js_hashes`) |
| Typed hydration island (the flags analogue) | shipped — `[wasm] mode = "spa" \| "hydrate"`; `HydrationState` serialised into a `<script type="application/ipe-model+json">` island, `island_escape`d (script-injection foreclosed), parsed with `serde_json`, never evaluated; `HydrationState` must be a plain value | `src/runtime/rust/src/web/mod.rs:190–224` (`render_page_hydrate`), `src/ipe-cli/src/project.rs:87–118`, `src/compiler/backend/rust/src/project.rs:351–408,1916` |
| Escape hatch | shipped — `Ui.html : Html msg -> Element msg` (`src/compiler/lower/src/lower.rs:14777`, kernel wiring `:16325`); `Html.unsafeRaw` → `HRaw` is the ONLY un-escaped render surface; every other text path is entity-escaped | `src/runtime/rust/src/html.rs:18,222,1468–1484` |
| Capability machinery | shipped — closed `Capability` vocabulary; `ipe capabilities` report; manifest `[capabilities]` must EQUAL the inferred set; package-index admission CI re-runs the gate | `src/compiler/kernels/src/capability.rs:21,56`, `src/ipe-cli/src/help.rs:311`, `src/ipe-cli/src/audit.rs:494–534`, the hosted index repository `arthurmaciel/ipe-index`, `docs/adr/0044-package-coordination-manifest-index-gate.md` |
| Security headers / CSP | shipped — safe-by-default response headers; `frame-ancestors` env value response-splitting-sanitised; inline bootstrap still needs `'unsafe-inline'` (nonce tightening deferred) | `src/runtime/rust/src/telemetry.rs:277`, `src/runtime/rust/src/web/mod.rs:285–290` |

So: the reserved name, the fail-closed gate, both wire directions, the typed
island, SRI serving, and the capability/audit spine all exist. What this
design adds is the *typing acceptance*, the *seal codec*, the *view node +
emission*, the *generated glue*, and the *capability/package wiring*.

## 2. The boundary model

### 2.1 The seal

A boundary type (a down-type or up-type) must be a **plain, closed, declared
value type**: primitives (`Bool`, `Int`, `Float`, `String`), records, lists,
tuples, `Maybe`, and user-declared ADTs over those — transitively. Excluded,
rejected at the type level:

- functions, `Cmd`, `Task`, view/`Element` values (the existing
  `HydrationState` plain-value gate, `src/compiler/backend/rust/src/project.rs:1916`,
  is the reusable check);
- `Secret` and every reserved sink type (`SqlFragment`, CSS-safety types): a
  secret or a sink-privileged value must never be serialisable across the JS
  seam;
- open rows / type variables: the seal is monomorphic and concrete
  (prefer-concrete codegen — the codec is generated per concrete type, no
  reflection, no `dyn`).

**Encoding** is the canonical JSON form already used by the two shipped
crossings: event `args` (serde_json values, `EventBody`) and the hydration
island (serde-serialised, `island_escape`d). One encoder family, generated
per seal type in both directions and both languages (Rust codec in the
emitted program, JS codec in the generated glue) from the same type
definition — so the two sides cannot drift independently of the compiler.

**Decoding is total and fail-closed.** A value arriving from JS that does not
decode to the declared type is dropped at the boundary with an observable
diagnostic (dev console / log), and no partial value is constructed. There is
no `Json.Value`-style typed-hole that lets an undecoded value travel inward.

### 2.2 The three surfaces

1. **`CustomElement down up`** (§3) — declarative; state crosses down as a
   decoded property, events cross up as encoded typed events.
2. **Ports** (later tier, §6) — `port send : T -> Cmd msg` /
   `port receive : (T -> msg) -> Sub msg` in the wasm client, with the same
   seal rules; the JS side subscribes/sends through a generated typed shim.
3. **`Ui.html` + `Html.unsafeRaw` + a served static script** — the existing
   named unsafe surface. Nothing new ships here; its role in this design is
   only to be *documented as the non-typed tier* so no new ad-hoc hatch is
   ever justified by its absence.

### 2.3 Follows Elm / diverges from Elm

| Aspect | Elm | Ipê |
| --- | --- | --- |
| Two port directions (`Cmd` out / `Sub` in), message-passing not function-calling | yes | followed |
| Custom elements as the recommended declarative widget boundary | yes (via `Html.node` + attributes) | followed, but **typed**: Elm's custom-element seam is stringly (attributes hand-stringified, event decoders hand-written); Ipê generates the codec from `CustomElement down up` |
| Flags typed at init | yes; mismatch errors **on the JS side** at `init` | followed via the `HydrationState` island, but mismatch is a **fail-closed Ipê-side decode** (clean-init fallback, `src/compiler/backend/rust/src/project.rs:370`), never a JS exception |
| `Json.Encode.Value` as an allowed boundary type | yes (the recommended escape) | **diverged: rejected.** No undecoded value crosses inward; the failure branch is typed at the boundary instead |
| Ports forbidden in packages | yes | **diverged: allowed**, but disclosed — a JS-touching package carries a mandatory capability (§4) and its scripts are content-hash-pinned |
| Unused-port dead-code elimination | yes | followed: an unused `CustomElement`/port binding and its glue are DCE'd with the rest of the program |

## 3. The `CustomElement` reserved type

### 3.1 Surface

```ipe
codeEditor : CustomElement EditorState EditorEvent
codeEditor = customElement "js/code-editor.js"

view model =
    Ui.widget codeEditor (stateOf model) EditorChanged
```

- `CustomElement down up` — exactly two type parameters, both seal-legal
  (§2.1). Wrong arity, a non-concrete parameter, or a seal-illegal parameter
  is a canon/type error.
- `customElement` is a reserved constructor, legal **only** as the entire body
  of a `CustomElement`-annotated binding, applied to a **single string
  literal** — the same shape discipline as `Ffi.kernel "…"`
  (`src/compiler/canon/src/resolve.rs:4444`). The literal names the author's
  widget-hook JS file, resolved at build time: the compiler cleans the path,
  requires it inside the project root (no `..` escape — reuse the path seal in
  `src/runtime/rust/src/path_core.rs`), requires the file to exist, and hashes
  its content. A non-literal argument, a bare `customElement` value, or a
  missing file is a compile error. (If typed `Path` values land, the
  constructor takes one; the literal requirement stays.)
- `Ui.widget : CustomElement down up -> down -> (up -> msg) -> Element msg` —
  the one view node that places a widget. The `CustomElement` value itself is
  opaque: not comparable, not serialisable, not storable in the `Model` (it
  fails the existing plain-Model gate exactly like a function value).

### 3.2 Typing (replacing the fail-closed rejection)

The IPE-N0037 rejection in `canonicalise_type`
(`src/compiler/canon/src/resolve.rs:4166`) flips to acceptance:
`CustomElement` canonicalises to a builtin two-parameter opaque type. The
reservation (IPE-N0026) stays — user code still cannot declare its own. The
constrain layer gives `customElement` and `Ui.widget` their kernel signatures;
the seal-legality of `down`/`up` is checked where `HydrationState` legality is
checked today, extended with the `Secret`/reserved-sink exclusion.

### 3.3 Codegen

**Element identity.** Each `CustomElement` binding registers one custom
element with a generated, content-addressed tag name
`ipe-ce-<hash-of(binding, seal, js-hash)>`. The tag name never contains user
input; `customElements.define` is called only on generated names.

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

where `decode_down` is the generated total decoder for `down` (unknown field,
wrong shape, wrong tag → drop + console diagnostic, hook never sees it) and
`emit` is the generated encoder for `up` that wraps the encoded value in a
`CustomEvent` (wasm) or posts it through the existing `/_ipe/event` wire
(server-driven). The author hand-writes only the irreducible widget hook, in
the file the literal names (sketch):

```js
export function mount(host, emit) {
  return { onState(state) { /* … */ } };
}
```

**Server-driven (Web shape) transport.** The emitted view renders
`<ipe-ce-… state="{escaped json}">`; the state attribute value goes through
the standard attribute entity-escaper (never `HRaw`). State changes ride the
existing attribute-delta patches (`Patch.attrs`,
`src/runtime/rust/src/dom/diff.rs:14`); `attributeChangedCallback` decodes
and forwards. Up-events post `{handlerId, msg: "ipe-ce", args: [encoded]}`
through `/_ipe/event`; the server resolves the handler by ipe-id exactly as
for clicks (`EventBody`, `src/runtime/rust/src/web/mod.rs:398`), then the
generated **Rust** decoder for `up` parses `args[0]` — fail-closed drop on
mismatch — and dispatches the typed `msg` to `update`.

**Wasm client transport.** Same glue file, property path instead of
attribute: the `hydrate`/`spa` renderer assigns the decoded-side value via
the `state` setter and listens for the typed `CustomEvent`. One glue, two
adapters; the codec is byte-identical in both modes.

**Serving.** Glue + author hook are bundled per widget and served
content-addressed with SRI, exactly like
`/_ipe/client.<hex16>.js` (`src/runtime/rust/src/web/mod.rs:279`). The build
hash recorded at compile time is the hash the page's `integrity` attribute
pins — a swapped file fails to execute.

No dynamic reflection anywhere: every decoder/encoder is concrete generated
code for the concrete seal type, on both sides of the seam.

## 4. Package sharing

JS-interfacing packages are **allowed** (Elm divergence, §2.3) under three
bounds:

1. **Capability disclosure.** A new axis in the closed `Capability` vocabulary
   (`src/compiler/kernels/src/capability.rs:21`): `JsWidget` (wire name
   `js-widget`), inferred for any module whose reachable code contains a
   `CustomElement` binding (later also ports / `Html.unsafeRaw`-reaching
   code, each its own axis if finer disclosure proves useful). The existing
   manifest EQUALS-check (`src/ipe-cli/src/audit.rs:494`) then forces every
   package to declare it, `ipe capabilities` surfaces it transitively, and
   the package-index admission CI re-verifies it — no new machinery, one new
   enum variant plus tagging.
2. **Content pinning.** The package entry (extend
   the `arthurmaciel/ipe-index` `SCHEMA.md`) records the content hash of every shipped
   widget JS file; `ipe package publish` computes it, the admission CI
   re-verifies it, and the consuming build refuses a hash mismatch. The same
   hash is what SRI pins in the served page — one hash from index to browser.
3. **Honest limits.** The native sandbox and the native-code audit protect the
   build and the server process. They do **not** make third-party *browser*
   JS safe: a widget script runs with full DOM authority in the page. The
   shipped guarantee is exactly (a) the boundary is typed and fail-closed,
   (b) the capability is disclosed and auditable, (c) the script is
   hash-pinned and CSP-constrained. Beyond that it is declared trust in the
   package author — and every user-facing doc must say so. Never market the
   sandbox as covering client JS.

## 5. Security

Precedence Security > Correctness. The trust edges, each with its single
enforcement sink:

| # | Edge | Threat | Enforcement (fail-closed) |
| --- | --- | --- | --- |
| 1 | down-state → page | XSS via state serialisation | state crosses only as an entity-escaped attribute value or decoded property — never spliced into a script/`HRaw` position; the island precedent (`island_escape`, `src/runtime/rust/src/web/mod.rs:197`) governs any future inline carrier |
| 2 | up-event → server | forged/malformed browser input | existing session + CSRF gates (`src/runtime/rust/src/web/csrf.rs`, sid authorisation `mod.rs:817`) unchanged; then the generated total decoder — mismatch drops the event, no partial value, body size already capped (`IPE_LIVE_MAX_BODY_BYTES`, `mod.rs:1000`) |
| 3 | widget JS itself | arbitrary code with DOM authority | SRI content-addressing (tamper fails to load), build-time hash from the package index (§4.2), CSP `script-src 'self'`; NOT sandboxed — declared trust, documented |
| 4 | element registration | user string into `customElements.define` | impossible by construction: tag names are compiler-generated |
| 5 | `customElement "path"` literal | path traversal at build | path-seal clean + in-project check at compile time |
| 6 | secrets in the seal | `Secret`/sink types exfiltrated to JS | rejected at the type level (§2.1) |
| 7 | glue serving | header/response splitting | reuse the existing header sanitisation discipline (`frame_ancestors`, `src/runtime/rust/src/telemetry.rs:277`); glue routes are static, no user input in the path |

Standing rules: absent proof a crossing value is safe, the conservative
branch wins; a decode failure is observable but never constructs a value; the
escape hatch stays the *only* un-escaped surface and gains no new power.

**Mandatory gate: a security-soundness-guardian review of every
implementation increment that touches this boundary, before merge.** This is
a language boundary; the review is not optional and not batched away.

## 6. Scope and non-goals

**Ships first (minimal typed boundary):** `CustomElement` typing + seal gate,
the seal codec, `Ui.widget` emission for the server-driven Web shape,
generated glue served with SRI, the `js-widget` capability + package hash
pinning.

**Ships later:** ports for the wasm client; wasm-mode property adapter for
custom elements; per-widget CSP tightening / nonce work; finer capability
axes.

**Non-goals:** an untyped `Value` crossing; synchronous Ipê→JS calls
(request/response belongs to native FFI, not the browser seam); executing JS
server-side; sandboxing third-party browser JS; any second escape hatch.

## 7. Implementation plan (ordered, test-first, independently landable)

Every increment: failing test first → minimal change → full gate (workspace
build + clippy + nextest + affected goldens + emitted-project compile/SEAL) →
**security-soundness-guardian review** (every increment below touches the
boundary). New emission surfaces imply golden additions/re-blesses; that cost
is never a consideration.

1. **Already landed — reserved name + fail-closed rejection.**
   (`negative_suite.rs:463,476,492`; IPE-N0026/N0037.) The contract every
   later increment must not weaken: no untyped fallback ever becomes
   reachable.
2. **Typing acceptance + seal gate.** Failing tests: a
   `CustomElement EditorState EditorEvent` binding with a
   `customElement "js/x.js"` body type-checks (with the file present); arity
   ≠ 2, non-literal argument, missing file, function-carrying seal,
   `Secret`-carrying seal, `CustomElement` in the `Model` — each a named
   negative. Change: flip the `canonicalise_type` arm, add the constructor +
   `Ui.widget` kernel signatures, extend the plain-value gate. Emission still
   rejects (a lower-level fail-closed diagnostic replaces IPE-N0037) so
   nothing unemittable slips to codegen. Gate: negative suite + canon tests;
   goldens unchanged.
3. **Seal codec.** Failing tests: Rust-side encode/decode round-trip per seal
   shape (record, ADT, nested, `Maybe`, list) plus adversarial
   malformed-input tests asserting typed failure (no panic, no partial
   value). Change: the generated per-type codec in the emitted program,
   sharing the event-args JSON conventions. No JS yet. Gate: unit tests;
   soundness (no `unwrap` on wire data).
4. **Server-driven emission.** Failing test: a golden Web fixture whose view
   uses `Ui.widget` emits the `<ipe-ce-…>` element with the escaped state
   attribute; a runtime test that a state change produces an attribute-delta
   patch and an incoming `/_ipe/event` with a valid encoded `up` dispatches
   the typed msg — and a malformed one is dropped. Change: lower/emit for
   `Ui.widget`, handler-index wiring, diff coverage. Gate: goldens +
   emitted-project compile + the two runtime tests.
5. **Glue generation + serving.** Failing test: browser E2E (playwright, as
   in `examples/*/playwright-test.mjs`): widget mounts, state flows down
   through `attributeChangedCallback`, a user action flows up as a typed msg,
   a hand-forged malformed event is dropped with a console diagnostic; the
   served glue URL carries a matching SRI hash. Change: glue emitter,
   bundling, content-addressed route.
6. **Capability + package pinning.** Failing tests: `ipe capabilities`
   reports `js-widget` for a widget-bearing program; the audit EQUALS-check
   rejects an undeclared `js-widget`; the index validator rejects an entry
   whose recorded JS hash mismatches the source. Change: `Capability` variant
   + tagging, SCHEMA field, publish/audit/admission wiring.
7. **Later tier.** Wasm property adapter (same glue, property path; E2E under
   `[wasm] mode`), then ports (`send`/`receive` seal reuse). Each its own
   doc-level addendum + guardian review.

## 8. Risks and cost

- **XSS regression risk** concentrates in the server-driven-emission
  increment: one emit path writing state anywhere but the escaped-attribute
  sink reopens injection. Mitigation: a single state-carrier function shared
  by render and diff, plus adversarial goldens (`</script>`, quote-breaking
  payloads) as permanent fixtures.
- **Dual-language codec drift.** The Rust and JS codecs are generated from
  one type but live in two emitters; drift produces silent event drops
  (fail-closed hides bugs as lost events). Mitigation: the round-trip browser
  E2E, a shared canonical-JSON spec section in the codec module docs, and
  observability of every boundary drop.
- **Supply-chain surface** (the Elm-divergence cost): allowing JS packages
  imports the risk Elm's ban avoided. Bounded by §4 (capability EQUALS +
  hash pinning + honest docs) — but the residual is real and permanently
  declared trust.
- **Server-driven round-trip latency.** Every up-event crosses the network;
  a high-frequency widget (editor keystrokes) must batch or keep interaction
  local inside the widget and emit coarse events. Guidance belongs in the
  widget-author docs; no compiler mechanism in the first tier.
- **Two-transport divergence.** Attribute (SSR) vs property (wasm) adapters
  can drift behaviourally; the single-glue/two-adapter split plus shared E2E
  fixtures is the containment.
- **Emit complexity.** Per-widget glue, routes, hashes, and capability wiring
  touch project emission, the web runtime, the CLI, and the index schema —
  wide but shallow; the increment split keeps each landing narrow, and the
  standing fail-closed gates mean a half-landed state can never emit an
  untyped seam.
