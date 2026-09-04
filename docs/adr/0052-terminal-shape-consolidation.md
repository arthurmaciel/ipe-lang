Status: Superseded by 0069
Date: 2026-07-29

# 0052. Terminal shape consolidation — four shapes, escapes as nodes

## Context

An Ipê program's shape is chosen by the entry kernel `main` is bound to. The
shape surface had grown two kinds of redundancy:

1. **Escapes expressed as parallel entry points.** Alongside `Web.app` /
   `WebView.app` (which take `view : Model -> Element Msg` and apply `Ui.layout`
   internally), each web shape carried a second entry — `Web.appHtml` /
   `WebView.appHtml` — taking `view : Model -> Html Msg` for authoring the DOM
   directly. But an `Element` view can already embed raw HTML through the
   `Ui.html : Html msg -> Element msg` node. The whole-view raw-`Html` entry
   duplicated, at the entry-point level, a capability the view vocabulary
   already provided as a node.

2. **One terminal medium split across two modules.** `Ipe.Tea.Tui` (a
   full-screen keystroke UI) and `Ipe.Tea.Console` (a line-oriented stdin REPL)
   were separate shapes, yet both are a managed TEA loop over a terminal —
   differing only in the drive axis (screen-addressed versus line-streamed).
   `Ipe.Tea.Tui` additionally carried `Tui.program`, a full-screen entry taking
   `view : Model -> String` — again an escape (paint-the-frame-yourself)
   expressed as its own entry point rather than a node.

The result was five shapes and a surface where "escape hatch" and "shape entry"
were conflated.

## Decision

Reduce to **four shapes — `Web`, `WebView`, `Terminal`, `Program` — each with a
single structured entry (plus `Web`'s routing/static entries), and every escape
expressed as a node inside the view rather than a parallel entry point.**

- **Remove `Web.appHtml` and `WebView.appHtml`.** Raw HTML is reached through the
  existing `Ui.html` node inside the one `Element` view. Rejected alternative:
  keep the whole-view raw-`Html` entries for convenience — rejected because it
  keeps two ways to say one thing and forces every reader to learn which entry a
  view type demands.

- **Collapse `Ipe.Tea.Tui` and `Ipe.Tea.Console` into `Ipe.Tea.Terminal`** with
  two entries keyed on the drive axis: `Terminal.appScreen`
  (`view : Model -> Element Msg`, driven by `onKey` — the former `Tui.app`) and
  `Terminal.appLines` (`view : Model -> String`, driven by `onLine` — the former
  `Console.app`, the line REPL, preserved verbatim). The `app*` prefix is shared
  with `Web.app` / `WebView.app`, so "`app*` is a TEA entry" reads uniformly.
  Rejected names: `Tui` (a terminal-UI abbreviation inside a module already named
  `Terminal`) and `cli` (a CLI is a one-shot arg→exit tool — the `Program`
  shape — not an interactive `onLine` loop, and it collides with `Ipe.Cli`).

- **Drop `Tui.program`** (the full-screen raw-`String` entry). Its
  paint-the-cells-yourself capability returns as a terminal-only `Ui.cells` node
  inside an `appScreen` `Element` view — the terminal analogue of `Ui.html`.

`Element` is not phantom-tagged by medium. The shape entry alone fixes which
escape nodes are admissible: `Ui.html` under the web shapes, `Ui.cells` under
`Terminal.appScreen`. Portable views stay medium-agnostic and the type surface
stays small.

This supersedes the shape layout recorded in ADR 0048 (which introduced the
`Ipe.Tea.*` relocation and the `appHtml` escape entries).

## Consequences

- The shape surface is smaller and uniform: four shapes, one structured entry
  each, escapes as nodes. A reader learns one entry per shape and one escape
  node per medium.
- `Web.app`, `Web.appRouted`, `Html.renderStatic`, `WebView.app`, and `Program`
  keep their behavior. The per-shape emitters are re-pointed to the renamed
  kernel variants (`TerminalAppScreen`, `TerminalAppLines`), not rewritten.
- The raw-cell terminal escape, `Ui.cells`, is the follow-on that completes the
  model: it must be admissible only under `Terminal.appScreen` and rejected
  under the web shapes, mirroring how `Ui.html` is a web-side node.
- Invariant to preserve: escapes stay nodes. A future medium-specific capability
  is added as a node gated by shape admissibility, never as a new whole-view
  entry point.
