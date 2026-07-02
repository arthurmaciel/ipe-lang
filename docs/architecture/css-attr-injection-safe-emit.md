# CSS + HTML-attribute injection-safe emission

Design specification for three linked, pre-push tasks treated as **one
security surface**:

- **#46** — wire the plain HTML attribute builders
  (`class` / `id` / `href` / `type_` / `value` / `style` / the generic
  `attribute k v` / `boolAttribute k b`).
- **#47** — port `Std.Css` (`styleNode` / `stylesheet` / `rule` / `px` /
  `rem` / `hex` / colour + length + gradient builders).
- **F7** — the CSS **and** attribute **injection** security gate spanning both.

`F7` is a security item. Under the project ordering
`security > correctness > soundness > efficiency > completeness > readability`
it **outranks** #46 and #47: no convenience of the attribute or CSS surface
may open an injection vector. This design forecloses each vector **by
construction** — typed constructors, scheme-parsing into typed Safe-URL
values, and an attribute-name allowlist parsed into a typed `SafeAttrName` —
rather than by after-the-fact blocklist filtering.

Two fundamental rules govern the whole surface:

1. **Parse, don't validate.** An untrusted string crosses each sink boundary
   exactly once, is parsed into a typed value that carries the proof of
   safety in its structure, and is never re-inspected downstream.
2. **Make invalid states unrepresentable.** A dangerous attribute name, a
   dangerous URL scheme, or a `</style>`-bearing CSS body must not be
   *representable* at the emit sink — not merely rejected by a check that a
   future refactor could forget.

This mirrors Sky's HTML-escape-everything / no-`data-sky-eval` posture (Std.Ui
"HTML-escapes everything"; `data-sky-eval` forbidden). The Go runtime's
`html.EscapeString`-based render is the byte-parity reference oracle; ipê's
render sink already reproduces it. `Std.Css`'s typed-with-`*Raw`-escape-hatch
shape is likewise the reference for #47's constructor set.

---

## 1. Executive summary

The render **sink** in this codebase is already the single choke point and is
already hardened; #46 and #47 must be wired so that **every** producer they add
flows through it, and F7 must close the **one** producer that currently bypasses
it.

- **#46 attribute builders are pure Sky** (`Std/Html/Attributes.sky`:
  `class v = Attr "class" v`, `attribute k v = Attr k v`,
  `boolAttribute k b = BoolAttr k b`). They construct the runtime
  `Attribute::Attr` / `BoolAttr` variants via the codegen bridge and are
  neutralised at render by the existing `SafeAttrName` + `sanitise_url_attr`
  gates in `runtime/src/sky_runtime/html.rs`. No per-attribute kernel and no
  new escaping is required; #46 is a wiring + regression-test task.
- **The generic `attribute k v` escape hatch is already safe** because its
  name and value are gated at the render sink, not at construction — but the
  design pins that guarantee with a name-allowlist parse and forbids the
  event-handler (`on*`) and `srcdoc` name classes there, so events can *only*
  travel through the typed `onClick`/`onInput`/`onBool` channel.
- **#47 `Std.Css` is the real F7 gap.** `Std.Css` renders to a `String` that
  `Std.Html.styleNode` places as an `HRaw` child of a `<style>` element
  (`Std/Html.sky` `styleNode attrs css = HElement "style" attrs [ HRaw css ]`).
  `HRaw` renders **verbatim** (`html.rs` `Html::HRaw(r) => s.push_str(r)`), so a
  `</style>` inside a `Css.property`/`colorRaw`/`LenRaw` value breaks out of the
  element into script context. The Std.Ui `<style>` path is protected by
  `strip_style_close` (`live/style_inject.rs`), but the `Std.Html.styleNode` /
  `Std.Css` path does **not** run it. F7 routes the `styleNode` body through the
  same close-tag-neutralising encoder, and keeps `Std.Css`'s safe path on typed
  length/colour constructors so no raw string value or selector is required for
  normal use.
- **No second, weaker encoder.** #46's `style` attribute, #47's `styleNode`
  body, and Std.Ui's existing inline-style path all route through the same
  injection-safe encoders (`SafeCssValue` / `SafeCssPropertyName` for
  declarations; `strip_style_close` for `<style>` bodies). One policy, one
  place.

Top open decisions for the user: (a) **reject vs neutralise** for a bad URL
scheme (current sink **neutralises** to empty — keep, or switch to a
fail-closed diagnostic?); (b) whether the **generic `attribute k v` escape
hatch ships at all pre-push**, or is withheld until v-next so the only
attribute surface is the typed builder set.

---

## 2. Trust boundaries, sinks, and the current gate inventory

### 2.1 Untrusted inputs

Every string that reaches an attribute value, an attribute name, a CSS
declaration, a CSS selector, or a `<style>` body is potentially
attacker-influenced: it flows from `Model` (which commonly holds user content —
comments, profile fields, query params) through `view` into the render tree.
The name argument of the generic `attribute k v` and every `*Raw` CSS escape
hatch are the highest-risk because they let the *structure* (not just a leaf
value) be attacker-derived.

### 2.2 Injection sinks and the existing gate at each

| Sink | Vector | Existing gate (file:symbol) |
|---|---|---|
| Text node content | `<script>` injection | `escape_text` — `& ' < > "`, Go `html.EscapeString` parity (`html.rs:417`) |
| Attribute **value** | quote breakout (`x" onmouseover=…`) | `escape_attr` (`html.rs:428`) at `render_into_ctx` (`html.rs:339`) |
| Attribute **name** | new-attribute / handler injection (`x onload=…`, `onerror`, `srcdoc`) | `SafeAttrName::parse` = `is_safe_html_name` + `!is_dangerous_attr_name` (`html.rs:445,459,472`) |
| Element **tag** name | start-tag breakout (`div><script>`) | `is_safe_html_name` (`html.rs:226,445`) |
| Event marker name | marker injection | `is_safe_html_name` (`html.rs:352`) |
| `href`/`src`-shaped value | `javascript:` / `data:text/html` / scriptable `data:image/svg+xml` | `is_url_attr` + `is_dangerous_url` + `url_scheme` + `is_inert_data_image` → `sanitise_url_attr` (`html.rs:488-604`) |
| SSE patch attribute | same as name+value, but bypasses first-paint render | `safe_patch_attr` (`html.rs:621`) → `SafeAttrName` + `sanitise_url_attr` |
| Std.Ui inline `style` **value** | `expression()` / `url(javascript:)` / `;`-breakout | `SafeCssValue::parse` whole-string scan (`ui/render.rs:74`) |
| Std.Ui inline `style` **key** | key-smuggled rule (`background:…;x`) | `SafeCssPropertyName::parse` charset gate (`ui/render.rs:37`) |
| Std.Ui `background-image` URL | `javascript:`/`data:text` | `is_dangerous_url_scheme` (`ui/render.rs:126`) |
| Std.Ui pseudo-class / media `<style>` body | `</style>` breakout | `strip_style_close` loop-until-stable (`live/style_inject.rs:203`) |

### 2.3 The gap this design closes

`Std.Html.styleNode attrs css` (`Std/Html.sky:454`) builds
`HElement "style" attrs [ HRaw css ]`. At render, `HRaw` is emitted verbatim
(`html.rs:208`), and even the `raw_text` path for a literal `<style>` tag does
not strip `</style>` (documented Std.Html raw escape hatch, `html.rs:191-207`).
`Std.Css`'s renderers (`stylesheet` / `renderRule` / `styles`) concatenate
values including the `*Raw` hatches (`ColorRaw`, `LenRaw`, `property k v`) with
**no** escaping (`Std/Css.sky:1463-1508`). Therefore a `Std.Css` string fed to
`styleNode` reaches the DOM unfiltered — the `</style><script>` breakout and
`expression()` sinks are reachable on this path even though they are closed on
the Std.Ui path. **This is the F7 hole #47 introduces if wired naively.**

---

## 3. Locked answers

### Q1 — Attribute-value encoding (confirm)

**Locked: keep the existing `escape_attr` sink; #46 adds no new value path.**

Every attribute value is emitted only through `render_into_ctx`
(`html.rs:339`), which wraps it in `escape_attr(sanitise_url_attr(k, v))`.
`escape_attr` escapes `&` first, then `< > ' "` (with `"` → `&#34;` for Go
byte-parity, `html.rs:428-433`), so no value can break out of the double-quoted
attribute, and the same escaping is safe for a single-quoted attribute.

#46's `class` / `id` / `value` / `type_` / `style` are pure-Sky constructors of
`Attr k v` (`Std/Html/Attributes.sky:55-79`); they carry the value verbatim to
the runtime `Attribute::Attr` variant and *only* meet HTML at the render sink.
The generic `attribute k v` and `boolAttribute k b` are likewise
`Attr`/`BoolAttr` constructors. **No #46 builder emits HTML directly**, so there
is exactly one value-encoding site and #46 cannot introduce a second, weaker
one. The `Std.Html.attrToString` sibling sink (`html_attr_to_string_`,
`html.rs:750`) routes the same value through `html_escape_attr_` and the key
through `SafeAttrName` — verified by the `attr_to_string_drops_event_handler_and_srcdoc`
regression (`html.rs:875`).

Rationale: correctness+security served by a single sink; parse-don't-validate
is satisfied because the value is never re-inspected between construction and
the one escaping call.

### Q2 — URL-scheme safety for `href`/`src`-shaped attributes

**Locked: parse the value's scheme into a typed decision; block dangerous
schemes at the sink. Current behaviour = neutralise-to-empty. (Reject-vs-
neutralise is an open decision — see §6.)**

`sanitise_url_attr(name, value)` (`html.rs:598`) fires only for URL-bearing
names (`is_url_attr`: `href`, `src`, `action`, `formaction`, `cite`, `poster`,
`background`, `xlink:href`, `manifest`, `longdesc`, `data`, `html.rs:491`). It
parses the scheme with `url_scheme` (`html.rs:529`) — the run before the first
`:` that precedes any `/ ? #`, with C0/space stripped and lowercased, mirroring
the browser URL parser so `java\tscript:` and `\x01javascript:` are caught — and
returns a typed `Option<String>` scheme (`None` = relative/no-scheme = safe).
`is_dangerous_url` (`html.rs:579`) then blocks `javascript:` / `vbscript:`
always, and blocks `data:` everywhere except an **inert raster**
`data:image/{png,jpeg,gif,webp,bmp,avif,x-icon}` on a **media** attribute
(`src`/`poster`/`background`) — `data:image/svg+xml` is excluded because SVG can
script (`is_inert_data_image`, `html.rs:545`). A blocked value is neutralised to
`""` (an inert URL) before escaping.

This is parse-don't-validate: the scheme is parsed once into a typed value; the
allowlist (relative + `http`/`https`/`mailto` and the raster-`data:` media
carve-out) is the safe set, and everything else is neutralised. The allowlist
is default-deny for the schemes that matter (no `javascript:`/`vbscript:`/`data`
document ever passes). #46's `href`/`src` builders inherit this at the sink with
no new code.

Reference to Sky: Sky's Go runtime applies the analogous URL-scheme gate on the
same navigational attributes; ipê mirrors that neutralise-at-sink choice.

### Q3 — The generic `attribute k v` escape hatch (the danger)

**Locked: the attribute *name* is parsed into a typed `SafeAttrName`; event-
handler names (`on*`) and `srcdoc` are unrepresentable there; events travel
only through the typed `onClick`/`onInput`/`onBool` channel; `data-sky-eval`
stays forbidden. Whether the hatch ships pre-push at all is an open decision
(§6).**

`SafeAttrName` (`html.rs:472`) is the parse-don't-validate type: its **only**
constructor `parse` runs `is_safe_html_name` (charset: non-empty,
`[A-Za-z0-9-_:.]` only — rejects whitespace, `<>"'=/\`` `` ` ``, controls,
non-ASCII, so `x onload=…` cannot form) **and** `!is_dangerous_attr_name`
(rejects any name starting `on` case-insensitively, and `srcdoc` —
value-escaping is useless when the value *is* script or a scripting context,
`html.rs:459`). Every emit sink — `render_into_ctx` (`html.rs:332`),
`safe_patch_attr` (`html.rs:623`), `html_attr_to_string_` (`html.rs:759`) —
consumes a `SafeAttrName`, so a sink **cannot** forget the check; a name that
fails policy causes the whole attribute to be **dropped** (fail-closed).

Because `on*` is unrepresentable through the generic setter, an author cannot
register a DOM event via `attribute "onclick" "…"`; the *only* way to attach a
handler is the typed `Ui.onClick`/`Event.onBool`/`Ui.onInput` path, which lowers
to `EventAttr` and emits the client-side `data-sky-on` marker (`html.rs:352`),
never an inline handler. `data-sky-eval` is never emitted by any builder and is
not an accepted attribute name; it remains forbidden, and the URL sanitiser plus
`SafeAttrName` mean no attribute can smuggle an eval sink.

No `_ =>` wildcard swallow: the emit match arms for attributes are explicit
(`Attr` / `BoolAttr` / `EventAttr` / `NoAttr`, `html.rs:254-296`), and a
disallowed name is a deliberate drop, documented at the drop site, not a silent
catch-all.

### Q4 — `Std.Css` emission (#47)

**Locked: the safe path is typed constructors only; free-string values and
selectors go through a whole-string CSS-value gate and a selector gate; the
`<style>` body is close-tag-neutralised; `expression()` / `url(javascript:)` are
unreachable on the safe path and neutralised on the escape-hatch path.**

`Std.Css` already makes most invalid states unrepresentable: `px : Int ->
Length`, `rem : Float -> Length`, `rgb : Int -> Int -> Int -> Color`,
`hex : String -> Color` are typed constructors, and lengths/colours render via
`lengthToString`/`colorToString` from bounded numeric fields
(`Std/Css.sky:322-460`, `255-322`). A caller building a stylesheet from `px`,
`rem`, `rgb`, `rgba`, `hsl` cannot express a `</style>` or `expression()` — the
numbers stringify to digits.

The residual vectors are the **`*Raw` escape hatches** and free-string entry
points that Std.Css exposes for the rare value the enums do not cover:
`ColorRaw` (via `colorRaw`), `LenRaw`, `property : String -> String -> CssProp`
(`Std/Css.sky:533`), the `rule`/`media`/`keyframes` **selector**/query strings,
and `raw : String -> CssRule`. #47's port must gate these:

1. **Declaration values on the escape-hatch path** (`property k v`, `colorRaw`,
   `LenRaw`) route through the **same** `SafeCssValue` whole-string scan already
   used for Std.Ui inline styles (`ui/render.rs:74`): reject `;` `{` `}` `</`
   `/*` `@import` and the script sinks `expression(`, `javascript:`,
   `vbscript:`, `url(javascript:`, `url(data:text`, `url(data:application` —
   checked case-folded with whitespace stripped so `url( javascript:` and
   `java script:` cannot evade. A failing value is dropped.
2. **Declaration keys** route through `SafeCssPropertyName` (`ui/render.rs:37`):
   `[A-Za-z0-9-]` only (covers vendor prefixes and `--custom` properties),
   closing the key-smuggled-rule vector.
3. **The `<style>` body** produced by `styleNode` (and any `stylesheet` string
   spliced into a `<style>`) is passed through `strip_style_close`
   (`live/style_inject.rs:203`) — the loop-until-stable `</style` remover that
   defeats the `</sty</stylele` reconstruction trick — before it becomes an
   `HRaw` child. This is F7's core change: `styleNode`'s body must not reach the
   DOM as un-neutralised `HRaw`.
4. **Selectors / media queries** (`rule` / `media`) are validated against a
   conservative selector grammar (letters, digits, and the CSS structural set
   `. # : - _ [ ] = " ' , > + ~ * ( ) space`, no `{ } ; </ /* @` except the
   leading `@media`/`@keyframes` keyword the constructor itself supplies); a
   selector that fails is dropped along with its rule. Since `strip_style_close`
   also runs over the assembled body, a selector that slipped a `</style` is
   caught by defence-in-depth.

Result: on the typed path (`px`/`rem`/`hex`/`rgb`…) injection is
**unrepresentable**; on the `*Raw`/`property`/selector path it is
**neutralised** by the shared CSS gate + close-tag stripper. `expression()` and
`url(javascript:)` are not reachable through either path.

Rationale: the typed constructors satisfy make-invalid-states-unrepresentable
for the 99% path; the shared `SafeCssValue`/`strip_style_close` gate covers the
escape hatches without a second encoder.

### Q5 — style-attribute vs `<style>`-element: one encoder

**Locked: three producers, two shared encoders, zero new ones.**

- The **`style` attribute** (#46 `Attr "style" v`, and Std.Ui's computed inline
  style) — declaration *values* are gated by `SafeCssValue` and keys by
  `SafeCssPropertyName` in `build_style_string` (`ui/render.rs:160-347`); the
  final merged `style="…"` string is additionally `escape_attr`'d at the HTML
  sink (`html.rs:339`). The style-merge logic (`html.rs:253-297`) that folds a
  computed style and a user `htmlAttribute "style"` into one attribute keeps
  both declarations without introducing an unescaped path.
- The **`<style>` element body** (#47 `styleNode`, Std.Ui pseudo-class/media
  injection) — gated by `strip_style_close` (`live/style_inject.rs:203`), the
  single close-tag-neutralising encoder.

#47 and #46 both reuse these; the design forbids a `styleNode`-specific or
`Css`-specific encoder. Concretely: `SafeCssValue`, `SafeCssPropertyName`, and
`strip_style_close` are promoted to a shared module path (e.g.
`sky_runtime::css_safety`) so `Std.Css`'s port and the existing Std.Ui path
import the identical functions — one policy, one place, no drift.

### Q6 — Test plan (injection regressions + Go-parity)

Each dangerous input MUST be neutralised or rejected in emitted output; assert
the dangerous substring is escaped/absent. Benign inputs get Go-parity value
checks. New tests live beside the existing ones (`html.rs` `#[cfg(test)]`,
`ui/render.rs` `#[cfg(test)]`, `live/style_inject.rs` tests) plus a golden.

Injection regressions (all assert *absence* of the live substring):

1. **Attr-value breakout** — `attribute "title" "x\" onmouseover=\"alert(1)"`
   → output contains `onmouseover=&#34;` escaped, no bare `onmouseover="`.
2. **`javascript:` href** — `href "javascript:alert(1)"` → emitted `href=""`,
   no `javascript:`.
3. **`data:text/html` src / `data:image/svg+xml` src** — neutralised;
   `data:image/png` src passes (Go-parity value check).
4. **`onclick` via generic `attribute`** — `attribute "onclick" "x"` and
   `attribute "OnClick" "x"` and `attribute "onfoo" "x"` → attribute dropped,
   no `onclick`/`onfoo` in output; `srcdoc` likewise (extends existing
   `render_drops_event_handler_and_srcdoc_attrs`, `html.rs:841`).
5. **Tag breakout** — `Html.node "div><script>" [] []` → element dropped.
6. **`</style><script>` in a CSS value** — `styleNode [] (Css.stylesheet [
   Css.rule "body" [ Css.property "color" "red</style><script>alert(1)</script>"
   ] ])` → body contains no `</style>` and no `<script>`; and the direct
   `Css.property "x" "y}</style>…"` value is dropped by `SafeCssValue`.
7. **`expression()`** — `Css.property "width" "expression(alert(1))"` and the
   mid-value `Css.property "background" "0; background:url(javascript:alert(1))"`
   → dropped (extends `css_midvalue_injection_dropped`, `ui/render.rs:706`).
8. **`</style` case/obfuscation** — `styleNode [] "a{}</StYlE ><script>"` and the
   `</sty</stylele` reconstruction case → `strip_style_close` leaves no
   `</style` (extends `style_inject.rs` tests).
9. **Selector breakout** — `Css.rule "body{}</style><script>" [...]` → rule
   dropped / body sanitised.
10. **SSE patch parity** — `safe_patch_attr("onclick", "x")` → `None`;
    `safe_patch_attr("href", "javascript:…")` → value neutralised
    (extends existing `safe_patch_attr` coverage).

Go-parity (benign) value checks: `class "a b"` → `class="a b"`;
`hex "ff6600"` → `#ff6600` (leading `#` normalised, `Std/Css.sky:262`);
`px 100` → `100px`; `rgb 255 102 0` → `rgb(255,102,0)`; `boolAttribute
"disabled" True` → `disabled="true"` (BoolAttr Go-parity, `html.rs:284`); a
full `stylesheet [ rule ".card" [ property "color" (colorToString (rgb …)) ] ]`
byte-compares to the Go renderer output. These run in the existing skyc golden
harness (`crates/skyc/tests/golden_m7_stdui_*`) as the Rust≡Go equivalence gate.

### Q7 — Roadmap and parallel-safety

**F7 lands before the public push (non-negotiable security gate). #46 and #47
are pre-sweep / before-push.**

Ordering within the surface:

1. **F7-a** (blocking): promote `SafeCssValue` / `SafeCssPropertyName` /
   `strip_style_close` to a shared `css_safety` module; route
   `Std.Html.styleNode`'s body through `strip_style_close` so the `HRaw`
   `<style>` body is neutralised. Add regressions 6–9. This closes the concrete
   hole independently of whether #47's full Css surface has landed.
2. **#46**: wire the attribute builders (pure-Sky `Std/Html/Attributes.sky`
   compilation → runtime `Attribute::Attr`/`BoolAttr` bridge) and add
   regressions 1–5, 10. No new escaping — verification is that every builder's
   output meets the render sink.
3. **#47**: port `Std.Css` (typed constructors + `*Raw`/`property`/selector
   escape hatches routed through the shared `css_safety` gate) and add the
   Go-parity goldens.

Parallel-safety with the concurrent workstreams:

- **exit0 registry migration** touches `canon` / `types` (constrain schemes,
  registry keys). This surface touches `crates/sky_backend_rust/src/emit_*` and
  `runtime/src/sky_runtime/**` (emit + runtime). The only `sky_types` change
  #46/#47 need is adding constrain schemes for the Css/attribute kernels
  (`crates/sky_types/src/constrain.rs`, alongside the existing Ui/Event arms at
  `constrain.rs:3759-3792`) — **disjoint** from the registry-key migration as
  long as both land their `constrain.rs` edits as additive arms (no shared line
  ranges). Coordinate only the constrain-arm insertion point; emit + runtime
  files do not overlap the registry migration at all.
- **TailCallOpt** lives in `crates/sky_lower/src/lower.rs` (tail-position
  detection) and its emit consumers touch `emit_expr.rs` / `emit_types.rs` /
  `naming.rs`. This surface also edits `emit_expr.rs` (adding the attribute /
  Css kernel arms in the same `emit_ui_call` match as the existing
  `UiOnClick`/`UiOnInput` arms, `emit_expr.rs:1896-2035`). **Overlap is in
  `emit_expr.rs` only, and only additively**: new `KernelFn::HtmlClass` /
  `HtmlAttribute` / `CssStyleNode` / … arms are appended to the same match, not
  interleaved with TCO logic (TCO operates on the lowered call graph, not on
  these leaf kernel arms). Flagged: land the two `emit_expr.rs` edits as separate
  arms to avoid a merge conflict; no semantic interaction.

---

## 4. Injection-vector ledger — foreclosed by construction

| # | Vector | Foreclosed by | Construct (not blocklist) |
|---|---|---|---|
| V1 | Attr-value quote breakout | `escape_attr` at the sole value sink | single escaping call, value never re-inspected |
| V2 | Attr-name new-attribute / handler injection | `SafeAttrName` parse (charset) | only constructor runs the policy; sinks take the type |
| V3 | `on*` / `srcdoc` script-name attrs | `is_dangerous_attr_name` inside `SafeAttrName` | name class unrepresentable through generic setter; events use typed channel |
| V4 | Tag / event-marker breakout | `is_safe_html_name` | element/marker dropped, not escaped |
| V5 | `javascript:`/`vbscript:`/`data:` URL | `url_scheme` parse → typed scheme → allowlist | scheme parsed into typed value; safe set is default-deny |
| V6 | Scriptable `data:image/svg+xml` | `is_inert_data_image` raster allowlist | only inert raster subtypes on media attrs |
| V7 | Inline-style `expression()` / mid-value `url(js:)` | `SafeCssValue` whole-string scan | dangerous value unrepresentable in a declaration |
| V8 | Style key-smuggled rule | `SafeCssPropertyName` charset gate | key charset excludes `: ; { }` |
| V9 | `<style>` `</style>` breakout | `strip_style_close` loop-until-stable | body neutralised before becoming `HRaw` |
| V10 | Css `*Raw`/`property`/selector free-string | shared `SafeCssValue` + selector gate + `strip_style_close` | escape hatches routed through the same gate |
| V11 | SSE patch bypass of first-paint render | `safe_patch_attr` (`SafeAttrName` + `sanitise_url_attr`) | patch path shares the render policy |

Every drop/neutralise is an explicit, documented arm — no `_ =>` wildcard
swallow in any emit or constrain match; a disallowed attribute, scheme, or CSS
value is a fail-closed drop, not a silent fall-through.

---

## 5. Non-regression invariants to preserve

- Typed errors only (`Result Error a` / `Task Error a`) in any new kernel
  surface; no stringly errors.
- Record-field enumeration ordered by field index where any Css/attribute record
  is emitted.
- Explicit walker arms for any new AST/kernel node; no `_ -> []` catchall.
- Std.Ui HTML-escapes everything; `data-sky-eval` never emitted, never an
  accepted attribute name.
- Byte-parity with the Go renderer on benign inputs remains the equivalence
  gate; any intentional divergence is recorded, not silent.

---

## 6. Open decisions for the user

1. **Reject vs neutralise for a bad URL scheme.** The current sink
   **neutralises** `href="javascript:…"` to `href=""` (silent, matches Sky's Go
   runtime). Alternative: fail-closed with a compile/emit-time diagnostic when a
   *literal* dangerous scheme is present in source (dynamic values still
   neutralise at runtime). Recommendation: keep runtime neutralise for parity;
   optionally add a lint-level diagnostic for literal `javascript:` in `href`.
2. **Does the generic `attribute k v` escape hatch ship pre-push at all?** It is
   safe as specified (name parsed into `SafeAttrName`, value escaped, URL
   scheme-checked), but it is the widest surface. Option A: ship it (typed gate
   makes it safe). Option B: withhold pre-push and expose only the named
   builders (`class`/`id`/`href`/`type_`/`value`/`style`) + `boolAttribute`,
   adding `attribute` in v-next. Recommendation: ship it — the gate makes it
   safe by construction, and withholding it pushes users toward `HRaw`, which is
   strictly more dangerous.
3. **Selector grammar strictness for `Css.rule`/`media`.** A conservative
   allowlist (§Q4-4) rejects exotic-but-legitimate selectors (e.g. `:has()`
   with nested quotes). Decide whether to start strict (drop-on-doubt, safest)
   and widen with evidence, or start permissive and rely solely on
   `strip_style_close`. Recommendation: start strict; `strip_style_close` is
   defence-in-depth, not the primary gate.
4. **Whether `Std.Css.raw : String -> CssRule` ships pre-push.** It is a
   whole-rule verbatim hatch; even routed through `strip_style_close` it allows
   arbitrary (non-`</style>`) CSS. Recommendation: keep it, gate its body
   through `strip_style_close`, and document it as trusted-author-only (the
   `HRaw` analogue for CSS).
