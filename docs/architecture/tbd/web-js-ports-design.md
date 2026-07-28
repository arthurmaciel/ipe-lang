# Web-shape JS integration — network-spanning typed ports

Status: design proposal, no implementation yet.

## The problem

The Web shape is server-driven TEA: `Model`, `update`, and `view` run on the
server; the browser is a thin client that applies DOM patches streamed over the
server→browser event stream and reports user events back through
`data-ipe-ev-<name>` markers. Ipê never runs in the browser.

That leaves a gap. Some things only the browser can do — a third-party map or
charting widget, `WebAudio`, `Geolocation`, `IndexedDB`, a payment SDK — and
Ipê has no expression for them. The client currently carries a
`data-ipe-eval` / `new Function` seam for this, which AGENTS.md forbids: it
executes server-supplied strings in the browser, an unbounded code-injection
surface with no type at the boundary.

We need a JS boundary that is **explicit, typed, and secure**: every byte that
crosses it passes a total decoder or a typed projection, and the set of things
JS can do is declared, not open-ended.

## The reference, and the twist

Elm's answer is **ports**: the one sanctioned JS boundary, two one-way typed
channels — `Value -> Cmd msg` outbound, `(Value -> msg) -> Sub msg` inbound,
with decoding on the Elm side. Ports are async message passing with no return
value, which is what keeps the pure core pure.

The twist for the Web shape: Elm's ports are *in-browser* (Elm and JS share a
process), but here the TEA loop is on the **server** and JS is in the
**browser**, so the port spans the network. The shape of the API is Elm's; the
hazards are distributed-systems hazards Elm never had to face.

## Chosen design: three typed, schema-guarded channels

A single declared *port module* names, in one place, everything that may cross
the JS boundary. It compiles to three channels. The Ipê sketches below are
**illustrative of the proposed surface** — this capability does not exist yet,
so they are not runnable; they show the intended types, not shipped API.

### 1. Inbound intent — `jsInbound : Decoder JsMsg`

JS sends a message; the server decodes it into a typed value before `update`
ever sees it.

```
type JsMsg
    = LocationFixed { lat : Float, lng : Float }
    | PaymentAuthorized { token : String }

jsInbound : Decoder JsMsg
```

The decoder **is** the security gate — parse-don't-validate at the trust
boundary. The server never handles a raw blob, only `Result DecodeError JsMsg`.

### 2. Outbound projection — `viewJs : Model -> JsState`

A declarative slice of state that JS should always mirror (the data behind a
chart, a map's marker set). It is a *second view*: a pure projection with its
own encoder, diffed and streamed over the same server→browser channel as the
DOM patches, re-emitted only when the projection changes.

```
viewJs : Model -> JsState        -- derived, publishable projection
encodeJsState : JsState -> Value
```

### 3. Outbound command — `sendToJs : JsCmd -> Cmd Msg`

A one-shot imperative effect that is not state (play a sound, fire an analytics
event, open a file picker) — the classic Elm outbound port, encoded and
delivered to a JS handler.

```
type JsCmd = PlayChime | ScrollTo { anchor : String }
sendToJs : JsCmd -> Cmd Msg
```

| Channel | Direction | Kind |
|---|---|---|
| `jsInbound : Decoder JsMsg` | JS → server | intent, decoded at the trust boundary |
| `viewJs : Model -> JsState` | server → JS | declarative projection, diffed |
| `sendToJs : JsCmd -> Cmd Msg` | server → JS | imperative one-shot effect |

## Three refinements that make it principled

### `JsMsg` is a distinct, narrow ADT — never all of `Msg`

The browser is fully attacker-controlled: anyone can open devtools and call the
send API with any payload. So "the messages JS may send" must **not** be the
internal `Msg` type — that would hand an attacker every transition the state
machine can reach, including ones the UI never exposes (`AdminPurge`,
`SetBalance`). `JsMsg` is a separate, deliberately small type; the server maps
it into `Msg`. An attacker can only name transitions that were explicitly
published. This is make-invalid-states-unrepresentable applied to the attack
surface itself.

### `viewJs` is a projection, never the raw `Model`

Sending "part of the Model" to JS sends it to the browser, i.e. to the user.
The Model may hold other users' data, server tokens, or unpublished computed
state. The discipline already exists for the DOM — you never ship the raw Model
to the screen, you ship `view model` — and it applies unchanged here: `viewJs`
is the one place that decides what leaves the server on this channel. Raw field
access into the Model for the JS stream is structurally impossible; secrets
cannot leak because they are never in `JsState`.

### One schema, both directions

`jsInbound`'s decoder and `viewJs`'s encoder are the two faces of a single
declared port type, so the browser and server cannot drift on wire format.
Parse-don't-validate holds on both ends of one contract.

## The hazard Elm does not have: stale reads and ordering

Elm ports are in-process and single-threaded, so inbound messages are ordered
against the runtime. Here JS acts on a `JsState` it received over the network,
then sends a `JsMsg` — but by the time the server folds that message the Model
has already moved on. **JS always holds a stale, read-only replica.** The design
answers this explicitly:

- `JsState` is authoritative-server and read-only. JS never "writes" it; it
  sends *intents* (`JsMsg`), and the server reconciles them against the current
  Model. A `JsMsg` is a request, not an assertion about state.
- When an intent must be conditional on what JS saw, it carries a **sequence
  token** taken from the `JsState` it was derived from, and `update` may reject
  or rebase an intent whose token is stale (optimistic concurrency). The token
  is part of the projection's schema, not something the developer wires by hand.

## Developer mental model

There are now three server↔browser channels: framework-owned DOM patches
(out), framework-owned `data-ipe-ev-*` events (in), and this developer-owned
port (both ways). The rule that keeps it comprehensible is unchanged: **the only
way to change server state is to send a message.** `JsState` is explicitly a
read-only replica, not a source of truth, so a developer who mutates it in JS
sees no server effect — because state changes travel only as `JsMsg` intents.
Framing the projection as read-only is what prevents the "why didn't my JS edit
stick?" confusion.

## Alternatives considered

- **Network-spanning ports (chosen).** Typed, minimal, Elm-aligned; every
  crossing is a decoder or a projection. The imperative/declarative split
  (`sendToJs` vs `viewJs`) covers both one-shot effects and always-in-sync data.
- **Custom elements.** JS widgets encapsulated as DOM nodes with typed
  attributes and events, composing through the existing DOM-patch channel. This
  is complementary, not competing: it is the right tool for a *visual* widget
  embedded in the view, whereas ports are for imperative side effects and
  out-of-band data. A later design can add custom elements on top; they do not
  replace ports.
- **Hooks / lifecycle callbacks** (run arbitrary JS on node mount/update).
  Rejected as the primary boundary: the callback body is untyped JS with
  ambient DOM access — the same unbounded surface as the forbidden eval seam,
  only spelled differently. Ports keep the boundary declared and decoded.

## Boundaries

- Design-only; nothing here is implemented. It defines the *shape* of a future
  capability, not a schedule.
- The JS-side runtime surface is a small fixed API — an intent sender, a
  projection subscription, and a command handler registry (e.g. `ipe.send`,
  `ipe.onState`, `ipe.onCommand`) — never `eval`/`new Function`. It replaces the
  `data-ipe-eval` seam, which this design removes.
- WebView and TUI are out of scope: WebView has its own host bridge and TUI has
  no JS. This is specific to the Web shape's server↔browser split.
- Capability inference: a program that declares a JS port is exercising the
  browser-scripting capability and is classified accordingly, the same as any
  other effectful surface.
