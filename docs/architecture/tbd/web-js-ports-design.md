# JS integration — one typed port, two transports

Status: design proposal, no implementation yet. The Ipê sketches are
**illustrative of the proposed surface** — this capability does not exist yet, so
they are not runnable; they show intended types, not shipped API. The names
(`jsInbound`, `sendToJs`, `viewJs`, `ipe.send`/`onCommand`/`onState`) are
placeholders under active review — see "Naming (open)".

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

## The surface: three typed, schema-guarded channels

A single declared *port module* names, in one place, everything that may cross
the JS boundary. It compiles to three channels.

### 1. Inbound intent — `jsInbound : Decoder JsMsg`

JS sends a message; the runtime decodes it into a typed value before `update`
ever sees it.

```
type JsMsg
    = LocationFixed { lat : Float, lng : Float }
    | PaymentAuthorized { token : String }

jsInbound : Decoder JsMsg
```

The decoder **is** the security gate — parse-don't-validate at the trust
boundary. The core never handles a raw blob, only `Result DecodeError JsMsg`.

### 2. Outbound projection — `viewJs : Model -> JsState`

A declarative slice of state JS should mirror (the data behind a chart, a map's
markers). It is a *second view*: a pure projection with its own encoder,
delivered on the same outbound channel as the DOM patches, re-emitted only when
it changes.

```
viewJs : Model -> JsState
encodeJsState : JsState -> Value
```

### 3. Outbound command — `sendToJs : JsCmd -> Cmd Msg`

A one-shot imperative effect that is not state (play a sound, fire analytics,
open a file picker) — the classic Elm outbound port, encoded and delivered to a
JS handler.

```
type JsCmd = PlayChime | ScrollTo { anchor : String }
sendToJs : JsCmd -> Cmd Msg
```

| Channel | Direction | Kind |
|---|---|---|
| `jsInbound : Decoder JsMsg` | JS → core | intent, decoded at the trust boundary |
| `viewJs : Model -> JsState` | core → JS | declarative projection, diffed |
| `sendToJs : JsCmd -> Cmd Msg` | core → JS | imperative one-shot effect |

## Two decisions that keep the boundary honest

### The boundary type must be a concrete ADT — a mandatory seal

A port's declared inbound/outbound type MUST be a concrete declared type, never
`Json.Value` (nor any opaque passthrough). `Decoder JsMsg` where `JsMsg` is a
real sum type — the compiler **rejects** `Decoder Value`:

```
jsInbound : Decoder Value
--                  ^^^^^ rejected: a JS-port boundary type must be a declared
--                  concrete type. Name the messages JS may send as a sum type.
```

This is the single rule that structurally forecloses Elm's `Value` free-for-all:
the untyped channel *cannot be spelled*. It is fail-closed by construction
(Security #1) and make-invalid-states-unrepresentable pointed at the boundary
type itself — an opaque port is exactly the "silent flexible variable that defers
failure downstream" the fundamental rules forbid. A genuinely opaque passthrough
is expressed by *naming* it (`type RawJson = RawJson String`), so "not
interpreted here" is explicit and greppable, not an untyped hole — Completeness
(#5) is kept without conceding Security (#1).

Advisory (warn-but-allow) is rejected: it leaves the permissive branch reachable,
and a lower principle (the Readability/Completeness convenience of `Value`) can
never justify compromising Security.

### One closed ADT per direction, with generated per-variant senders

The published JS surface is a single closed `JsCmd` / `JsMsg` sum type, not a set
of scattered per-function port declarations. The closed type **is** the attack
surface as one auditable object (read one `type JsMsg`, see everything JS can do),
and the inbound `case jsMsg of` is exhaustive — publish a new variant and every
handler fails to compile until it is handled. Per-function ports (Elm's `port
playTone : … -> Cmd msg`) scatter the surface and let an inbound port be silently
forgotten in `subscriptions`; the single ADT makes "published but unhandled"
unrepresentable.

Elm's per-function *ergonomics* are recovered without giving up the closed type:
the compiler generates a per-variant sender, so call sites read directly.

```
sendToJs PlayChime      -- the primitive
playChime               -- generated: = sendToJs PlayChime
```

One closed type for Security + exhaustiveness; generated sugar for Readability.

## Three refinements that make it principled

- **`JsMsg` is a distinct, narrow ADT — never all of `Msg`.** The browser is
  fully attacker-controlled. "The messages JS may send" must not be the internal
  `Msg` type — that would hand an attacker every transition the state machine can
  reach (`AdminPurge`, `SetBalance`). `JsMsg` is a separate small type the runtime
  maps into `Msg`; an attacker can only name transitions explicitly published.
- **`viewJs` is a projection, never the raw `Model`.** The same discipline the DOM
  already has (ship `view model`, never the Model). Raw field access into the
  Model for the JS stream is structurally impossible; secrets cannot leak because
  they are never in `JsState`. (Rationale shifts by target — see below.)
- **One schema, both directions.** `jsInbound`'s decoder and `viewJs`'s encoder
  are two faces of one declared port type, so browser and core cannot drift on
  wire format.

## One boundary, two orthogonal flags

The boundary to non-Ipê code is one abstraction. What varies across cases is two
*independent* flags — conflating them is what left client-WASM without a home:

- **Trust → decode gate.** Is the far side trusted? Untrusted (browser JS,
  sandboxed Rust) → the inbound decode gate is **ON**. Trusted (pure in-process
  Rust) → a direct typed binding, no gate. Keyed on trust, **never on transport**:
  browser JS is attacker-controlled whether it sits across a network or in the
  same page, so in-process ≠ trusted.
- **Ordering → staleness layer.** Does the transport preserve ordering against the
  runtime? Network / async-unordered → sequence tokens (optimistic concurrency).
  In-process-ordered → **no tokens**. Keyed on transport latency, never on trust.

| Far side | Transport | Trust → gate | Ordering → tokens |
|---|---|---|---|
| Browser JS, server-driven Web | network (server↔browser stream) | untrusted → ON | unordered → tokens |
| Browser JS, client-WASM | in-process (wasm-bindgen, same page) | untrusted → ON | ordered → none |
| Tier-2 sandboxed Rust | subprocess pipe (the run jail) | semi-trusted → ON | unordered → tokens |
| Async in-process Rust | the async bridge | trusted far side, async | (Task completion) |
| Pure in-process Rust | same address space | trusted → OFF (direct binding) | ordered → none |

The port is declared once; `--target` picks the transport; these two flags decide
what the lowering emits.

## Per-target lowering

| | Server-driven Web | Client-WASM |
|---|---|---|
| TEA loop runs | server | browser (WASM) |
| Transport | network stream | in-process (wasm-bindgen) |
| Inbound gate (`jsInbound`) | ON | **ON** — unchanged; in-process ≠ trusted |
| `sendToJs` delivery | encode → stream → `ipe.onCommand` | direct dispatch, same tick |
| `viewJs` projection | encode → **diff** → stream deltas | direct in-memory handoff |
| Sequence tokens | present | **absent** — single-threaded, ordered, like real Elm |
| Developer surface | `jsInbound`/`sendToJs`/`viewJs` | **identical** |
| App's JS glue | `ipe.send`/`onCommand`/`onState` | **identical** |

Two runtime implementations of the `ipe` object (one posts over the network, one
calls into the WASM instance); the app's handlers and the Ipê port declarations
are byte-identical across both. This is ADR-0042's "one backend, inherit don't
fork" applied to the JS boundary: **write once, deploy server-driven or client
bundle unchanged.**

## Stale reads and ordering — the network-only hazard

Elm ports are in-process and single-threaded, so inbound messages are ordered
against the runtime. **Client-WASM inherits that** — no staleness, no tokens. Only
the **server-driven** transport has the distributed hazard: JS acts on a `JsState`
it received over the network, then sends a `JsMsg`, but by the time the server
folds it the Model has moved on. JS always holds a stale, read-only replica.

- `JsState` is authoritative-server and read-only. JS never "writes" it; it sends
  *intents* (`JsMsg`), and the server reconciles them against the current Model. A
  `JsMsg` is a request, not an assertion about state.
- When an intent must be conditional on what JS saw, it carries a **sequence
  token** taken from the `JsState` it was derived from, and `update` may reject or
  rebase a stale intent (optimistic concurrency).

Because tokens are the *ordering flag's* machinery, they are **opt-in** (only
conditional intents take one) and **auto-elided under client-WASM** (in-process
ordering already guarantees freshness, so the token variant compiles to a no-op).
Portable code pays zero client-side tax. The token-carrying surface is designed
in "Versioned intents (opt-in)".

## `viewJs`'s rationale shifts by target

Server-side, `viewJs` is a **secret boundary**: the Model holds server state that
must not reach the browser. Client-side the Model is already in the browser, so
`viewJs` is an **encapsulation boundary**: host and third-party page scripts see
only the declared typed slice, not the internal Model shape — with a residual
secret role against *other* scripts sharing the page. Same mechanism, both
motivations hold; `viewJs` is not dead weight client-side.

## Versioned intents (opt-in)

To be designed: the surface of a token-carrying conditional intent — how an intent
opts into optimistic concurrency, how `update` sees and rebases a stale one, and
how the whole layer elides under client-WASM.

## Naming (open)

`jsInbound` / `sendToJs` / `viewJs` and the JS-side `ipe.send` / `onCommand` /
`onState` are placeholders; the naming axis is inconsistent (direction vs
view-variant vs verb+target) and under review.

## Relationship to Rust FFI

The port is a boundary to non-Ipê code, and so is the Rust FFI. The axis that
decides *whether something is a port* is not the target language (JS vs Rust) but
the **trust and execution model of the far side** — the same trust/ordering flags
above. Browser JS and capability-isolated Rust land on the port side; pure
in-process Rust lands on the direct-binding side.

A pure total Rust function — a parser, a hash — should surface as an ordinary Ipê
value (`let h = blake3 bytes`), not a `Cmd` round-trip: it has a trusted,
synchronous, same-address-space far side (gate OFF, ordered), so parse-don't-
validate argues *against* re-validating at runtime what was parsed at compile
time. Async Rust (a `Task` with an inbound completion) and Tier-2 sandboxed Rust
(a narrow declared capability, serialized across the jail, decoded because the
isolated side is only semi-trusted) are ports under a different name — see
[async-ffi-bridge-design.md](async-ffi-bridge-design.md).

## Alternatives considered

- **Custom elements.** JS widgets encapsulated as DOM nodes with typed attributes
  and events, composing through the existing DOM-patch channel. Complementary, not
  competing: the right tool for a *visual* widget embedded in the view, whereas
  ports are for imperative effects and out-of-band data. A later design can add
  them on top.
- **Hooks / lifecycle callbacks** (arbitrary JS on node mount/update). Rejected as
  the primary boundary: the callback body is untyped JS with ambient DOM access —
  the unbounded surface of the forbidden eval seam, only spelled differently.

## Boundaries

- Design-only; nothing here is implemented.
- Applies to both browser execution models — server-driven Web **and** client-WASM
  — via the one surface + two transports. WebView (its own host bridge) and the
  Terminal shape (no JS) are out of scope.
- The JS-side runtime surface is a small fixed API — an intent sender, a
  projection subscription, and a command handler registry — never `eval` /
  `new Function`. It replaces the `data-ipe-eval` seam, which this design removes.
- Capability inference: a program that declares a JS port is exercising the
  browser-scripting capability and is classified accordingly.
- Where a stable browser API (Geolocation, storage, WebAudio) should instead be a
  first-class typed module rather than a hand-written port is an open question —
  see "kernel vs package" discussion.
