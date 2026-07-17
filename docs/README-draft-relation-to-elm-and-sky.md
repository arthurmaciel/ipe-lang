<!--
  DRAFT — pre-1.0 groundwork. NOT the real README.

  This is a staging draft of the README section "How ipê relates to Elm and
  Sky", assembled from the committed divergence ledgers
  (docs/divergences-from-elm.md, docs/divergences-from-sky.md) and their
  guardian synthesis (docs/divergences-review.md). It is intended to be lifted
  into the real README ONCE the compiler port is complete and the tracked
  convergence items have been re-checked. Revise every "[DRAFT — confirm before
  publish]" marker before publication, and re-run the divergence review so no
  in-progress item is stated as a shipped strength.

  Public-artifact voice rule (enforced throughout): state what differs and the
  technical rationale. Never characterize Elm or Sky as buggy, wrong, outdated,
  or limited. No "contribute upstream" notes.
-->

# How ipê relates to Elm and Sky

> **Status: pre-1.0 draft.** This section is groundwork written while the
> compiler is still being brought to full parity. Claims below are drawn from
> the project's internal divergence ledgers and are worded to be defensible and
> non-overclaiming; a final pass will confirm each before publication.

## Positioning

ipê is an **Elm-family** functional language: it shares Elm's core syntax
(`let…in`, `case…of`, `|>`/`<|` pipelines, `::` cons, `\x -> …` lambdas,
`{ r | f = v }` record update, `type` / `type alias`, `module … exposing (…)`),
Elm's Hindley–Milner type discipline, and The Elm Architecture — `init` /
`update` / `view` / `subscriptions` over `Model` / `Msg` / `Cmd` / `Sub`. It was
ported from **Sky**, a compiler in the same family, and it keeps Go / behavioral
parity with that reference as its **default contract** — ideally byte-for-byte
for the same well-typed program and input. What sets ipê apart from both is its
target: instead of compiling to JavaScript for a browser sandbox, ipê compiles
to **typed Go and Rust**, producing server, native CLI/TUI, and desktop binaries
(with WASM planned). Most of the deliberate, recorded departures below follow
directly from that one shift, or from choosing a stricter guarantee where a
different target makes one available.

---

## Relationship to Elm

ipê inherits Elm's syntax, HM type system, and TEA wholesale. The departures are
concentrated in the effect model, the error model, and the standard library —
all consequences of targeting a real process rather than a browser client.

### The effect model: Task-everywhere

Elm's effect model is managed and sandboxed: user code performs a side effect by
returning a `Cmd msg`, and the Elm runtime (inside the browser) performs it. In
`elm/core` a `Task` is inert until converted to a `Cmd`. ipê inverts this into
**Task-everywhere**: *every observable side effect returns `Task Error a`*, and a
`Task` is a first-class effect that runs directly at the program's entry
boundary (`main = Task.run …`).

- **A directly-runnable effect stdlib.** `File.*`, `Http.*`, `Db.*`,
  `Process.run`, `Io.*`, `System.*`, `Crypto.{randomBytes, randomToken}`,
  `Time.{now, sleep}`, `Random.*`, `Log.*` are all `Task Error a`. This is
  natural on a server/native runtime where the filesystem, sockets, DB handles,
  and subprocesses are ordinary capabilities.
- **A four-tier effect taxonomy.** Every binding is classified in its type:
  **Pure** (bare `a`), **Fallible-pure** (`Result e a` / `Maybe a`), **Effects**
  (`Task Error a`), and **Diverging** (`Int -> a`, i.e. `System.exit`). The type
  says whether a call can fail, must be sequenced as an effect, or never
  returns — "parse, don't validate" applied at the signature.
- **Result/Task bridges as a named surface.** `Task.fromResult`,
  `Task.andThenResult`, `Result.andThenTask`, `Task.mapError`, `Task.onError`,
  plus `RetryPolicy` / `ShouldRetry` retry combinators, codify the "keep effects
  in Task; the entry boundary executes them" discipline as an API.
- **Server-side pub/sub and command execution.** `Cmd.perform`,
  `Cmd.publish` / `Sub.subscribeTopic` drive async effects by dispatching a
  `Msg` back through the loop — structurally like Elm's `Cmd`, but the loop and
  broker live on the host process, enabling cross-session fan-out a client
  runtime does not provide.

### Typed errors

ipê mandates `Result Error a` / `Task Error a` with a single canonical typed
`Error` — a non-regression rule enforced by the test suite, not a style guide.
`Result String a` / `Task String a` are not used in public surfaces. A uniform
typed `Error` gives structured classification, correlation IDs, and structured
logging across the whole effect stdlib; the idiomatic two-level pattern
(correlation id + server-side structured log + user-facing message) keeps
internal detail out of user-facing errors.

### A server/native/desktop stdlib with no Elm-core counterpart

Because the target is a real process, ipê ships a large native standard library
that Elm's browser-sandbox core does not include — persistence (`Std.Db`,
`Std.Csv`, `Std.Config`, `Std.Compression`), security (`Std.Auth`,
`Sky.Core.Crypto`, `Sky.Core.Jwt`, `Sky.Core.Encoding`, `Sky.Core.Bytes`),
money/precision (`Std.Decimal`, `Std.Money`), servers and sockets
(`Sky.Http.Server`, WebSocket, SSE streaming), observability (`Std.Log`,
`Std.Trace`, metrics, OTLP export), and runtime services (`Std.Cache`,
`Std.Email`, `Std.Time`, `Sky.Core.File`/`Io`/`System`/`Process`). These are
additions in surface area, each justified by the server/native target.

**One pure `view`, three backends.** The same `Std.Ui` `view` renders to
server-driven HTML with SSE patches (`Sky.Live`), ANSI terminal cells
(`Sky.Tui`), and a native desktop window (`Sky.Webview`). `Std.Ui` is
elm-ui-derived but renders inline-styled HTML on the server, and it adds a typed
surface elm-ui does not cover — pseudo-classes, media queries with a typed
`Breakpoint` ADT, CSS transitions and keyframe animations, and a CSS-grid track
ADT — all while keeping the "never write CSS" contract.

### Syntax additions

Most surface syntax is identical to Elm. The additions:

- **`Ffi.kernel` declarations + auto crate-binding.** An HM-typed binding whose
  body routes to a native kernel, plus automatic binding of Go/Rust crates.
  Elm's only interop is ports; ipê needs typed native binding at SDK scale (up
  to ~76k symbols).
- **String interpolation in triple-quoted strings.** `"""… {{expr}} …"""`
  interpolates identifiers, field access, qualified names, and calls; `\{{`
  escapes a literal `{{`. Elm has triple-quoted strings but concatenates with
  `++`. The escape rule lets Mustache/Handlebars payloads pass through
  untouched.

### Constraints shared with Elm (not ipê-specific)

To avoid overclaiming, these match Elm 0.19.x and are **not** ipê inventions or
ipê-only limits:

- **No higher-kinded types** — both are Hindley–Milner / rank-1.
- **No user-defined operators** — Elm 0.19 disallows them in application code;
  ipê likewise.
- **No `where` clauses** — use `let…in` in both.
- **Negative-literal arguments need parens** — `f (-1)` in both.
- **Exhaustive `case…of`**, extensible record annotations and record update, and
  the core prelude names (`Ok`/`Err`, `Just`/`Nothing`, `identity`, `always`,
  `not`, `fst`, `snd`, `clamp`, `modBy`) all match Elm.

Some Elm modules are not ported: `Array`, `Bitwise`, the `Tuple` module
(`fst`/`snd` live in `Basics`), and `Debug`; browser-only modules and the
port/`Platform` model have no counterpart, replaced by FFI + Task + the server
runtimes. `List`/`Dict` cover the common container need. [DRAFT — confirm before
publish: do NOT imply `Array`/`Bitwise` parity with Elm; state them plainly as
not-yet-ported so the copy neither overclaims coverage nor characterizes Elm.]

---

## Relationship to Sky

ipê is a Rust port of Sky. Sky — the Haskell compiler and its Go and Rust
backends — is the parity and capability reference ipê was ported from, and
**Go / behavioral parity is ipê's default contract**: for the same well-typed
program and input, ipê aims to match, ideally byte-for-byte. Every place ipê
nonetheless differs is recorded in-repo and pinned by the oracle test framework
(each carries a marker and a tagged reason), so divergences are non-silent by
construction. Where a difference exists it is because ipê targets a stricter
guarantee, a different substrate, or Go-conformance in a spot where a secondary
fork drifted. The deliberate ones, framed as strengths where that is genuinely
true:

### Compiler structure: total-by-construction, typed IR

- **A typed IR checkpoint.** ipê lowers `canon → typed IR → Rust` in two stages,
  so a malformed shape is unrepresentable in the IR rather than surfacing only at
  `rustc` — "make invalid states unrepresentable" moved earlier in the pipeline.
- **A closed, fail-closed kernel registry.** Kernel dispatch goes through a
  closed typed `KernelFn` enum indexed anti-drift from a single registry; an
  unknown kernel fails **closed** with a diagnostic (`IPE-L0108`). This
  structurally eliminates the "compiler exits 0, the emitted code then fails to
  build" class.
- **Total type rendering.** The type renderer is a closed function with no
  catch-all default — total by construction.
- **Typed tail-call loops.** Tail recursion is a typed IR node emitted as a Rust
  `loop`, giving constant stack rather than relying on host tail-call behavior.
- **Compiler-checked drift.** Crate name+version live in a typed `const` table
  read by every manifest emitter, with a co-located drift test; kernel-registry
  tripwires keep call-site resolution and the registry in lockstep.

### Correctness: Go-parity by default, more correct where a fork drifted

- **Rune-correct strings.** Every `String` length / index / slice operation
  counts Unicode code points (runes) uniformly across the module, so an
  astral-plane character counts as length 1 and is never split. (The unit is
  code points, not grapheme clusters — see the do-not-overclaim note below.)
- **Full-Unicode case mapping.** `toUpper`/`toLower`/`casefold` apply full
  Unicode `SpecialCasing` (e.g. `ß → SS`), aligned with mainstream
  Rust/Python/Swift; ASCII case is identical to the reference.
- **Money splits that sum to their input.** `Money.allocate` distributes the
  residue by sign so the shares sum to the exact input for negative totals as
  well as positive. Positive-total behavior is byte-identical to the reference.
- **Comparable `min`/`max`.** `Math.min`/`Math.max` compare at the argument type
  (Elm's polymorphic `comparable`), so `Math.min 0.4 1.3 = 0.4` and string
  comparison is meaningful.
- **Stricter numeric parsing.** `String.toFloat` accepts the standard float
  grammar — "parse, don't validate" at the numeric boundary.
- **Lossless typed `Bytes`.** `Bytes` is a distinct `Vec<u8>` primitive rather
  than a `String` alias, so a non-UTF-8 binary payload is representable and
  `String ↔ Bytes` conversion is always explicit. This makes "non-UTF-8 hidden
  in a String" an unrepresentable state.
- **Richer ADT shapes.** Recursive enums carried through tuple or record
  payloads, and certain `Set` shapes, compile and run on ipê.
- **Go `%v` float formatting, confirmed against Go 1.26.2.** ipê's float
  stringification switches to scientific notation at the same decimal exponent
  as Go's `%v` (≥ 6, and < −4), confirmed by a direct probe of Go 1.26.2 and
  pinned by regression tests. Framed precisely: **ipê matches Go byte-for-byte;
  a secondary Rust fork of the reference uses a different threshold here.**

### Security hardening in the runtime fork

ipê's vendored runtime is a strict superset of the shared baseline — every
module carries the reference's logic plus additional fail-closed hardening. Each
of these either matches Go or is more conservative:

- **Auth fail-closes** on an id-column decode error rather than defaulting to a
  privileged identity.
- **JWT** rejects the `now == exp` boundary (Go-parity, RFC-aligned) and treats
  `exp`/`nbf` as optional.
- **HTTP / WebSocket / stream** redact URL userinfo and query in error messages,
  default SSRF-deny in production, and return an error on an invalid HTTP method
  rather than silently downgrading it.
- **Telemetry** strips CRLF from CSP frame-ancestors and scrubs control
  characters (including U+2028/U+2029) from logs.
- **Decimal/money** rounding, division-precision cap, and `allocate` use
  Go-parity, saturating arithmetic; counters saturate rather than overflow.
- **Env access** is routed through a process-global lock, closing an env
  data-race.

On the rendering side, ipê has no client-side eval sink, HTML-escapes
everything, and renders CSP-strict by default — an XSS/CSP concern specific to
emitting server HTML, which Elm's client model does not face.

### Substrate differences (stated neutrally, no claim)

Some differences are simply consequences of emitting Rust rather than Go:
`Std.Db` runs on `sqlx` rather than cgo/SQLite, and `Std.Ui` currently emits a
compact inline-CSS HTML skeleton that differs byte-wise from the Go renderer
while producing semantically-equivalent layouts. Byte-parity for HTML is a later
goal. These are recorded as differences, not strengths.

---

## What's still converging

The following are tracked, filed, and sequenced convergence items — not open
questions and not shipped features. They are called out here for honesty and
will be resolved before or shortly after the port is declared complete:

- A **front-end pattern-completeness workstream** — nested-constructor-payload
  patterns (`Just (h :: t)`, `Ok {name}`), refutable function-argument patterns,
  the bare `.field` accessor-as-function, and continuation-inside-a-type-body —
  currently handled fail-closed (no panic from well-typed code), converging via a
  shared desugar.
- The **`Sky.Core.Jwt` builder API** — the codec emits token bytes identical to
  Go today; the builder-style call surface (`Algorithm`/`Claims`) is the tracked
  follow-up.
- **`Std.Ui` HTML byte-parity** — semantically correct now, byte-identical
  later.
- A small number of **stdlib text-path and codegen items** are in progress and
  deliberately not described here; they are being fixed, not shipped. [DRAFT —
  confirm before publish that all in-progress items remain out of the public
  copy until resolved.]

<!--
  Deliberately EXCLUDED from this draft per the divergence review's
  "keep out of README until fixed" list (both are being fixed, not shipped):
    * B3 — Encoding.* Latin-1 char-as-byte over the TEXT path (silent truncation
      for code points ≥ 0x80). Filed #55. Referenced above only as an
      unspecified "text-path item in progress", never as a feature/divergence.
    * B7 / R3 — bare arity-0 Uuid.v4/v7 typed as the PURE tier (entropy in a
      pure signature). Filed #54. Omitted entirely; no UUID-generation claim is
      made. The related B8 (Uuid.parse) is also omitted, since the review flags
      it as pending-verify and possibly the same arity-0 codegen artifact — do
      not surface it as a standalone strength until confirmed.

  Do-not-overclaim guardrails applied:
    * "rune-correct", never "grapheme-correct".
    * No Array/Bitwise parity implication with Elm.
    * Every "matches Go / follows Elm-conformance" claim is framed as ipê's
      conformance, never as the reference being broken.
-->
