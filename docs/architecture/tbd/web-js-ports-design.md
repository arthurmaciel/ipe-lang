# JS integration — one typed port, two transports

Status: design proposal, no implementation yet. The Ipê sketches are
**illustrative of the proposed surface** — this capability does not exist yet, so
they are not runnable; they show intended types, not shipped API.

## The problem

An Ipê app reaches the browser two ways. In the **server-driven Web shape**,
`Model`/`update`/`view` run on the server and the browser is a thin client
applying streamed DOM patches. In the **client-WASM target** (ADR-0042) the same
`init`/`update`/`view` app is compiled to WebAssembly and runs *in* the browser,
patching the real DOM via `web-sys`. Either way, some things only the browser can
do — a third-party map or charting widget, `WebAudio`, `Geolocation`,
`IndexedDB`, a payment SDK — and Ipê needs an expression for them.

The client historically carried a `data-ipe-eval` / `new Function` seam for this,
which AGENTS.md forbids: it executes supplied strings in the browser, an
unbounded code-injection surface with no type at the boundary. We need a JS
boundary that is **explicit, typed, and secure**: every byte that crosses passes
a total decoder or a typed projection, and the set of things JS can do is
declared, not open-ended.

## The reference, and the twist

Elm's answer is **ports**: the one sanctioned JS boundary, two one-way typed
channels — `Value -> Cmd msg` outbound, `(Value -> msg) -> Sub msg` inbound, with
decoding on the Elm side. Async message passing, no return value, which keeps the
pure core pure.

Elm's ports are in-browser (Elm and JS share a process). Ipê has *both* that case
(client-WASM) and a network-spanning case (server-driven Web, where the TEA loop
is on the server and JS is in the browser). The developer-facing surface is one
and the same; the compiler lowers it per target.

## The surface: two channel kinds, three primitives

A single declared *port module* names, in one place, everything that may cross
the JS boundary. It has two channel *kinds*: discrete **messages** (bidirectional,
`send`/`receive`) and a continuous **state mirror** (one-way, `sync`).

### Messages in — `receive : Decoder JsMsg`

JS sends a message; the runtime decodes it into a typed value before `update`
ever sees it.

```
type JsMsg
    = LocationFixed { lat : Float, lng : Float }
    | PaymentAuthorized { token : String }

receive : Decoder JsMsg
```

The decoder **is** the security gate — parse-don't-validate at the trust
boundary. The core never handles a raw blob, only `Result DecodeError JsMsg`.

### Messages out — `send : JsCmd -> Cmd Msg`

A one-shot imperative effect that is not state (play a sound, fire analytics,
open a file picker) — the classic Elm outbound port, encoded and delivered to a
JS handler.

```
type JsCmd = PlayChime | ScrollTo { anchor : String }
send : JsCmd -> Cmd Msg
```

### State mirror — `sync : Model -> JsState`

A declarative slice of state JS should mirror (the data behind a chart, a map's
markers). It is a *second view*: a pure projection with its own encoder, diffed
and delivered on the same outbound channel as the DOM patches, re-emitted only
when it changes. It is to JS-mirrored data what `view` is to the DOM —
declarative, framework-diffed, no change-detection code.

```
sync : Model -> JsState
encodeJsState : JsState -> Value
```

| Kind | Flow | Ipê port module | JS `ipe` |
|---|---|---|---|
| Message | JS → core (intent) | `receive : Decoder JsMsg` | `ipe.send(msg)` |
| Message | core → JS (command) | `send : JsCmd -> Cmd Msg` | `ipe.onReceive(cb)` |
| State | core → JS (mirror) | `sync : Model -> JsState` | `ipe.onSync(cb)` |

`send`/`receive` for the bidirectional message channel; `sync`/`onSync` for the
one-way state mirror — every channel shares a root across both sides. From each
side's local view, `send` means "push to the other party" — the direction is
implicit in which side you are on, as with a socket.

## Declaring a port — no new language construct

There is no `port module` keyword or special module kind. A port is expressed
with primitives Ipê already has:

- **Outbound command** is a `Cmd`: `Js.send : a -> Cmd msg` (the `JsCmd` ADT's
  wire encoder is derived). `update` returns `Js.send PlayChime` — or the
  generated per-variant `playChime`.
- **Inbound intent** is a `Sub`: `Js.subscribe : Decoder a -> (a -> msg) -> Sub msg`,
  wired in `subscriptions`.
- **State mirror** is a field on the shape's cfg record — `sync : Model -> JsState`,
  alongside `view`, diffed the same way.

```
-- App/Ports.ipe — the audit surface is these two closed ADTs
type JsMsg = LocationFixed { lat : Float, lng : Float } | PaymentAuthorized { token : String }
type JsCmd = PlayChime | ScrollTo { anchor : String }

update msg model =
    case msg of
        Chimed                  -> ( model, Js.send PlayChime )   -- outbound: a Cmd (or `playChime`)
        GotJs (LocationFixed c) -> ( { model | at = c }, Cmd.none )

subscriptions model = Js.subscribe receiveJsMsg GotJs            -- inbound: a Sub

main = Web.app { init, update, view, subscriptions, sync = projectJs }  -- state mirror: a cfg field
```

The auditable "everything JS can do" surface is the two closed ADTs `JsMsg` and
`JsCmd` — read those two declarations and you have seen the whole boundary.
Colocating them (with `sync` and any custom decoder) in one `App.Ports` module is
a readability convention, not a compiler construct. Encoders/decoders derive from
the ADT by default and are hand-writable for a custom wire format; the mandatory
concrete-ADT seal applies to `Js.send`'s argument and `Js.subscribe`'s decoder
type. Shape is inferred from the entry kernel and everything is configured by
records + `Cmd`/`Sub`, so ports reuse that machinery rather than adding syntax.

## Two decisions that keep the boundary honest

### The boundary type must be a concrete ADT — a mandatory seal

A port's declared inbound/outbound type MUST be a concrete declared type, never
`Json.Value` (nor any opaque passthrough). `Decoder JsMsg` where `JsMsg` is a
real sum type — the compiler **rejects** `Decoder Value`:

```
receive : Decoder Value
--                 ^^^^^ rejected: a JS-port boundary type must be a declared
--                 concrete type. Name the messages JS may send as a sum type.
```

This is the single rule that structurally forecloses Elm's `Value` free-for-all:
the untyped channel *cannot be spelled*. It is fail-closed by construction
(Security #1) and make-invalid-states-unrepresentable pointed at the boundary
type itself — an opaque port is exactly the "silent flexible variable that defers
failure downstream" the fundamental rules forbid. A genuinely opaque passthrough
is expressed by *naming* it (`type RawJson = RawJson String`), so "not
interpreted here" is explicit and greppable, not an untyped hole — Completeness
(#5) is kept without conceding Security (#1). Advisory (warn-but-allow) is
rejected: a lower principle can never justify compromising Security.

### One closed ADT per direction, with generated per-variant senders

The published JS surface is a single closed `JsCmd` / `JsMsg` sum type, not a set
of scattered per-function port declarations. The closed type **is** the attack
surface as one auditable object (read one `type JsMsg`, see everything JS can do),
and the inbound `case jsMsg of` is exhaustive — publish a new variant and every
handler fails to compile until it is handled. Per-function ports (Elm's `port
playTone : … -> Cmd msg`) scatter the surface and let an inbound port be silently
forgotten in `subscriptions`; the single ADT makes "published but unhandled"
unrepresentable. Elm's per-function *ergonomics* are recovered without giving up
the closed type: the compiler generates a per-variant sender.

```
send PlayChime      -- the primitive
playChime           -- generated: = send PlayChime
```

## Three refinements that make it principled

- **`JsMsg` is a distinct, narrow ADT — never all of `Msg`.** The browser is
  fully attacker-controlled. "The messages JS may send" must not be the internal
  `Msg` type — that would hand an attacker every transition the state machine can
  reach (`AdminPurge`, `SetBalance`). `JsMsg` is a separate small type the runtime
  maps into `Msg`; an attacker can only name transitions explicitly published.
- **`sync` is a projection, never the raw `Model`.** The same discipline the DOM
  already has (ship `view model`, never the Model). Raw field access into the
  Model for the JS stream is structurally impossible; secrets cannot leak because
  they are never in `JsState`. (Rationale shifts by target — see below.)
- **One schema, both directions.** `receive`'s decoder and `sync`'s encoder are
  two faces of one declared port type, so browser and core cannot drift on wire
  format.

## One boundary, two orthogonal flags

The boundary to non-Ipê code is one abstraction. What varies across cases is two
*independent* flags — conflating them is what left client-WASM without a home:

- **Trust → decode gate.** Is the far side trusted? Untrusted (browser JS,
  sandboxed Rust) → the inbound decode gate is **ON**. Trusted (pure in-process
  Rust) → a direct typed binding, no gate. Keyed on trust, **never on transport**:
  browser JS is attacker-controlled whether across a network or in the same page,
  so in-process ≠ trusted.
- **Ordering → staleness layer.** Does the transport preserve ordering against the
  runtime? Network / async-unordered → optimistic-concurrency handling.
  In-process-ordered → none. Keyed on transport latency, never on trust.

| Far side | Transport | Trust → gate | Ordering → staleness |
|---|---|---|---|
| Browser JS, server-driven Web | network (server↔browser stream) | untrusted → ON | unordered → yes |
| Browser JS, client-WASM | in-process (wasm-bindgen, same page) | untrusted → ON | ordered → none |
| Tier-2 sandboxed Rust | subprocess pipe (the run jail) | semi-trusted → ON | unordered → yes |
| Pure in-process Rust | same address space | trusted → OFF (direct binding) | ordered → none |

The port is declared once; `--target` picks the transport; these two flags decide
what the lowering emits.

## Per-target lowering

| | Server-driven Web | Client-WASM |
|---|---|---|
| TEA loop runs | server | browser (WASM) |
| Transport | network stream | in-process (wasm-bindgen) |
| Inbound gate (`receive`) | ON | **ON** — unchanged; in-process ≠ trusted |
| `send` delivery | encode → stream → `ipe.onReceive` | direct dispatch, same tick |
| `sync` mirror | encode → **diff** → stream deltas | direct in-memory handoff |
| Staleness handling | present | **absent** — single-threaded, ordered, like real Elm |
| Developer surface | `receive`/`send`/`sync` | **identical** |
| App's JS glue | `ipe.send`/`onReceive`/`onSync` | **identical** |

Two runtime implementations of the `ipe` object (one posts over the network, one
calls into the WASM instance); the app's handlers and the Ipê port declarations
are byte-identical across both. This is ADR-0042's "one backend, inherit don't
fork" applied to the JS boundary: **write once, deploy server-driven or client
bundle unchanged.**

## Stale reads and ordering — the network-only hazard

Elm ports are in-process and single-threaded, so inbound messages are ordered
against the runtime. **Client-WASM inherits that** — no staleness. Only the
**server-driven** transport has the distributed hazard: JS acts on a `JsState` it
received over the network, then sends a `JsMsg`, but by the time the server folds
it the Model has moved on. JS always holds a stale, read-only replica.

- `JsState` is authoritative-server and read-only. JS never "writes" it; it sends
  *intents* (`JsMsg`), and the server reconciles them against the current Model. A
  `JsMsg` is a request, not an assertion about state.
- The default is fold-against-current: an intent is an event, folded into whatever
  the Model is now — correct for the common case.

### Optimistic concurrency is typed application logic, not framework machinery

There is deliberately **no** framework version-token / `Fresh`/`Stale` mechanism.
Optimistic concurrency — the case where blindly folding a stale intent would be
*wrong*, not merely folded-against-current — is expressed by **naming the
precondition the intent depends on as an explicit typed field**, which is
parse-don't-validate applied to the intent itself:

```
-- compare-and-swap: the observed value travels in the intent, typed
type JsMsg = SaveField { expected : FieldValue, next : FieldValue }

-- update applies only if the precondition still holds
SaveField m ->
    if currentField model == m.expected
    then ( setField m.next model, Cmd.none )
    else ( rejectStale model, Cmd.none )
```

This is stronger than an opaque framework "version token" (the validate-later
pattern the fundamental rules push against): the dependency is typed, explicit,
and greppable. Most apparent staleness also just *dissolves* once intents
reference **stable identities** rather than snapshot positions — a reorder that
says "move B before A" folds against current state safely, where "move index 1 to
0" does not.

The default remains fold-against-current (an intent is an event). The one thing
this leaves out is a *framework-forced* whole-state optimistic lock; it is
expressible today with a developer-maintained version field, and a forced variant
can be added later iff real usage demands it — not built speculatively (YAGNI,
and no wire-version overhead on every fire-and-forget port).

## `sync`'s rationale shifts by target

Server-side, `sync` is a **secret boundary**: the Model holds server state that
must not reach the browser. Client-side the Model is already in the browser, so
`sync` is an **encapsulation boundary**: host and third-party page scripts see
only the declared typed slice, not the internal Model shape — with a residual
secret role against *other* scripts sharing the page. Same mechanism, both
motivations hold; `sync` is not dead weight client-side.

## Stable browser APIs are kernel-backed stdlib, not packages

The `WasmClient` security model forces this: FFI is denied client-side and *"the
client's only host surface is the fixed web-sys allowlist"* (ADR-0042). Granting a
browser capability *widens that host surface* — a compiler+runtime decision, not
something a downloaded package may do. A pure-`.ipe` package cannot ship its own
`web-sys` glue (arbitrary host access, forbidden client-side), and letting a
"blessed" package register allowlisted kernels reintroduces the central trust the
decentralized-packaging stance rejects.

So the split, driven by Security #1:

- **Primitive host access → first-party kernel + vendored runtime, target-gated**
  — exactly how `Http` (fetch), WebSocket, timers, and `Random` already live.
  Geolocation, storage/IndexedDB, WebAudio, clipboard, notifications join them as
  first-party `Ipe.Browser.*` stdlib modules backed by allowlisted kernels.
- **Ergonomic APIs → `.ipe`, composing over those kernels** — pure Ipê helpers,
  nicer types; community packages, because they introduce no new host access.

A browser capability not yet shipped as a kernel is reached through the **typed
port** (your own audited JS handler) until it graduates to a first-party kernel —
the port is the escape valve precisely because packages cannot introduce host
access.

## Custom elements — the typed visual-widget tier

Ports carry imperative effects and out-of-band data; they have no place in the
view tree. A **visual** third-party widget — a map, a chart, a date-picker —
belongs *in* the view, and gets its own boundary: a custom element.

`Ui.customElement : String -> List (Attribute msg) -> List (Element msg) -> Element msg`
renders a `<tag …>` node that composes through the existing DOM-patch channel; the
browser's custom-element machinery instantiates the JS-backed widget. Attributes
flow in (Ipê-typed, encoded); events flow out through typed decoders into `Msg` —
the same decode-at-the-boundary discipline as a port, with the mandatory
concrete-ADT seal on the event decoder. The developer wraps the primitive in a
typed constructor:

```
map : { lat : Float, lng : Float, onMarkerClick : MarkerId -> msg } -> Element msg
map cfg =
    Ui.customElement "ipe-map"
        [ Ui.attrFloat "lat" cfg.lat
        , Ui.attrFloat "lng" cfg.lng
        , Ui.onCustom "marker-click" (Decode.map cfg.onMarkerClick markerId)
        ]
        []
```

The widget's JS is a checked-in `customElements.define('ipe-map', …)`, loaded at
build, never `eval`'d — the same audit model as a port handler. It works on both
browser transports (server-driven renders the tag in DOM patches; client-WASM
instantiates it via `web-sys`) and, being DOM, not on the Terminal shape.

**Decision rule:** visual-in-the-view → custom element; imperative effect or
out-of-band data → port. `Ui.html` embeds *static* raw HTML; a custom element is
its typed, live, interactive counterpart.

## Relationship to Rust FFI

The port is a boundary to non-Ipê code, and so is the Rust FFI. The axis that
decides *whether something is a port* is not the target language (JS vs Rust) but
the **trust and execution model of the far side** — the same trust/ordering flags
above. Browser JS and capability-isolated Rust land on the port side; pure
in-process Rust lands on the direct-binding side. A pure total Rust function — a
parser, a hash — surfaces as an ordinary Ipê value (`let h = blake3 bytes`), not a
`Cmd` round-trip: trusted, synchronous, same address space (gate off, ordered).
Async Rust and Tier-2 sandboxed Rust are ports under a different name — see
[async-ffi-bridge-design.md](async-ffi-bridge-design.md).

## Alternatives considered

- **Custom elements** — kept, not rejected: the typed visual-widget tier,
  complementary to ports. See "Custom elements" above.
- **Hooks / lifecycle callbacks** (arbitrary JS on node mount/update). Rejected as
  the primary boundary: the callback body is untyped JS with ambient DOM access —
  the unbounded surface of the forbidden eval seam, only spelled differently.

## Implementation sequencing

Phased by dependency; the two transports reuse infrastructure that already
exists (the server→browser DOM-patch stream + `data-ipe-ev` reverse channel for
server-driven; the ADR-0042 Cmd/Sub browser bridge + wasm DOM sink for
client-WASM), so no new transport has to be built from scratch.

**Can start now:**

1. **Port primitives + the mandatory ADT seal (land together).** `Js.send : a ->
   Cmd msg` and `Js.subscribe : Decoder a -> (a -> msg) -> Sub msg` as
   Cmd/Sub kernels reusing the existing effect machinery; the seal is a
   canon/types rule rejecting `Decoder Value` / opaque types at a port boundary
   (Security #1, lands with the primitives it guards). Derived ADT
   encoder/decoder by default.
2. **`sync` cfg field.** A projection field on `Web.app`/`WebView.app`, diffed and
   streamed by the same machinery as the DOM patches server-side, handed over
   in-memory client-side.
3. **Generated per-variant senders.** A desugar over the `JsCmd` ADT (`playChime`
   = `Js.send PlayChime`); small, follows the primitives.
4. **The fixed JS runtime API** `ipe.send` / `onReceive` / `onSync`, in two
   implementations (network + wasm), each plugging into its existing transport.
   This step removes the `data-ipe-eval` / `new Function` seam.

**Follow-on, independent:**

5. **Custom elements** — `Ui.customElement` + typed attribute encoders / event
   decoders, over the existing DOM-patch channel. Separate node family; not on the
   port core's critical path.
6. **Browser-API kernels** (`Ipe.Browser.Geolocation`, storage, WebAudio, …) —
   each an independent target-gated kernel + `web-sys` denotation, added
   incrementally as demand appears; graduating a capability off the typed port.

The optimistic-concurrency stance is documentation only (the typed-precondition
pattern needs no build). Order: (1)+seal → (2)+(3)+(4) per transport, in parallel
→ (5), (6) as independent follow-ons.

## Boundaries

- Design-only; nothing here is implemented.
- Applies to both browser execution models — server-driven Web **and** client-WASM
  — via the one surface + two transports. WebView (its own host bridge) and the
  Terminal shape (no JS) are out of scope.
- The JS-side runtime surface is a small fixed API — `ipe.send` / `onReceive` /
  `onSync` — never `eval` / `new Function`. It replaces the `data-ipe-eval` seam,
  which this design removes.
- Capability inference: a program that declares a JS port is exercising the
  browser-scripting capability and is classified accordingly.
