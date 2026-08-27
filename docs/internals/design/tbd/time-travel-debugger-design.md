# Time-travelling debugger for TEA apps

> All three staged delivery shapes — WASM (a), server-driven (b), and Terminal
> (c) — are implemented. The design below documents the settled shape and
> decisions common to all shapes.

A dev-only debugger that records a TEA run and lets a developer scrub, inspect, and
replay past `Model` states. Feasible because TEA state changes only through
`update : Msg -> Model -> (Model, Cmd Msg)`, so the whole run is a fold over a
message log — recording the log reconstructs any past state exactly.

## Settled shape

- **Data model — message log + re-fold.** Store the `Msg` log plus a base `Model`;
  the `Model` at step N is `update` re-applied over the retained messages from the
  base, discarding the `Cmd`s. No per-step `Model` snapshots. The in-memory record
  *is* the exportable artifact (one representation). Relies only on `update` being
  pure and deterministic, which TEA guarantees.
- **Effects.** `Cmd`s fire on the live forward pass only; scrub/re-fold **never**
  re-fires them (no re-issued HTTP/IO). Reconstruction applies `Model` transitions
  and drops the `Cmd`.
- **Enablement — the `--debugger` build flag.** `ipe build --debugger` /
  `ipe run --debugger` compiles the recorder (and overlay) into the single runtime
  loop. The flag exists only on the development commands — `ipe release` does not
  offer it, so the debugger can never ship. (Named `--debugger`, not `--debug`,
  because production-vs-development is the *command* — `build`/`run` vs `release` —
  not a flag.) This is the debug family's dev-only guarantee expressed the way that
  fits a whole-loop tool, distinct from the source-construct gate on `Debug.log`/
  `todo`/`explain`.

## Decisions (PRINCIPLES-weighted)

- **Bounded history (bounded-by-construction).** The log is a ring buffer with a
  cap — no input can grow it without bound. On overflow, drop the oldest message
  *and* roll the base `Model` forward by one step (`base = update(base, dropped).0`),
  so re-fold always starts from a valid base. The bound thus carries exactly one
  rolling checkpoint; memory is `1 Model + N Msgs`. The cap is configurable with a
  sane default (same discipline as the session-store working-set bound).
- **Export/import via the seal codec (Security > Completeness).** Export is offered
  **only when `Msg` is seal-legal** (encodable) — a typed availability, so
  "export an unencodable session" is unrepresentable rather than half-working; the
  compiler reports cleanly when `Msg` is not encodable, and **live time-travel still
  works** (in-memory values need no encoding). An imported session is untrusted
  input, so import runs through the **total, fail-closed** seal decoder (5MiB
  budget, depth 128, recursion_limit; drop on mismatch, no partial value) — never a
  bespoke parser.
- **Secrets never in the log surface.** If a `Msg`/`Model` carries a `Secret`, it
  is redacted in any rendered/exported form (the `Secret` redaction already holds
  for `Debug`/serialisation); a `Secret`-bearing `Msg` is not seal-legal, so export
  is unavailable for it (consistent with the port/custom-element seal).

## Staged delivery (each stage: design → implement → guardian)

- **(a) Client-WASM first.** The `wasm/mod.rs` TEA sink runs `update` in-process, so
  the shape-agnostic **core** (ring-buffer recorder, re-fold reconstruction,
  seal-codec export/import, memory bound) lands here together with the **overlay**:
  a message list + a scrubber; selecting step N re-renders the view at the
  reconstructed `Model` (feed it to the same in-process view — trivial in-process).
  Re-render at a past state does not re-fire effects.
- **(b) Server-driven (`Ipe.Web`).** Reuse the same core behind the server loop
  driver: record server-side, and the scrub re-drives the client render over the
  existing patch wire. No new codec.
- **(c) Terminal.** Reuse the core behind the terminal loop driver; a terminal-native
  overlay (message list + scrub) is the lightest surface.

  The terminal debugger lives in `src/runtime/rust/src/debugger/tui.rs` behind
  `#[cfg(all(feature = "debugger", not(target_arch = "wasm32")))]`. Both TUI
  drivers (`Terminal.appLines` / `tui_app` and `Terminal.appScreen` / `tui_app_ui`)
  are hooked. Key bindings (active only in a `--debugger` build):

  | Key        | Effect                                       |
  |------------|----------------------------------------------|
  | **Ctrl-T** | Toggle time-travel mode on / off             |
  | Ctrl-Left  | Step one message backward (time-travel mode) |
  | Ctrl-Right | Step one message forward  (time-travel mode) |

  In live mode a status line at the bottom of the frame shows `[DBG] recording —
  N steps`. In time-travel mode it shows `[DBG TT] step N/M …prev | CURRENT |
  next…` with truncated message labels. Labels pass through `IpeStringify::ipe_show`
  so any `Secret`-bearing field renders as `<redacted>`. Pressing Ctrl-T again
  returns to the live head without re-firing any `Cmd`.

The core (record / re-fold / export / bound) is written once in (a) and shared;
(b) and (c) add only a driver hook and an overlay-delivery path.

## Testing

- Core: record a run, reconstruct `Model` at each step by re-fold, assert it equals
  the live `Model`; a `Cmd`-carrying step reconstructs without re-firing the effect;
  the ring buffer caps memory and the rolling base keeps re-fold correct after
  overflow.
- Export/import: a seal-legal `Msg` round-trips; a non-encodable `Msg` disables
  export with a clean diagnostic while live scrub still works; a malformed imported
  session is dropped fail-closed (no panic, no partial value); a `Secret`-bearing
  value is redacted / not exported.
- Gate: a `--debugger` build records; a `release` build has no `--debugger` and no
  debugger code (it cannot ship).
- Per shape (a/b/c): the overlay scrubs and re-renders a past `Model` without
  re-firing effects.

## Out of scope

The value-inspection helper (`Debug.log`) ships; `Debug.todo`/`Debug.explain` are a
separate increment. This document covers only the debugger.
