# `Ui.mediaQuery` — emission mechanism + security gate (2026-07-11)

> Closes the 1-of-20 deliberately-deferred kernel from the 2026-07-10
> Std.Ui/Html wiring batch (BACKLOG "Sweep to green" row). Companion to
> `docs/architecture/ui-html-completeness-design.md` (#76) and
> `docs/architecture/ui-css-escaping-fix-spec-2026-07-10.md`.

## 1. What was actually missing (reuse-vs-new call)

The deferral note said mediaQuery "needs a genuinely new wrapper-Element CSS
media-query emission mechanism". **That is no longer true at HEAD.** The
2026-07-10/11 batches landed `runtime/src/sky_runtime/live/style_inject.rs`,
whose `apply_style_injections` first pass already consumes the
`data-sky-mq-q` + `data-sky-mq-rules` marker pair via `build_mq` and emits
exactly the upstream shape:

```
<style data-sky-mq="<sid>">@media <q> { [sky-id="<sid>"] { <rules> } }</style>
```

— sky-id-scoped (no cross-contamination between two breakpoints on a page),
close-tag-stripped, void-hoisted (#409), idempotent, and already covered by
security regressions (`mq_strips_style_close_in_query_and_rules`,
`fixture70_mediaquery_breakout_probe_neutralised`).

So the CONSUMER exists and is tested; what was missing is the PRODUCER: a
kernel + runtime helper that builds the wrapper element carrying the markers.
Upstream (`../sky` `sky-stdlib/Std/Ui.sky:1234-1247`) defines it in 4 lines:

```elm
mediaQuery query attrs child =
    Node NoDescription
        [ AttrAttribute "data-sky-mq-q" query
        , AttrAttribute "data-sky-mq-rules" (mediaQueryRulesCss attrs)
        ]
        [ child ]
```

**Decision: reuse, no new infrastructure.** The Rust port mirrors upstream
verbatim as `ui_media_query_` in `runtime/src/sky_runtime/ui/helpers.rs`:

- rules string = `render::build_style_string(&attrs)` — the SAME collector as
  the inline-`style=""` path and `Ui.onPseudo` (upstream's
  `mediaQueryRulesCss` is the same fold). This inherits the 2026-07-11
  `SafeCssValue` hardening: every value-as-data attr in `attrs` is already
  gated per-declaration, so a `}`-breakout via `Font.family` etc. is dropped
  before it ever reaches the marker.
- wrapper = `Element::Node(Description::NoDescription, markers, vec![child])`
  — renders as a plain `<div>`, same as `Ui.el`.
- the Live / Webview pipelines (`apply_style_injections` call sites in
  `live/mod.rs` + `webview.rs`) do the rest, unchanged. Plain `Html.render`
  (no pipeline) keeps the markers inert-but-visible, identical to the
  established `data-sky-pc-rules` behaviour in the wiring-batch golden.

Kernel wiring follows the established 8-file recipe, cloned from
`UiBreakpoint` (the one other `String -> List (Attribute msg) -> Element msg
-> Element msg` kernel): `StdlibKernel::UiMediaQuery` variant + `decl()` +
`ALL` + `is_ui()`; constrain scheme (same shape as `K::UiBreakpoint`) +
`FIRST_SCHEMED`; lower legacy-match arm + arity-3 bucket; `naming.rs`;
additive `emit_expr.rs` arm; `pretty.rs`; remove `("Ui", "mediaQuery")` from
`deliberately_unbacked_members` in `crates/sky_canon/src/lib.rs` so the
exhaustiveness gate now REQUIRES the backing.

## 2. Security gate on the query string

The `attrs` side is covered by `build_style_string`'s per-value
`SafeCssValue` gate plus `build_mq`'s `strip_style_close`. The raw `query`
STRING is the one new attacker-influenced input: it is spliced into
`@media {query} {` inside a raw (`HRaw`) `<style>` body, so a crafted query
could otherwise close the media prelude (`{`/`}`), terminate the style
element (`</style>`), open a comment (`/*`), or smuggle `@import`.

Candidate gates in `runtime/src/sky_runtime/css_safety.rs`:

| Option | Verdict |
|---|---|
| `SafeCssSelector` | **Rejected.** Its allowlist blocks `<` outright — but Media Queries Level 4 range syntax (`(400px <= width <= 700px)`) legitimately uses `<`/`<=`. A selector and a media query are different grammars; borrowing the selector gate would silently break valid queries. |
| `SafeCssValue` as-is | **Policy fits, type doesn't.** Its danger-pattern set (`; { } </ /* @import` + script sinks, scanned on both the raw and CSS-escape-decoded forms) is exactly the breakout set for the `@media` position, and none of those characters occur in any valid media query. But reusing the "declaration value" type for a "media query" boundary muddles the parse-don't-validate story (each boundary should carry its own proof type). |
| Narrow new validator | **Chosen — as a thin newtype over the SHARED policy.** `SafeCssMediaQuery::parse` delegates to the same `has_dangerous_css_pattern` + `css_unescape` re-scan pair `SafeCssValue` uses (one policy, one place — module contract §Q5; no second weaker encoder can drift), and names the media-query boundary in the type system. |

Fail mode is **fail-closed drop, styling only**: when the query fails the
gate, `ui_media_query_` emits the wrapper `<div>` with NO marker attrs — the
child renders normally, the media-query styling is silently dropped
(identical posture to `build_style_string` dropping a poisoned declaration).
DOM shape stays stable either way (always a wrapper), so the Live diff never
sees a gate-dependent structural change. `build_mq`'s `strip_style_close`
stays as sink-side defence-in-depth, not the primary gate.

## 3. `Ui.breakpoint` un-stubbed for free

Upstream defines `breakpoint bp attrs child = mediaQuery (breakpointToQuery
bp) attrs child`, and this port types `Breakpoint` as the raw query `String`
(sanctioned divergence), so `breakpointToQuery` is the identity here.
`ui_breakpoint_`'s documented Phase-0 eager-passthrough stub (query + attrs
ignored) is therefore replaced by a one-line delegation to
`ui_media_query_`. No test pinned the passthrough behaviour; the Phase-0
comments in `helpers.rs` / `sky_kernels` are updated. This closes the
"breakpoint has no real CSS emission" half of the BACKLOG row with zero
additional mechanism.

## 4. Tests

- `css_safety.rs`: `SafeCssMediaQuery` accepts real queries (incl. level-4
  range syntax with `<=`), rejects `{`/`}`/`;`/`</`/`/*`/`@import` and their
  CSS-hex-escaped spellings.
- `ui/render.rs`: `Ui.mediaQuery` emits the wrapper with both markers
  (`data-sky-mq-q` verbatim query, `data-sky-mq-rules` collector output);
  breakout query → no markers, child intact.
- `live/style_inject.rs`: end-to-end pipeline (`ui_media_query_` →
  `ui_layout` → `assign_sky_ids` → `apply_style_injections` → `render_html`)
  produces the sky-id-scoped `<style data-sky-mq=…>@media …` block and leaks
  no marker; plus an exact-output assertion at the injector.
- E2E golden `crates/skyc/tests/golden_ui_mediaquery.rs` +
  `tests/golden/ui_mediaquery/Main.sky` (SKY_E2E=1): a real program using
  `Ui.mediaQuery "(min-width: 768px)" [Background.color …] child` compiles
  through skyc, `cargo build`s, runs, and prints the wrapper with correct
  markers (same oracle stance as the wiring-batch golden:
  `oracle_divergence = true`, semantics pinned by direct assertions).
