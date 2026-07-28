Status: Accepted
Date: 2026-07-28

# 0048. Relocate TEA shapes under `Ipe.Tea.<Shape>` and make TEA-vs-Program one structural rule

## Context

Ipê has two kinds of runnable module. A **Program** is a plain `main : Task Error ()`
— a batch or server entry that runs an effect and exits, with no managed state
loop and no view. A **TEA app** runs the managed `init` / `update` / `view` /
`subscriptions` loop and comes in four shapes, today spelled as top-level stdlib
modules: `Ipe.Web` (browser-live, VNode-diff over SSE), `Ipe.WebView` (native
desktop webview), `Ipe.Tui` (ANSI terminal), and `Ipe.Console` (line-oriented
stdout). Each exposes an app entry — `Web.app`, `WebView.app`, `Tui.app`,
`Tui.program`, `Console.app` — taking a closed config record. The canonical
`Cmd` / `Sub` effect types are their own top-level kernel qualifiers `Ipe.Cmd`
and `Ipe.Sub`.

Two forces made the flat top-level layout unsatisfactory:

1. **There was no structural marker of "this module drives a live loop."** The
   only thing separating a Program from a TEA app was which named entry it
   happened to call. A reader — and, more importantly, the compiler — could not
   answer "is this a managed-loop module?" without knowing the full list of app
   entries. That list is open (shapes are added), so any gate keyed on it is a
   drifting enumeration, exactly the shape the "make invalid states
   unrepresentable" and "single source of truth" rules forbid. A Program that
   accidentally reached into live-loop machinery had no representable barrier
   stopping it.

2. **The four graphical/console shapes disagreed on the `view` type for no
   principled reason.** Verified against the backend emitters:
   - `Web.app` → `view : Model -> Html Msg` (`emit_web.rs`, app path recovers
     `Model` from `view`'s first parameter; `Web.renderStatic :
     (Model -> Html Msg) -> Model -> Task Error ()`).
   - `WebView.app` → `view : Model -> Html Msg`, where the view wraps
     `Ui.layout [] element` to produce the `Html` (`emit_webview.rs`).
   - `Tui.app` → `view : Model -> Element Msg` — the `Ipe.Ui` typed element tree,
     rendered to ANSI cells by the runtime (`emit_tui.rs`).
   - `Tui.program` → `view : Model -> String` — a raw ANSI frame, painted
     verbatim (`emit_tui.rs`).
   - `Console.app` → `view : Model -> String` — printed to stdout on each state
     change (`emit_console.rs`).
   - Program → no view.

   So switching an app between Web, WebView, and Tui was not the one-line change
   the shared TEA loop promises: `Web`/`WebView` demanded `Html`, `Tui.app`
   demanded `Element`, even though `Ipe.Ui`'s `Element` already renders to the
   DOM on the web side (`IrType::Ui` lowers to `Element`) and to ANSI on the
   terminal side.

The governing principle order is **Security ≫ Correctness ≫ Soundness ≫
Efficiency ≫ Completeness ≫ Readability** (PRINCIPLES.md). Readability alone —
"a nicer namespace" — cannot buy a design change. But the first force above is a
**soundness** locus: the make-invalid-states-unrepresentable rule wants a single
structural barrier, not a per-shape enumeration, so that a Program can never
reach live-loop machinery. That is what justifies the move, with the ergonomic
and readability gains riding along for free.

## Decision

**Relocate the TEA shapes under an `Ipe.Tea.<Shape>` namespace and make the
TEA-vs-Program distinction a single structural rule. Keep view / effect data
types shape-agnostic and top-level.**

1. **Shapes move under `Ipe.Tea.*`.** The four managed-loop shapes become
   `Ipe.Tea.Web`, `Ipe.Tea.WebView`, `Ipe.Tea.Tui`, and `Ipe.Tea.Console`. Each
   exposes a **per-shape, precisely-typed `.app`** (and `Ipe.Tea.Tui` also
   `program`). The `.app` entries are per-shape rather than one unified entry
   because the `view` type genuinely differs per shape; a single entry could not
   type `view` without higher-kinded machinery or a loose config that would
   readmit invalid states. (This mirrors Elm's split of `Browser.sandbox` /
   `element` / `document` into separate, separately-typed entries — the
   Elm-family design our stdlib follows.)

2. **The gate is one structural rule.** The TEA-vs-Program distinction is exactly
   *"does the module import anything under `Ipe.Tea.*`?"* A **Program** — a plain
   `main : Task …`, non-TEA module — **may not import any `Ipe.Tea.*` module**,
   and any `Ipe.Tea.*` import marks the module as a TEA app. This is a single
   rule that is independent of the shape list: adding a fifth shape adds nothing
   to the gate. It is the make-invalid-states-unrepresentable locus — the one
   representable barrier between the two module kinds, enforced by a new
   compile-time diagnostic rather than by convention.

3. **One canonical `Cmd` / `Sub`, re-exported per shape.** There remains exactly
   ONE internal canonical `Cmd` / `Sub` type (the single source of truth today
   at `Ipe.Cmd` / `Ipe.Sub`). It moves to an internal canonical home and is
   **re-exported by each `Ipe.Tea.<Shape>`**, so a user imports only the shape
   they use — there is no public bare `Ipe.Tea` module to import. Effect
   libraries (`Http`, `Time`, …) and the `Task`→`Cmd` bridge
   (`Task.perform` / `Task.attempt`) reference the internal canonical home. A
   single shared `Cmd` type is required so those effect libraries compose across
   every shape; per-shape `Cmd` *types* would fragment the effect ecosystem into
   incompatible islands.

4. **Unify the graphical view on `Element Msg`.** `Ipe.Tea.Web`,
   `Ipe.Tea.WebView`, and `Ipe.Tea.Tui`'s `.app` unify on
   `view : Model -> Element Msg`. On Web and WebView the framework applies
   `Ui.layout` internally — the web runtime already renders `Element` → DOM
   (`IrType::Ui` lowers to `Element`), so `Element` becomes the uniform, portable
   view across the graphical shapes, and switching among them is a genuine
   one-line change of the imported shape. `Html` remains the Web/WebView-only
   raw-DOM escape hatch, reached two ways: `Ui.html` embeds raw `Html` inside a
   `Ui` tree, and an optional raw-`Html` entry —
   `Ipe.Tea.Web.appHtml : { view : Model -> Html Msg, … }` — serves apps that
   author the DOM directly. `Ipe.Tea.Tui.program` and `Ipe.Tea.Console` stay
   `view : Model -> String`: a terminal line or a painted frame is not a DOM
   node, a genuinely different medium, so forcing `Element` there would be a
   false unification.

5. **`Ipe.Ui` / `Ipe.Html` / `Ipe.Css` stay TOP-LEVEL.** These are shape-agnostic
   **data + static-rendering** modules usable by ANY module, Program included:
   `Html.render` and `Web.renderStatic` are `Task`-based, with no live loop. The
   `Ipe.Tea` gate forbids only the **live-loop machinery** — the `.app` / `program`
   entries and the `Cmd` / `Sub` re-exports — and never the `Html` / `Ui` / `Css`
   data or their static renderers. A Program is free to build a `Ui` tree and
   render it with a `Task`. This boundary is load-bearing: it is precisely why
   the gate is safe to state as "no `Ipe.Tea.*` import" without accidentally
   forbidding legitimate static rendering.

6. **Shape-switch ergonomics come from the shared `Element` view plus a minimal
   `.app` per shape** for the common case — the Elm `sandbox`-vs-`element` split.
   They do NOT come from optional record fields (Ipê records are closed; there is
   no optional-field feature) and NOT from a unified Shape-config ADT.

## Rejected alternatives

- **Per-shape `Cmd` / `Sub` *types*.** Rejected: it fragments the effect
  ecosystem. A shared `Cmd` is what lets `Http` / `Time` / any effect library
  compose across shapes; per-shape effect types would force every library to be
  written once per shape or bridged by hand.

- **A unified `Tea.app (Shape cfg)` config ADT** with one entry point. Rejected:
  the `view` type genuinely differs per shape (`Element` vs `String`), so a
  single entry cannot type `view` without higher-kinded types (which Ipê, like
  Elm 0.19, does not have) or a loose config that readmits invalid states —
  regressing make-invalid-states-unrepresentable, a soundness cost the ordering
  forbids.

- **Optional record fields on one `.app`** to paper over the per-shape
  differences. Rejected: Ipê records are closed and have no optional-field
  feature; adding one for this is a large language change to avoid a namespace,
  and it would move a static per-shape distinction into a runtime-checked record
  shape.

- **Naming the namespace `Ipe.Platform`** (Elm's name for its runtime plumbing).
  Rejected: Elm's `Platform` is effect-manager plumbing and every Elm program is
  a TEA program, so `Platform` reads as "the runtime." Ipê additionally has a
  non-TEA Program shape that Elm lacks, so the meaningful axis here is
  specifically *the managed update loop*, which `Tea` names directly and
  `Platform` does not.

## Consequences

- **A new structural barrier.** The `Ipe.Tea.*`-import gate becomes the single
  definition of "TEA app." Adding a shape requires no gate change — the invariant
  is shape-list-independent, which is the whole point. A Program importing any
  `Ipe.Tea.*` module is now a compile-time error (a new `IPE-N`-class diagnostic).

- **`Ui` / `Html` / `Css` must stay top-level for the gate to be correct.** If any
  of them were ever pulled under `Ipe.Tea`, a Program doing legitimate static
  rendering would trip the gate. This invariant must hold for the decision to
  remain valid: the `Ipe.Tea` namespace contains only live-loop machinery.

- **One canonical `Cmd` / `Sub` remains the single source of truth**, now reached
  through per-shape re-exports. The bridge and every effect library must continue
  to reference the internal canonical home, never a per-shape copy.

- **The graphical shapes converge on `Element`**, making a Web↔WebView↔Tui switch
  a one-line import change; `Html` narrows to an explicit escape hatch
  (`Ui.html`, `appHtml`). `Tui.program` and `Console` keep `String`.

- **Migration cost.** First-party examples and goldens that import the flat
  `Ipe.Web` / `Ipe.WebView` / `Ipe.Tui` / `Ipe.Console` shape modules, or the
  bare `Ipe.Cmd` / `Ipe.Sub`, must be migrated to the `Ipe.Tea.*` imports and
  regenerated. The tracked Sky mirror under `examples/sky/` is out of scope and
  is not touched. Docs that describe the shapes (`docs/shapes/*.md`, `docs/ui.md`)
  and the Elm ledger must be updated.

- **Staged implementation.** The change is filed as a staged implementation issue
  (see the impl issue linked from the PR): Stage 1 relocates the modules + moves
  `Cmd` / `Sub` to the internal canonical home + adds the Program gate + migrates
  first-party examples/goldens; Stage 2 unifies the graphical view on `Element`
  and adds `appHtml`; Stage 3 delivers the minimal-`.app`-per-shape ergonomics.
  Stages are independent PRs where noted in the issue.

## Conventions

This ADR describes Ipê on its own terms. Where it cites Elm (`Browser.sandbox` /
`element` / `document`; the `Platform` naming), it does so because Ipê is an
Elm-family language and Elm precedent is the project's declared design reference,
not as parity with a prior implementation.
