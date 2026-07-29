# Application-shapes model — consolidation design

> Working spec. Once implemented, the *decision* is captured in an ADR and this
> file is deleted (per the project's tbd/ → ADR convention).

## Goal

Settle the public shape model into its final form: **four shapes**, each with a
small, consistent set of entry points whose names describe *what the view
produces* and *how the terminal/screen is driven* — no jargon, no redundant
qualifiers, no one-off entry per escape hatch.

## Current state (grounding)

Shapes are selected by the entry kernel `main` is bound to (canon infers the
shape; nothing in the manifest selects it). Today there are five shapes and
these entry kernels (`src/compiler/kernels/src/lib.rs`, registered in
`src/compiler/canon/src/env.rs`, typed in `src/compiler/types/src/constrain.rs`,
lowered in `src/compiler/lower/src/lower.rs`, emitted per-shape under
`src/compiler/backend/rust/src/emit_*.rs`):

| Module (`Ipe.Tea.*`) | Entries | View type | Notes |
|---|---|---|---|
| `Web` | `app`, `appHtml`, `appRouted`, `renderStatic` | `Element` / `Html` | `app`=Element, `appHtml`=raw-Html escape |
| `WebView` | `app`, `appHtml` | `Element` / `Html` | same split |
| `Tui` | `app`, `program` | `Element` / `String` | `app`=Element+`onKey`; `program`=raw String+`onKey` |
| `Console` | `app` | `String` | line-oriented, `onLine` |
| `Program` | plain `main` | — | no TEA loop |

Two problems:

1. **`Tui` and `Console` are the same shape** — a managed TEA loop over a
   terminal. They differ only in *how the terminal is driven*: `Tui` addresses
   the whole screen (alternate screen, `onKey`); `Console` streams lines
   (scrollback, `onLine`). Two modules for one medium is noise.
2. **`Tui.program` is an escape as an entry point.** A full-screen *raw String*
   view is the "drive the cells yourself" escape. Escapes belong *inside* a view
   as nodes (like `Web`'s raw-Html), not as a parallel shape entry.

## Target model

**Four shapes: `Web`, `WebView`, `Terminal`, `Program`.**

`Tui` + `Console` collapse into one `Ipe.Tea.Terminal` module with two entries,
named for the return type / drive axis, symmetric with the `app`/`appHtml`
verb family on `Web`/`WebView`:

| Module | Entry | View type | Input | Drives |
|---|---|---|---|---|
| `Web` | `app` | `Model -> Element Msg` | events | server DOM |
| `Web` | `appHtml` | `Model -> Html Msg` | events | server DOM (raw-Html escape) |
| `WebView` | `app` | `Model -> Element Msg` | events | system webview |
| `WebView` | `appHtml` | `Model -> Html Msg` | events | system webview (raw escape) |
| **`Terminal`** | **`appScreen`** | `Model -> Element Msg` | `onKey` | full screen (alternate screen) — was `Tui.app` |
| **`Terminal`** | **`appLines`** | `Model -> String` | `onLine` | line stream (scrollback) — was `Console.app` |
| `Program` | plain `main` | — | — | one-shot / server, no TEA |

### Naming rationale

- `Terminal.appScreen` / `Terminal.appLines` — both key off the **drive axis**
  (screen-addressed vs line-streamed), both keep the `app*` verb prefix shared
  with `Web.app`/`WebView.app` (so "`app*` = a TEA entry" reads uniformly across
  every shape), and neither carries the redundant "Tui" (which expands to
  *Terminal* User Interface inside a module already named `Terminal`) nor the
  misleading "cli" (a CLI is a one-shot arg→exit tool — that is the `Program`
  shape — not an interactive `onLine` loop, and it would collide with the
  `Ipe.Cli` arg-parsing module).

### Unified `Element`, no medium tag

`Web.app`, `WebView.app`, and `Terminal.appScreen` all take the **same**
`Model -> Element Msg`. `Element` is **not** phantom-tagged by medium (decided:
no `Element medium msg`). The shape entry alone fixes which escape nodes are
admissible; a view that embeds a terminal-only node under `Web.app` is rejected
by that entry's constraint/seal, not by a type-level medium parameter the user
must write or read. This keeps portable view code medium-agnostic and the type
surface small.

### `Tui.program`'s role → escape nodes

The full-screen raw view (`Tui.program`) is dropped as an entry. Its capability
— "paint the terminal yourself" — returns as an **escape node inside an
`appScreen` `Element` view**:

- `Ui.cells` — a raw terminal cell-grid node (the terminal analogue of `Web`'s
  raw-Html escape).
- `Ui.raw` — a raw pass-through node.

and the cross-medium conversions discussed:

- `Ui.fromHtml : Html msg -> Element msg` (lift raw Html into a portable view;
  lives in `Ui`, the result module).
- `Html.fromUi : Element msg -> Html msg` (lower a portable view to Html; lives
  in `Html`, the result module — `fromX` lives where its result type lives).

These are **net-new nodes/kernels**, delivered as a **separate, later change**;
the consolidation below does not depend on them.

## Work breakdown

**The consolidation (this spec's deliverable).** Rename `Tui`+`Console` →
`Terminal`; `Tui.app`→`Terminal.appScreen`, `Console.app`→`Terminal.appLines`.
Handle `Tui.program`: migrate any in-tree user to `appScreen`; prefer dropping
the entry, since `Ui.cells` is its real home. Emitted output is per-shape; the
two existing emitters (`emit_tui.rs`→appScreen, `emit_console.rs`→appLines) are
kept and only re-pointed. No new runtime behavior.

**Escape + conversion nodes (deferred follow-up).** Add `Ui.cells`, `Ui.raw`,
`Ui.fromHtml`, `Html.fromUi`. Larger, independent, and gated on its own design;
not required for the shape model to be correct.

## Non-goals

- No change to `Web`/`WebView`/`Program` entry names or behavior.
- No auto-import / DCE changes (separate roadmap item).
- No medium-tagged `Element`.
