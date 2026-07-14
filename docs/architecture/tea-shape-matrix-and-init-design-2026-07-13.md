# TEA as a state engine, the app-shape matrix, and the `init` signature (2026-07-13)

**Status:** DECIDED for #180 (prescriptive `init`); the ocap direction is FILED
as a post-parity exploration, not adopted now.
**Context:** design discussion while burning down the examples-sweep blockers;
triggered by #180 (`26-ui-showcase`'s `init : {}` vs the Rust `Live.app`
scheme's required `LiveReq`). Companion to
`reference-cross-reference-sweep-blockers-2026-07-13.md`.

---

## 1. The principle: TEA is a *state engine*; `view` is an optional projection

"TEA drives every program" is **not** a sound assumption, and Ipê/Sky does not
actually make it — CLI runs on `Task.run` (a pipeline) and `Http.Server` on
`listen [routes]` (a router); neither is TEA. The sound, narrower principle to
carry:

> **A single `Model`, evolved by a pure `update` over typed `Msg`s, with every
> effect and external event reified as data (`Cmd`/`Sub`), is the right engine
> for *stateful reactive* programs. `view` is an *optional projection* of the
> Model — not part of TEA's definition.**

Elm couldn't see this (it is UI-only); Ipê spanning terminal/desktop/server
can. Independently confirmed by `elm-run` (§6): *"TEA is a contract that says
your code never performs effects, it describes them… the browser was an
implementation detail the whole time."*

Design consequence: build the reactive cfg so **`view` is a field of a
view-bearing shape, not baked into the core engine** — so a future headless
reactive shape is a clean subtraction, not a redesign.

## 2. The app-shape matrix (two orthogonal axes)

|                                   | **has a view (UI projection)**        | **headless (no view)**                          |
|-----------------------------------|---------------------------------------|-------------------------------------------------|
| **reactive** (long-lived Model+Msg loop) | Sky.Live, Sky.Tui, Sky.Webview  | *(worker/daemon — not a shape today; opt-in later)* |
| **one-shot / stateless-concurrent**      | —                               | Sky.Cli (`Task.run`), Sky.Http.Server (router)  |

- `init` belongs to the **reactive row** — its job is to *produce the initial
  `(Model, Cmd)`* (seed state + startup effects), regardless of its argument.
- `view` belongs to the **view column**.
- Non-reactive shapes are simply not on the TEA plane; do not force `init` onto
  them.

## 3. `init` signature — DECISION (prescriptive)

Chosen over the reference's permissive free-type-var approach (see §4).

| Shape            | TEA `init`? | Signature                              | Rationale |
|------------------|-------------|----------------------------------------|-----------|
| **Sky.Live**     | yes         | `init : LiveReq -> (Model, Cmd Msg)` **(mandatory)** | Per-session request context, not ambient. Elm-`Browser.application`-faithful (always receives the routing/URL context). |
| **Sky.Tui**      | yes         | `init : () -> (Model, Cmd Msg)`        | No non-ambient per-invocation context. Terminal size via a resize `Sub` (a one-shot init snapshot goes stale on resize). |
| **Sky.Webview**  | yes         | `init : () -> (Model, Cmd Msg)`        | Same. Window size is the `window` cfg field (set, not received). |
| **Sky.Cli**      | **no**      | `main = Task.run …`                    | One-shot; async handled by `Task` chaining. Input via `System.*`. |
| **Sky.Http.Server** | **no**   | `main = … Server.listen [routes] …`    | Per-*request* context goes to *handlers* (`Request`); startup config built in `main` via `System.*`. |

`init` stays on Tui/Webview because its **value is its return** (initial Model +
startup Cmd), not its argument — Elm's `sandbox` (`init : Model`, no arg) proves
this. `init : ()` is preferred over any "mirror the Model"/flags-shaped arg.

## 4. The effects-authority rule (why `init`'s arg is `LiveReq` or nothing)

> **`init`'s argument carries ONLY context that is (a) specific to this init
> invocation AND (b) not reachable through the ambient `System`/effects stdlib.
> All ambient input — env vars, CLI args, cwd — is accessed via `System.*` from
> anywhere, never threaded through `init`.**

The only thing that earns a place in `init`'s argument is genuine
per-invocation context with no ambient accessor — i.e. `LiveReq` (there is no
`System.currentRequest`; a session is born from one specific HTTP request).

**Justified divergence from Elm:** Elm needs `flags` as an init arg *only*
because a browser sandboxes JS — the sole external-data channel is JS→Elm
flags. Ipê runs natively with a real `System` API, so **`flags`-as-an-init-arg
is redundant for us.** We keep Elm's structure (TEA) but drop `flags`.

**Why prescriptive over the reference's permissive free-tvar:** the reference
(`../sky/src/Sky/Type/Constrain/Expression.hs:2674`) leaves `init`'s arg a free
`req` tvar for a *Go-runtime reason* — keeping the untyped `map[string]any` req
compatible with any inferred shape. Ipê has a **typed `LiveReq`**, so that
rationale doesn't transfer; being prescriptive is both more Elm-faithful and
make-invalid-states-unrepresentable.

## 5. Consequence: the example-patch-curation workstream

Prescriptive Ipê decisions mean the upstream Sky examples need Ipê-specific
patches. #180's fix is the FIRST such patch:

- **Patch #1:** `26-ui-showcase/src/Main.sky` — `init : {} ->` → `init : LiveReq ->`.
- Track these as an explicit, reviewable patch layer (`ipe-example-patches/` or
  per-example notes) so the Sky examples stay upstream-faithful while Ipê
  carries its divergences openly. Same layer will hold the Go-FFI→Rust-FFI
  conversions (e.g. `13-skyshop` Go-FFI-only → Rust-FFI).

## 6. Cross-reference: `elm-run` / native Elm (https://cekrem.github.io/posts/native-elm/)

Runs the **unmodified Elm compiler** with a swapped runtime (OS kernel + libcurl,
real ELF/Mach-O/PE binary, no Node/JS). Runtime selected by `main`'s type
(`Cli.Program` → CLI host; `Worker.Program` → HTTP/2+TLS server host).

**Validates our positions:** TEA-is-a-state-engine, `view`-optional,
runtime-by-`main`-type, and headless-TEA is real (its CLI is genuine
`init`/`update`/`subscriptions`, no `view`; async one-shot is naturally
message-driven: fetch → `GotResponse` → exit).

**Challenges one — the effects-authority question.** `elm-run` uses ocap:
- No `Flags`; the OS provides `Env` (argv/env/fds) as **`init : Env -> (Model, Cmd Msg)`**.
- **No ambient authority** — `Env`/capabilities are handed to `init` only and
  threaded explicitly; effects gated by capabilities in the type + runtime
  grants (`--allow-http`; ungranted no-ops). "Deno permission model reached
  through Elm's type system."
- **Lifecycle-as-type:** `Model = Done Int | Running { env }` — `env` exists only
  in `Running`, so a finished program *structurally cannot* fire effects.

| | Pros | Cons |
|---|---|---|
| **ocap `Env`-through-`init`** | Effect authority is a type-checked threaded value; a fn without a capability *cannot* perform that effect (make-invalid-states-unrepresentable for effects themselves). Stronger than anything Sky/Ipê has. | Verbose (thread `Env` everywhere) vs ambient `System.getenv`. Kills Sky's ambient `System.*` — a language-wide breaking redesign. |
| **native-TEA-everywhere (incl. CLI)** | Uniform model; async one-shots are naturally message-driven. | Heavier than monadic `Task.run` for trivial scripts. |
| **the tool itself** | Proves the thesis end-to-end. | v0.2.0, single-author; unsigned bytecode, FS sandbox escapes on spawn, "host is a kludge"; reimplements runtime+optimizer+native backend. |

## 7. Decisions / filed items

- **ADOPT NOW:** the §1 principle + §2 matrix + §3 prescriptive `init` (#180) +
  §4 effects-authority rule. Ship #180 as prescriptive; keep ambient `System.*`.
- **ADOPT NOW (cheap, high-value):** lifecycle-as-type (states that structurally
  can't fire effects, e.g. `Done | Running {…}`) in stdlib app scaffolding /
  the future headless-worker shape.
- **FILE FOR LATER (post-parity):** **capabilities-as-threaded-values (ocap
  `Env`)** as the principled long-term answer to "should effects be ambient?".
  Do NOT reopen #180 for it — it is a language-wide redesign, exactly the kind of
  "principled strategy adopted post-parity" the roadmap anticipates.
- **KEEP THE DOOR OPEN (YAGNI, no code now):** the empty matrix cell —
  a headless reactive "worker"/daemon shape (TEA minus `view`) — by keeping
  `view` an optional field of view-bearing shapes rather than core to the engine.
