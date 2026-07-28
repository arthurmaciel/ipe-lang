# 43. Webview app-entry target — native desktop window over a system webview

Date: 2026-07-25

## Status

Accepted and implemented. `WebView.app` is wired end to end: the runtime lives in
`src/runtime/rust/src/webview.rs` (`webview_app`, `WebviewWindowCfg`), emission in
`src/compiler/backend/rust/src/emit_webview.rs`, with the IR kernel
(`KernelFn::WebviewApp`), the callee resolution `("Webview","app") → WebviewApp`, the
constrain scheme, the lowering exemption arm, and the manifest/module injection all
in place. The runtime is feature-gated (`webview = ["wry", "tao", "live"]`) so a
default build does not require the system webview libraries.

## Context

Ipê already runs Elm-shaped apps through two proven app entries — the live HTTP
surface and the terminal UI. A desktop target wants the same `init`/`update`/`view`
loop to open a native window backed by the operating system's webview, without a
bundled browser engine and without a new networking or session stack.

Two forces shape the design. First, type safety: the app configuration must be a
concrete, closed shape so a mistyped entry is a compile error, never a silent blank
window. Second, the platform's threading rule: a system webview must be created and
pumped on the process's main thread on the platforms Ipê targets. The default app
entry spawns its executor on a worker thread — correct for a server, fatal for a
window (the app compiles, then dies on first paint).

## Decision

`WebView.app` takes a single closed five-field record, every field required:

```
WebView.app :
  { init          : () -> (Model, Cmd Msg)
  , update        : Msg -> Model -> (Model, Cmd Msg)
  , view          : Model -> Html Msg
  , subscriptions : Model -> Sub Msg
  , window        : { title : String, size : (Int, Int) }
  } -> Task Error ()
```

The shape is deliberately concrete: `init` takes `()` (the unit type, not an empty
tuple — the two do not unify); `view` returns `Html` (the user wraps a UI layout to
convert, and forgetting the wrap is a type error, `Element ≠ Html`); `window` is a
nested closed record checked by the existing structural record and tuple unification
with zero new type machinery. Making the config a closed record is what makes a
malformed app entry unrepresentable rather than a runtime surprise.

The bridge is **in-process and local-content-only**: the initial render is handed to
the webview as HTML; DOM events post an IPC message that is parsed, resolved through
the reused live handler index, run through `update`, re-rendered, and applied by a
single script-execution slot that swaps the document body. There is no HTTP server,
no server-sent events, and no session store. The content is always loaded as local
HTML, never from a URL.

The single genuinely new, soundness-critical requirement: when a program uses the
webview entry, the emitted `main` must drive the executor on the current (main)
thread rather than spawning it. This is applied as an anchor-asserted one-shot
rewrite that fails loud — it emits an internal compiler error on anchor drift rather
than silently doing nothing. Omitting it is the exact exit-0-at-compile,
death-on-first-paint failure the target must foreclose; its regression guard is the
windowed paint test.

Security posture is inherited, with one honest caveat recorded so it is not
mis-stated as stronger than it is. Every text and attribute node is HTML-escaped on
render. The one script-execution slot is safe because it is a JavaScript-*execution*
context (not an HTML parse) into which values are injected as JSON string literals,
which escapes the literal delimiters and control bytes — **not** because that
encoding escapes the line/paragraph-separator code points (it does not). There is no
eval-backed sink, no dynamic `Function` construction, and the IPC channel is
inbound-only from the app's own bridge.

Feature gating is structural, not a runtime toggle: enabling the webview feature
unconditionally requires the system webview development libraries, so the tool
preflights for them and emits an actionable diagnostic when they are absent. The
non-TEA command/subscription conveniences that have no window analogue are dropped
in the first cut with a one-time warning — observable, documented, never a panic.

Rejected: reimplementing a bundled browser engine, adding an HTTP/session layer for
what is an in-process app, and leaving the main-thread switch as a best-effort
textual substitution (a silent no-op on drift would reintroduce the death-on-paint
failure).

## Consequences

- A desktop app reuses the live and terminal app-entry mechanics; the only new
  runtime surface is the window bridge, and the only new compiler surface is the
  main-thread entry switch.
- The closed five-field config is the invariant that keeps the entry type-safe: any
  future field change must preserve "malformed entry is a compile error, blank
  window is impossible."
- The main-thread executor switch must stay anchor-asserted and fail-loud. If it ever
  degrades to a silent no-op, the target regresses to compile-clean-then-die.
- Testing is tiered so "exit-0 then cargo fail" and "exit-0 then death on paint" are
  both foreclosed: a blocking build-and-link tier (real libraries and the graceful
  stub), a windowed spawn/render/no-crash tier that loud-skips on a displayless host,
  and a round-trip coverage tier that never pollutes the shipped runtime. Interactive
  click-driving through the real window is not provable with the current webview
  library, so the round-trip tier covers that class instead.
- The security caveat about line/paragraph separators must remain stated honestly in
  user-facing docs: the encoding is safe for its JSON-string-literal context, and the
  claim must not be inflated into a general HTML-context guarantee.
