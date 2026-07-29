# Application-shapes model — consolidation design

> Working spec. Once implemented, the *decision* is captured in an ADR and this
> file is deleted (per the project's tbd/ → ADR convention).

## Goal

Settle the public shape model into its final form: **four shapes**, each with a
**single** structured entry, and every escape hatch expressed as a **node inside
the view** rather than a parallel entry point.

## Current state (grounding)

Shapes are selected by the entry kernel `main` is bound to (canon infers the
shape). Today there are five shapes and these entry kernels
(`src/compiler/kernels/src/lib.rs`, registered in `src/compiler/canon/src/env.rs`,
typed in `src/compiler/types/src/constrain.rs`, lowered in
`src/compiler/lower/src/lower.rs`, emitted per-shape under
`src/compiler/backend/rust/src/emit_*.rs`):

| Module (`Ipe.Tea.*`) | Entries | View type |
|---|---|---|
| `Web` | `app`, `appHtml`, `appRouted`, `renderStatic` | `Element` / `Html` |
| `WebView` | `app`, `appHtml` | `Element` / `Html` |
| `Tui` | `app`, `program` | `Element` / `String` |
| `Console` | `app` | `String` |
| `Program` | plain `main` | — |

The raw-`Html` escape node `Ui.html : Html msg -> Element msg` already exists
(`UiHtml` kernel). Two redundancies:

1. **`appHtml` is an escape as an entry point.** A raw-`Html` view duplicates
   what `Ui.html` already does *inside* an `Element` view — so the second entry
   is unnecessary.
2. **`Tui` and `Console` are one shape** — a managed TEA loop over a terminal,
   differing only in drive (screen vs line). And `Tui.program` (full-screen raw
   `String`) is likewise an escape-as-entry.

## Target model

**Four shapes: `Web`, `WebView`, `Terminal`, `Program`. One structured entry
each (plus `Web`'s routing/static entries). Escapes are nodes.**

| Module | Entry | View type | Input | Notes |
|---|---|---|---|---|
| `Web` | `app` | `Model -> Element Msg` | events | `appRouted`, `renderStatic` unchanged |
| `WebView` | `app` | `Model -> Element Msg` | events | |
| **`Terminal`** | **`appScreen`** | `Model -> Element Msg` | `onKey` | was `Tui.app` — full screen |
| **`Terminal`** | **`appLines`** | `Model -> String` | `onLine` | was `Console.app` — line stream / REPL |
| `Program` | plain `main` | — | — | one-shot / server, no TEA |

### What is removed, and where its capability goes

- **`Web.appHtml`, `WebView.appHtml`** — removed. Raw HTML is reached through the
  **existing `Ui.html`** node inside the single `Element` view.
- **`Tui.program`** (full-screen raw `String`) — removed. Its "paint the cells
  yourself" capability returns as a **new `Ui.cells` node** inside an `appScreen`
  `Element` view (the terminal analogue of `Ui.html`).
- **`Console`** the module — folded into `Terminal.appLines`; the line-oriented
  `onLine` REPL loop is preserved verbatim, just under the new name.

### Naming rationale

`Terminal.appScreen` / `Terminal.appLines` key off the **drive axis** (screen-
addressed vs line-streamed), keep the `app*` prefix shared with `Web.app`/
`WebView.app` (so "`app*` = a TEA entry" reads uniformly), and avoid both the
redundant "Tui" (Terminal User Interface, inside a module already named
`Terminal`) and the misleading "cli" (a CLI is a one-shot arg→exit tool — the
`Program` shape — not an interactive `onLine` loop, and it collides with the
`Ipe.Cli` module).

### Unified `Element`, no medium tag

`Web.app`, `WebView.app`, and `Terminal.appScreen` all take the same
`Model -> Element Msg`. `Element` is **not** phantom-tagged by medium. The shape
entry alone fixes which escape nodes are admissible: `Ui.html` under a web/
webview entry, `Ui.cells` under `Terminal.appScreen`. A view that embeds a
terminal-only `Ui.cells` under `Web.app` (or a web-only `Ui.html` under
`appScreen`) is rejected by that entry's seal, not by a type-level medium
parameter the user must write or read. Portable views stay medium-agnostic and
the type surface stays small.

### `Ui.cells` (new)

The raw terminal cell-grid escape, mirroring `Ui.html`. Proposed first signature
(illustrative — this function does not exist yet; it is delivered by this work):

```
Ui.cells : List (List Char) -> Element msg
```

a fixed grid of characters painted into the terminal by `appScreen`'s emitter. A
richer per-cell style/color type is a later refinement; the char grid restores
`Tui.program`'s core capability without a new record type. `Ui.cells` is
terminal-only: admissible under `Terminal.appScreen`, rejected under `Web.app`/
`WebView.app`.

## Work breakdown

**The consolidation.** Remove `appHtml` entries; collapse `Tui`+`Console` →
`Terminal` (`appScreen`/`appLines`); drop `Tui.program`. Emitted output is
per-shape; the existing emitters (`emit_tui.rs`→`appScreen`, `emit_console.rs`→
`appLines`) are re-pointed, not rewritten. `Tui.program`'s only in-tree users are
the `examples/sky/**` reference mirrors, which are untouched.

**`Ui.cells`.** Add the new node + its terminal-only admissibility. Ordered
after the consolidation so the shape model lands as a coherent unit even if the
node's signature iterates.

## Non-goals

- No change to `Web.appRouted` / `Web.renderStatic` / `Program`.
- No auto-import / DCE changes.
- No medium-tagged `Element`.
- No per-cell color type for `Ui.cells` yet (char grid first).
