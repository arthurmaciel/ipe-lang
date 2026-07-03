# Implementation Plan — CSS + HTML-attribute injection-safe emission (#46 / #47 / F7)

**Design source (do not re-decide):** `docs/architecture/css-attr-injection-safe-emit.md` (GO'd).
This plan turns that design into bite-sized, test-first tasks grounded in the
actual runtime + backend code. It does **not** redesign; where the design left an
open decision, the resolved answer is pinned in Global Constraints below.

---

## Global Constraints

### Principle order (non-negotiable, top wins)
`security > correctness > soundness > efficiency > completeness > readability`.
**F7 is a SECURITY task and outranks the convenience of #46/#47.** No attribute
or CSS ergonomics may open an injection vector; a feature is dropped before a
sink is weakened.

### The two governing rules (apply to every new sink in this surface)
1. **Parse, don't validate.** An untrusted string crosses each sink boundary
   *once*, is parsed into a typed value that carries the proof of safety in its
   structure (`SafeAttrName`, `SafeCssValue`, `SafeCssPropertyName`,
   `SafeCssSelector`), and is never re-inspected downstream.
2. **Make invalid states unrepresentable.** A dangerous attribute name, URL
   scheme, or `</style>`-bearing CSS body must not be *representable* at the emit
   sink — not merely rejected by a check a future refactor could forget. Every
   emit sink consumes the Safe* type, so it cannot skip the policy.

### Pinned decisions (from the design's §6 open list — already resolved)
- **URL scheme:** runtime **neutralise-to-empty** (Go parity), not fail-closed
  diagnostic. Keep `sanitise_url_attr`'s existing behaviour; add no compile-time
  lint in this surface.
- **Generic `attribute k v` / `boolAttribute k b`:** **ship it.** It is safe by
  construction (name parsed into `SafeAttrName`, value `escape_attr`'d, URL
  scheme-checked). Withholding it would push authors to `HRaw`, which is strictly
  more dangerous.
- **Selector grammar (`Css.rule` / `Css.media`):** **start strict, drop-on-doubt.**
  `strip_style_close` is defence-in-depth, not the primary gate.
- **`Std.Css.raw : String -> CssRule` (whole-rule verbatim hatch):** **DEFERRED —
  do NOT ship it in this surface.** `CssRaw` must not reach the DOM as an
  un-gated verbatim fragment. Until a trusted-author story lands, route any
  surviving `CssRaw` body through `strip_style_close` like every other `<style>`
  body, and do not expose `Css.raw` as a lowered kernel entry.

### Non-regression invariants (must still hold at the end)
- Typed errors only (`Result Error a` / `Task Error a`) in any new kernel surface
  — no stringly errors.
- **Explicit walker arms** for every new AST/kernel node in
  `Canonicalise`/`constrain`/`lower`/`emit`/`naming`; **no `_ => []` / `_ =>`
  catch-all** — a disallowed name/scheme/value is a *documented, explicit drop*.
- Record-field enumeration ordered by field index anywhere a Css/attribute record
  is emitted.
- `data-sky-eval` is never emitted and never an accepted attribute name.
- Byte-parity with the Go renderer on **benign** inputs remains the equivalence
  gate; any intentional divergence (e.g. `strip_style_close` being stronger than
  Go) is recorded in a code comment, never silent.

### Public-artifact rule
This surface mirrors Sky's "HTML-escape-everything / `data-sky-eval` forbidden"
posture. Comments and docs state the security property positively; no
disparagement of the Go backend — its `html.EscapeString` render is the parity
oracle, and where we are stronger (`strip_style_close`) we say *why*, not that Go
is wrong.

### Parallel-safety with concurrent workstreams
- **exit0 registry migration** touches `canon`/`sky_types` registry keys. The
  only `sky_types` edits here are **additive constrain arms** in
  `crates/sky_types/src/constrain.rs` alongside the existing Ui/Event arms
  (`constrain.rs:3654+`). Land them as new match arms on disjoint line ranges —
  no shared lines with the registry-key migration.
- **#49 / TailCallOpt** also edits `crates/sky_backend_rust/src/emit_expr.rs`.
  Overlap is **additive only**: new `KernelFn::HtmlStyleNode` / `CssStylesheet` /
  `CssStyles` arms are *appended* to the existing kernel match (near the Html
  builder arms at `emit_expr.rs:1736-1889`), never interleaved with TCO logic
  (TCO operates on the lowered call graph, not these leaf kernel arms). Land the
  two `emit_expr.rs` edits as separate arms to avoid a merge conflict; there is
  no semantic interaction.

### Ground truth already verified (cite, don't re-discover)
- **The single render sink** is `render_html` → `render_into_ctx`
  (`runtime/src/sky_runtime/html.rs:152,177`). Attribute values pass through
  `escape_attr(sanitise_url_attr(k,v))` (`html.rs:339`); attribute *names* through
  `SafeAttrName::parse` (`html.rs:332,472`); tags/event-markers through
  `is_safe_html_name` (`html.rs:226,352`). SSE patches share the policy via
  `safe_patch_attr` (`html.rs:622`). These are **already hardened** — #46 adds no
  new value/name path.
- **`SafeAttrName::parse` already forbids `on*` and `srcdoc`** via
  `is_dangerous_attr_name` (`html.rs:459,476`); regressions
  `render_drops_event_handler_and_srcdoc_attrs` (`html.rs:~841`) and
  `attr_to_string_drops_event_handler_and_srcdoc` (`html.rs:~875`) already assert
  it. #46 is therefore a **wiring + regression** task, not new escaping.
- **The F7 hole (two facts):**
  1. `Std.Html.styleNode attrs css = HElement "style" attrs [ HRaw css ]`
     (`sky-out/.sky-stdlib/Std/Html.sky:454`). `HRaw` renders **verbatim**
     (`html.rs:208`), and the `<style>` element sets `raw_body = true` so text
     children also emit verbatim (`html.rs:401,407`). **No `strip_style_close`
     runs on this path.** A `</style><script>` in the CSS body breaks out.
  2. `styleNode` currently **mis-lowers to `KernelFn::HtmlNode`**
     (`lower.rs:4009`), an *arity-3 tag/attrs/children* kernel
     (`sky_kernels/src/lib.rs:1082`), whereas `styleNode` is *arity-2
     `(attrs, css:String)`*. This is a latent shape bug F7 corrects with a
     dedicated kernel.
- **The Std.Ui `<style>` path IS protected** by `strip_style_close`
  (`live/style_inject.rs:203`, used at `:226,227,250`). The gate exists; F7 makes
  the `styleNode`/`<style>` path share it.
- **`Std.Css` is fully dormant** — `Std/Css.sky` exists (1508 lines) but no `Css`
  kernel is registered, no `"Css"` arm exists in `lower.rs`/`constrain.rs`. #47
  ports it. Its renderers (`renderProp`/`renderRule`/`stylesheet`/`styles`,
  `Std/Css.sky:1461-1508`) are pure-Sky string concatenation with **no** gating.
- **Reuse targets already present:** `SafeCssValue` (`ui/render.rs:74`, parse at
  `:80`), `SafeCssPropertyName` (`ui/render.rs:37`, parse at `:44`),
  `strip_style_close` (`live/style_inject.rs:203`) — all currently **private**;
  Task 1 promotes them.

### Working rules for the executor
- Read-only first; then strict TDD (red → green) per task.
- **Per-crate, timeout-bounded** commands only. No full example/perf sweep
  locally (push and let CI do the heavy lifting).
  - Runtime unit tests: `timeout 600 cargo test -p sky-runtime-rust <filter>`
  - Backend/kernel build: `timeout 600 cargo build -p sky_kernels -p sky_backend_rust -p sky_lower -p sky_types`
  - Clippy (must stay green — this surface is `deny`-linted): `timeout 600 cargo clippy -p sky-runtime-rust`
  - Goldens (SKY_E2E-gated): `SKY_E2E=1 timeout 900 cargo test -p skyc --test <golden>`
- Never run `sky build` from the repo root.
- After every red step, run the exact test and paste the failure before writing
  the fix. Evidence before assertions.

---

## Task 1 — Promote the shared `css_safety` module (F7-a foundation)

**Goal.** One policy, one place. Move the three CSS/style encoders into a single
shared module so #46's `style` attribute, #47's `styleNode` body, and the
existing Std.Ui path import identical functions. Add the strict selector gate
here so #47 has it ready.

**Files**
- **New:** `runtime/src/sky_runtime/css_safety.rs`
- `runtime/src/sky_runtime/mod.rs` — add `pub mod css_safety;` (module list at
  `mod.rs:1-90`, alongside `pub mod ...`).
- `runtime/src/sky_runtime/ui/render.rs` — delete the local `SafeCssValue`
  (`:74`) and `SafeCssPropertyName` (`:37`); `use super::super::css_safety::{...}`.
- `runtime/src/sky_runtime/live/style_inject.rs` — delete local
  `strip_style_close` (`:203`); `use super::super::css_safety::strip_style_close;`.

**Interfaces (all `pub(crate)`; the parse constructors keep the parse-don't-validate shape)**
```rust
// runtime/src/sky_runtime/css_safety.rs

/// A validated CSS property name — SOLE constructor runs the charset policy once.
/// Accepts `[A-Za-z0-9-]` (vendor prefixes + `--custom`); rejects empty and any
/// byte outside it (closes the key-smuggled-rule vector `background:...;x`).
pub(crate) struct SafeCssPropertyName<'a>(&'a str);
impl<'a> SafeCssPropertyName<'a> {
    pub(crate) fn parse(k: &'a str) -> Option<Self>; // moved verbatim from ui/render.rs:44
    pub(crate) fn as_str(&self) -> &str;
}

/// A validated CSS declaration value — whole-string, case-folded, whitespace-
/// stripped scan. Rejects `; { } </ /* @import` and the script sinks
/// `expression( javascript: vbscript: url(javascript: url(data:text ...`.
pub(crate) struct SafeCssValue<'a>(&'a str);
impl<'a> SafeCssValue<'a> {
    pub(crate) fn parse(v: &'a str) -> Option<Self>; // moved verbatim from ui/render.rs:80
    pub(crate) fn as_str(&self) -> &str;
}

/// A validated CSS selector / media query (NEW — strict, drop-on-doubt).
/// Allowed: letters, digits, and the CSS structural set
/// `. # : - _ [ ] = " ' , > + ~ * ( ) space`. Rejects `{ } ; @ </ /*`
/// (the leading `@media`/`@keyframes` keyword is supplied by the renderer, never
/// by the user selector string). A selector that fails is dropped with its rule.
pub(crate) struct SafeCssSelector<'a>(&'a str);
impl<'a> SafeCssSelector<'a> {
    pub(crate) fn parse(sel: &'a str) -> Option<Self>;
    pub(crate) fn as_str(&self) -> &str;
}

/// Strip the `</style` close-tag from a raw CSS body before it becomes an HRaw
/// `<style>` child. Case-insensitive, fixpoint-iterated (defeats the
/// `</sty</stylele` reconstruction trick). Total. Stronger-than-Go on purpose:
/// security outranks byte-for-byte parity (documented).
pub(crate) fn strip_style_close(s: &str) -> String; // moved verbatim from style_inject.rs:203
```

**Step 1 (RED) — write `css_safety.rs` unit tests first, before the move.**
Put a `#[cfg(test)] mod tests` in the new module asserting the moved behaviour
plus the new selector gate:
```rust
#[test] fn value_rejects_expression_and_scheme_sinks() {
    assert!(SafeCssValue::parse("expression(alert(1))").is_none());
    assert!(SafeCssValue::parse("0; background:url(javascript:alert(1))").is_none());
    assert!(SafeCssValue::parse("url( javascript:alert(1))").is_none()); // ws-stripped
    assert!(SafeCssValue::parse("#ff6600").is_some());                    // benign passes
}
#[test] fn propname_rejects_key_smuggle() {
    assert!(SafeCssPropertyName::parse("background:url(x);x").is_none());
    assert!(SafeCssPropertyName::parse("--brand").is_some());
    assert!(SafeCssPropertyName::parse("-webkit-box-shadow").is_some());
}
#[test] fn selector_strict_drops_breakout() {
    assert!(SafeCssSelector::parse("body{}</style><script>").is_none());
    assert!(SafeCssSelector::parse("@import url(x)").is_none());
    assert!(SafeCssSelector::parse(".card:hover > a[href^=\"/\"]").is_some());
}
#[test] fn strip_style_close_is_fixpoint_and_case_insensitive() {
    assert!(!strip_style_close("a{}</StYlE ><script>").to_ascii_lowercase().contains("</style"));
    assert!(!strip_style_close("</sty</stylele").to_ascii_lowercase().contains("</style"));
}
```
Run: `timeout 600 cargo test -p sky-runtime-rust css_safety` → **fails to compile**
(module absent). That is the red state.

**Step 2 (GREEN) — create the module and move.**
- Create `css_safety.rs`; move `SafeCssValue`/`SafeCssPropertyName` bodies from
  `ui/render.rs` and `strip_style_close` from `style_inject.rs` **verbatim**
  (they already carry their security rationale comments — keep them). Change
  visibility to `pub(crate)`.
- Implement `SafeCssSelector::parse` per the interface (strict allowlist).
- Add `pub mod css_safety;` to `mod.rs`.
- Replace the two `ui/render.rs` structs and the `style_inject.rs` fn with `use`
  imports; delete the originals so there is exactly one definition.

**Step 3 (VERIFY).**
```
timeout 600 cargo test -p sky-runtime-rust css_safety
timeout 600 cargo test -p sky-runtime-rust ui::render        # existing Std.Ui CSS tests still green
timeout 600 cargo test -p sky-runtime-rust style_inject      # existing mq/pc tests still green
timeout 600 cargo clippy -p sky-runtime-rust
```
**Acceptance:** all four green; `rg -n "fn strip_style_close|struct SafeCssValue|struct SafeCssPropertyName" runtime/src` shows **one** definition each, in `css_safety.rs`.

---

## Task 2 — F7 core: `styleNode` kernel + `<style>`-sink neutralisation

**Goal.** Close the one producer that bypasses the sink. Give `styleNode` a
dedicated kernel that pre-strips its body, AND harden the render sink so *any*
`<style>` element (including a hand-built `Html.node "style" [] [Html.raw css]`)
is neutralised — defence in depth. This also fixes the arity mis-wire
(`styleNode` → `HtmlNode`) found in `lower.rs:4009`.

**Files**
- `runtime/src/sky_runtime/html.rs` — new `pub fn html_style_node_`; harden the
  `<style>` child-emission in `render_into_ctx`.
- `crates/sky_kernels/src/lib.rs` — add `KernelFn::HtmlStyleNode`, register
  `d("Html", "styleNode", 2, Ui, "html_style_node_")` (near `:1080-1089`).
- `crates/sky_lower/src/lower.rs` — split `styleNode` out of the `HtmlNode` arm
  (`:4009`) into its own `("Html","styleNode") => Ok(Callee::Kernel(KernelFn::HtmlStyleNode))`.
- `crates/sky_types/src/constrain.rs` — additive arm
  `(Some("Html"), Some("styleNode"))` → `fun(list(attr(var(0))), fun(string, html(var(0))))`.
- `crates/sky_backend_rust/src/emit_expr.rs` — additive `KernelFn::HtmlStyleNode`
  arm (near the Html builder arms `:1736-1889`).
- `crates/sky_backend_rust/src/naming.rs` — `KernelFn::HtmlStyleNode => "html_style_node_"`.
- `crates/sky_ir/src/pretty.rs` — `KernelFn::HtmlStyleNode => "Html.styleNode"`.

**Interface (runtime)**
```rust
// html.rs — styleNode bakes the F7 fix into construction (parse-don't-validate:
// the css string is neutralised once, here, and the HRaw it produces is already safe).
pub fn html_style_node_<M>(attrs: Vec<Attribute<M>>, css: String) -> Html<M> {
    Html::HElement(
        "style".to_string(),
        attrs,
        vec![Html::HRaw(crate::sky_runtime::css_safety::strip_style_close(&css))],
    )
}
```

**Sink hardening (defence in depth).** In `render_into_ctx`, the `<style>` branch
(`html.rs:398-409`) currently emits `raw_body` children verbatim. Change it so
that when `tag == "style"` the children are rendered into a scratch buffer and
`strip_style_close` is applied before the buffer is pushed to `s`:
```rust
// html.rs, replacing the raw_body child loop for the style case only.
let raw_body = tag == "script" || tag == "style";
// ... select_value unchanged ...
if tag == "style" {
    let mut body = String::new();
    for c in kids {
        render_into_ctx(c, &mut body, None, /*raw_text*/ true, depth.saturating_add(1));
    }
    s.push_str(&css_safety::strip_style_close(&body)); // neutralise every <style> body
} else {
    for c in kids { render_into_ctx(c, s, child_select_value, raw_body, depth.saturating_add(1)); }
}
```
Keep `<script>` verbatim (its parity contract is documented at `html.rs:191-207`
and is a separate, author-owned raw hatch — not in scope for F7). Add a code
comment stating this asymmetry and *why* (`<style>` bodies are attacker-reachable
via `Std.Css`; `<script>` is the documented Go-parity raw escape hatch).

**Step 1 (RED) — regressions 6/8/9 as `html.rs` unit tests.**
```rust
#[test] fn style_node_strips_close_tag_breakout() {           // design regression #6
    let node = html_style_node_::<()>(vec![], "body{color:red}</style><script>alert(1)</script>".into());
    let out = render_html(&node);
    assert!(!out.to_ascii_lowercase().contains("</style><script"));
    assert!(!out.contains("<script>alert(1)"));
}
#[test] fn hand_built_style_element_is_also_stripped() {       // defence-in-depth
    let node: Html<()> = Html::HElement("style".into(), vec![],
        vec![Html::HRaw("x{}</StYlE ><script>".into())]);
    assert!(!render_html(&node).to_ascii_lowercase().contains("</style"));
}
#[test] fn style_node_defeats_reconstruction_trick() {         // design regression #8
    let node = html_style_node_::<()>(vec![], "a{}</sty</stylele>".into());
    assert!(!render_html(&node).to_ascii_lowercase().contains("</style"));
}
```
Run: `timeout 600 cargo test -p sky-runtime-rust html::` → **fails** (function
absent / sink still verbatim).

**Step 2 (GREEN).** Add `html_style_node_`, harden the sink, then wire the
backend (kernel registration, lower split, constrain arm, emit arm, naming,
pretty). Build the backend crates:
```
timeout 600 cargo build -p sky_kernels -p sky_lower -p sky_types -p sky_backend_rust
timeout 600 cargo test -p sky-runtime-rust html::
timeout 600 cargo clippy -p sky-runtime-rust
```

**Acceptance:** the three regressions green; `rg -n "styleNode" crates/sky_lower/src/lower.rs`
shows `styleNode` on its own `HtmlStyleNode` arm (no longer folded into `HtmlNode`);
no `_ =>` was added anywhere.

---

## Task 3 — #46: wire the attribute builders + injection regressions

**Goal.** Confirm every #46 builder flows through the (already hardened) sink and
lock it with regressions. The builders are pure Sky ADT constructors
(`Std/Html/Attributes.sky`: `class:55`, `id`, `type_`, `value`, `href`, `src`,
`style:214`, generic `attribute:347`, `boolAttribute:359`) that build
`Attribute::Attr` / `BoolAttr` (`UiCtor::HtmlAttribute`, `sky_ir/src/ir.rs:590`).
No new escaping.

**Files**
- Verify only (no expected change): `crates/sky_lower/src/lower.rs` (Attr/BoolAttr
  ctor lowering), `crates/sky_backend_rust/src/emit_types.rs:151`
  (`UiCtor::HtmlAttribute → sky_runtime::html::Attribute<{m}>`).
- `runtime/src/sky_runtime/html.rs` `#[cfg(test)]` — regressions 1–5, 10.
- **If** a build in Step 1 shows the generic `attribute`/`boolAttribute` ctor path
  does not lower (dormant), add the minimal ctor wiring in `lower.rs`/`constrain.rs`
  as an additive arm — same shape as the existing named builders. (Expected: no
  change needed; the named builders and the generic setter share the `Attr`/
  `BoolAttr` constructor, which already lowers for `class`/`href`.)

**Step 1 (RED) — regressions as `html.rs` unit tests.** These build the runtime
`Attribute`/`Html` values directly and assert neutralisation at `render_html`:
```rust
#[test] fn attr_value_quote_breakout_is_escaped() {                       // #1
    let n: Html<()> = Html::HElement("div".into(),
        vec![Attribute::Attr("title".into(), "x\" onmouseover=\"alert(1)".into())], vec![]);
    let out = render_html(&n);
    assert!(out.contains("onmouseover=&#34;"));      // escaped, inert
    assert!(!out.contains("onmouseover=\""));         // no live handler
}
#[test] fn javascript_href_is_neutralised() {                             // #2
    let n: Html<()> = Html::HElement("a".into(),
        vec![Attribute::Attr("href".into(), "javascript:alert(1)".into())], vec![]);
    let out = render_html(&n);
    assert!(out.contains("href=\"\""));
    assert!(!out.to_ascii_lowercase().contains("javascript:"));
}
#[test] fn data_uri_src_policy() {                                        // #3
    let bad: Html<()> = Html::HElement("img".into(),
        vec![Attribute::Attr("src".into(), "data:text/html,<script>1</script>".into())], vec![]);
    assert!(render_html(&bad).contains("src=\"\""));
    let ok: Html<()> = Html::HElement("img".into(),
        vec![Attribute::Attr("src".into(), "data:image/png;base64,iVBOR".into())], vec![]);
    assert!(render_html(&ok).contains("data:image/png;base64,iVBOR")); // inert raster passes (Go parity)
}
#[test] fn generic_attribute_drops_event_and_srcdoc_names() {             // #4
    for k in ["onclick", "OnClick", "onfoo", "srcdoc"] {
        let n: Html<()> = Html::HElement("div".into(),
            vec![Attribute::Attr(k.into(), "x".into())], vec![]);
        let out = render_html(&n).to_ascii_lowercase();
        assert!(!out.contains(&format!("{}=", k.to_ascii_lowercase())));
    }
}
#[test] fn unsafe_tag_name_drops_element() {                              // #5
    let n: Html<()> = Html::HElement("div><script>".into(), vec![], vec![]);
    assert!(!render_html(&n).contains("<script"));
}
#[test] fn sse_patch_shares_the_policy() {                                // #10
    assert!(safe_patch_attr("onclick", "x").is_none());
    let (_, v) = safe_patch_attr("href", "javascript:alert(1)").unwrap();
    assert!(!v.to_ascii_lowercase().contains("javascript:"));
}
```
Run: `timeout 600 cargo test -p sky-runtime-rust html::` — regressions 1–3, 5, 10
should already **pass** (sink hardened); #4 extends the existing
`render_drops_event_handler_and_srcdoc_attrs` coverage. If any fails, that is a
real gate hole — fix at the sink, not the test.

**Step 2 (GREEN, wiring proof).** Add one end-to-end golden fixture proving the
pure-Sky builders reach the sink: `tests/golden/attr_builders/Main.sky` renders
`Html.a [ Attr.href "javascript:alert(1)", Attr.class "a b", Attr.attribute "onclick" "x" ] [...]`
via `Html.render`. Golden `.rs` in `crates/skyc/tests/golden_m7_attr_builders.rs`
(mirror `golden_m7_stdui_onclick.rs`), SKY_E2E-gated, asserting `class="a b"`
present, `javascript:`/`onclick=` absent. (Set `oracle_divergence = true` if the
Go reference does not expose `Html.render` on this shape — follow the
`golden_m7_stdui_onclick.rs` provenance note.)
```
SKY_E2E=1 timeout 900 cargo test -p skyc --test golden_m7_attr_builders
```

**Acceptance:** regressions 1–5, 10 green; wiring golden green; no sink weakened;
generic `attribute`/`boolAttribute` demonstrably safe end-to-end.

---

## Task 4 — #47: port `Std.Css` (typed constructors + gated escape hatches)

**Goal.** Wire the dormant `Std.Css` so `styleNode`/`stylesheet`/`rule`/`px`/`rem`/
`hex`/`rgb` work, with every free-string entry point routed through the shared
`css_safety` gate. Mirror the Std.Ui precedent: the renderers that emit user
strings into CSS become **Rust kernels** that walk the `CssRule`/`CssProp` ADTs
and apply the gates (`ui/render.rs:build_style_string:160` is the template).
Typed constructors (`px`/`rem`/`hex`/`rgb`/`rgba`/`hsl`) stay **pure Sky** — they
stringify to digits and cannot express injection.

**Design boundary (kernelize only the sinks).**
- **Pure Sky (unchanged, safe by construction):** `px`/`rem`/`em`/`pct`/`vh`/`vw`,
  `rgb`/`rgba`/`hsl`/`hsla`/`hex`, and `lengthToString`/`colorToString`
  (`Std/Css.sky:322-460`). These produce already-safe strings.
- **Rust kernels (apply the gate):** `stylesheet : List CssRule -> String` and
  `styles : List CssProp -> String` (`Std/Css.sky:1500,1506`) — the two functions
  that fold user strings (`CssProp k v`, `CssRule selector props`, `LenRaw`,
  `ColorRaw`, `CssRaw`) into the final CSS. Porting these to Rust is the
  parse-don't-validate boundary.
- **`CssRaw` (from deferred `Css.raw`):** its body is routed through
  `strip_style_close` in the kernel and **no `Css.raw` lower entry is added**
  (deferred per Global Constraints).

**Files**
- **New:** `runtime/src/sky_runtime/css.rs` — runtime reflections of the Css ADTs
  + the two gated renderers. Register `pub mod css;` in `mod.rs`.
- `crates/sky_kernels/src/lib.rs` — `KernelFn::CssStylesheet` /
  `KernelFn::CssStyles`; `d("Css","stylesheet",1,Ui,"css_stylesheet_")`,
  `d("Css","styles",1,Ui,"css_styles_")`.
- `crates/sky_canon/src/env.rs` — register the `Css` module surface (mirror the
  `Html` surface list at `:815+`) so `Std.Css` names resolve.
- `crates/sky_lower/src/lower.rs` — `("Css","stylesheet")` / `("Css","styles")`
  arms; **no** `("Css","raw")` arm (deferred).
- `crates/sky_types/src/constrain.rs` — additive `(Some("Css"), …)` arms.
- `crates/sky_backend_rust/src/emit_expr.rs` — additive `CssStylesheet`/`CssStyles`
  arms (appended near Html arms; disjoint from #49).
- `crates/sky_backend_rust/src/{naming.rs,emit_types.rs}` + `sky_ir/src/pretty.rs`
  — names + any new `UiCtor` for the Css ADTs, if the ctor path needs one.

**Interface (runtime kernel — the gate lives here, once per declaration)**
```rust
// runtime/src/sky_runtime/css.rs
use crate::sky_runtime::css_safety::{SafeCssPropertyName, SafeCssValue, SafeCssSelector, strip_style_close};

pub enum CssProp { CssProp(String, String) }
pub enum CssRule {
    CssRule(String, Vec<CssProp>),      // selector { props }
    CssMedia(String, Vec<CssRule>),     // @media query { rules }
    CssKeyframes(String, Vec<String>),  // @keyframes name { frames }
    CssRaw(String),                     // deferred escape hatch — strip-only
}

/// Render one declaration, gated. Key AND value must both parse; else DROP the
/// declaration (explicit, documented — no `_ =>` swallow, no partial emit).
fn render_prop(p: &CssProp) -> Option<String> {
    let CssProp::CssProp(k, v) = p;
    let key = SafeCssPropertyName::parse(k)?;
    let val = SafeCssValue::parse(v)?;
    Some(format!("{}:{}", key.as_str(), val.as_str()))
}

fn render_rule(r: &CssRule) -> String {
    match r {
        CssRule::CssRule(sel, props) => match SafeCssSelector::parse(sel) {
            None => String::new(),                                  // drop rule + its props
            Some(sel) => { /* sel { declarations } from render_prop, dropping failures */ }
        },
        CssRule::CssMedia(q, rules) => match SafeCssSelector::parse(q) {
            None => String::new(),
            Some(q) => { /* @media <q> { render_rule* } */ }
        },
        CssRule::CssKeyframes(name, frames) => { /* name gated as selector; frames strip-only */ }
        CssRule::CssRaw(s) => strip_style_close(s),                 // deferred hatch: strip-only
    }
}

pub fn css_stylesheet_(rules: Vec<CssRule>) -> String {
    // assemble, then strip_style_close over the whole body (defence in depth)
    strip_style_close(&rules.iter().map(render_rule).collect::<String>())
}
pub fn css_styles_(props: Vec<CssProp>) -> String {
    props.iter().filter_map(render_prop).collect::<Vec<_>>().join(";")
}
```
Note the exhaustive `match` on `CssRule` — every variant has an explicit arm
(walker-arm invariant). `styleNode` (Task 2) already `strip_style_close`s again at
the `<style>` sink, so a `stylesheet` string spliced into `styleNode` is gated
twice — belt and braces.

**Step 1 (RED) — #47 regressions (7, 6-value, 9) as `css.rs` unit tests.**
```rust
#[test] fn property_expression_value_dropped() {                          // #7
    let s = css_styles_(vec![CssProp::CssProp("width".into(), "expression(alert(1))".into())]);
    assert!(s.is_empty());
}
#[test] fn property_midvalue_scheme_dropped() {                           // #7
    let s = css_styles_(vec![CssProp::CssProp("background".into(),
        "0; background:url(javascript:alert(1))".into())]);
    assert!(s.is_empty());
}
#[test] fn property_close_tag_in_value_dropped_and_stripped() {           // #6
    let sheet = css_stylesheet_(vec![CssRule::CssRule("body".into(),
        vec![CssProp::CssProp("color".into(), "red</style><script>alert(1)</script>".into())])]);
    assert!(!sheet.to_ascii_lowercase().contains("</style"));
    assert!(!sheet.contains("<script>alert(1)"));
    // the raw `;`/`</`-bearing value never renders as a declaration:
    assert!(!sheet.contains("red</style"));
}
#[test] fn selector_breakout_drops_rule() {                               // #9
    let sheet = css_stylesheet_(vec![CssRule::CssRule("body{}</style><script>".into(),
        vec![CssProp::CssProp("color".into(), "red".into())])]);
    assert!(sheet.trim().is_empty() || !sheet.to_ascii_lowercase().contains("<script"));
}
#[test] fn benign_stylesheet_go_parity_shape() {                          // Go-parity benign
    let sheet = css_stylesheet_(vec![CssRule::CssRule(".card".into(),
        vec![CssProp::CssProp("color".into(), "#ff6600".into())])]);
    assert!(sheet.contains(".card") && sheet.contains("#ff6600"));
}
```
Run: `timeout 600 cargo test -p sky-runtime-rust css::` → **fails** (module
absent). Red.

**Step 2 (GREEN).** Implement `css.rs`, wire canon/lower/constrain/emit/naming.
Keep the pure-Sky typed constructors as-is in `Std/Css.sky`; ensure `stylesheet`/
`styles` resolve to the new kernels (their pure-Sky bodies become unreachable
kernel aliases, same pattern as other `Ffi.kernel` surfaces).
```
timeout 600 cargo build -p sky_kernels -p sky_canon -p sky_lower -p sky_types -p sky_backend_rust
timeout 600 cargo test -p sky-runtime-rust css::
timeout 600 cargo clippy -p sky-runtime-rust
```

**Step 3 — Go-parity benign golden.** `tests/golden/m7_css/Main.sky`:
`Html.render (Html.styleNode [] (Css.stylesheet [ Css.rule ".card" [ Css.property "color" (Css.colorToString (Css.rgb 255 102 0)) ] ]))`
plus `Css.px 100`, `Css.hex "ff6600"`. Golden `.rs`
`crates/skyc/tests/golden_m7_css.rs` (mirror the stdui goldens), asserting the
benign byte-shape (`.card`, `100px`, `#ff6600`, `rgb(255,102,0)`) and absence of
any injected substring. SKY_E2E-gated.
```
SKY_E2E=1 timeout 900 cargo test -p skyc --test golden_m7_css
```

**Acceptance:** regressions 6/7/9 green; benign parity green; `rg -n '"Css","raw"' crates/`
returns nothing (deferred); every `CssRule` arm explicit.

---

## Task 5 — Port the ../sky fixtures as stored-HTML snapshot regressions

**Goal.** Port `../sky/runtime-rust/tests/sky/{69-html-render-parity,
70-style-injection,71-style-merge}` into this repo as durable snapshot
regressions that assert neutralisation on the injection cases and Go-parity on
the benign cases. (In `../sky` these are example projects driven by the sweep; the
equivalent durable artefacts here are runtime render snapshots + SKY_E2E goldens.)

**Files**
- `runtime/src/sky_runtime/html.rs` (or a new `runtime/src/sky_runtime/html_snapshot_tests.rs`
  wired via `#[cfg(test)] mod`) — snapshot tests that build the equivalent
  `Html`/`Css` trees in Rust and assert on `render_html`. Fast, no compiler.
- `tests/golden/{html_render_parity,style_injection,style_merge}/Main.sky` +
  `crates/skyc/tests/golden_html_render_parity.rs` etc. — SKY_E2E-gated
  end-to-end ports of the three `Main.sky` files (copy from `../sky`, adjust to
  local stdlib surface), for the Rust≡Go equivalence gate.

**Snapshot assertions to encode (each MUST hold in emitted output):**
- **69 — html-render-parity:** `<select value="b">` flips `selected` onto the
  matching `<option>` (assert `selected` on the `b` option, not `a`); `<script>`
  child emits verbatim (`if (1 < 2)` un-escaped); ordinary text child stays
  entity-escaped (`&lt;b&gt;raw&lt;/b&gt;`); `Html.doctype` → literal
  `<!DOCTYPE html>`. (These exercise the existing render sink — Go parity.)
- **70 — style-injection:** the Std.Ui pseudo-class/media markers produce
  sky-id-scoped `<style>` blocks, AND the raw media-query breakout probe
  `"(min-width: 1px) </style><script>alert(1)</script>"` is neutralised —
  assert the emitted `<style>` body contains **no** `</style>` and **no**
  `<script>` (this is the `strip_style_close` path Task 1/2 unified).
- **71 — style-merge:** an element carrying a computed inline `style`
  (padding+background) AND a user `Ui.htmlAttribute "style" "z-index: 5"` emits a
  **single** merged `style="…; z-index: 5"` (assert exactly one `style="`
  occurrence and that `z-index: 5` survives). Guards the merge logic at
  `html.rs:253-297`.

**Step 1 (RED).** Write the snapshot tests referencing helpers/values that do not
yet exist for 70 (the injection assertion depends on Task 2's sink strip). Run
`timeout 600 cargo test -p sky-runtime-rust snapshot` and confirm the 70 breakout
assertion is the one that gates on Task 2.

**Step 2 (GREEN).** With Tasks 1–4 landed, the assertions pass. Add the three
SKY_E2E goldens (copy `Main.sky`, retarget to the local stdlib; set
`oracle_divergence` per the existing goldens' provenance convention where the Go
reference does not expose the render entry).
```
timeout 600 cargo test -p sky-runtime-rust snapshot
SKY_E2E=1 timeout 900 cargo test -p skyc --test golden_html_render_parity --test golden_style_injection --test golden_style_merge
```

**Acceptance:** all snapshot assertions green; the `</style><script>` probe in 70
is provably absent from output; 71 shows exactly one `style="` attribute.

---

## Task 6 — Guardian self-verification sweep (blocking gate)

**Goal.** Prove the surface didn't reintroduce a banned pattern and that every
drop is explicit.

**Steps**
1. **Re-grep the bans over the diff:**
   ```
   rg -n '\b_ =>' crates/sky_backend_rust/src/emit_expr.rs crates/sky_types/src/constrain.rs crates/sky_lower/src/lower.rs
   rg -n 'unwrap\(|expect\(|panic!|unreachable!|todo!|unimplemented!' runtime/src/sky_runtime/css.rs runtime/src/sky_runtime/css_safety.rs
   rg -n 'data-sky-eval' runtime/src/sky_runtime
   rg -n 'Result String|Task String' crates/ runtime/src   # typed-error invariant
   ```
   Any hit in the new code is a finding to fix (a test `unwrap` is fine; a
   non-test one is not). New match arms must be explicit variants, never `_ =>`.
2. **One-definition check** for the shared encoders:
   `rg -n 'fn strip_style_close|struct SafeCssValue|struct SafeCssPropertyName|struct SafeCssSelector' runtime/src`
   → each exactly once, in `css_safety.rs`.
3. **Full per-crate green:**
   ```
   timeout 600 cargo test -p sky-runtime-rust
   timeout 600 cargo clippy -p sky-runtime-rust
   timeout 600 cargo build -p sky_kernels -p sky_canon -p sky_lower -p sky_types -p sky_backend_rust
   SKY_E2E=1 timeout 900 cargo test -p skyc --test golden_m7_css --test golden_m7_attr_builders \
       --test golden_html_render_parity --test golden_style_injection --test golden_style_merge
   ```
4. **Injection-vector ledger check.** Walk the design's §4 ledger (V1–V11) and
   confirm each vector has at least one green regression in this surface:
   V1 `attr_value_quote_breakout_is_escaped`, V2/V3 `generic_attribute_drops_*`,
   V4 `unsafe_tag_name_drops_element`, V5/V6 `javascript_href` + `data_uri_src`,
   V7 `property_expression_value_dropped` + `property_midvalue_scheme_dropped`,
   V8 `propname_rejects_key_smuggle`, V9 `style_node_*` + reconstruction,
   V10 `selector_breakout_drops_rule` + `property_close_tag_in_value_*`,
   V11 `sse_patch_shares_the_policy`.
5. **Clean up** background processes; confirm no orphan cargo/test loops before
   declaring done.

**Acceptance:** all commands green; every V1–V11 has a named passing test; no
banned pattern in new non-test code; exactly one definition of each shared
encoder. Only then is F7 satisfied and #46/#47 ready to push.

---

## Task dependency graph
```
Task 1 (css_safety) ──▶ Task 2 (styleNode + sink)   ──┐
                    └──▶ Task 4 (Std.Css port)        ─┤
Task 3 (#46 regressions) ── independent (sink already hardened) ─┤
                                                                  ├──▶ Task 5 (fixture snapshots) ──▶ Task 6 (guardian sweep)
```
Task 1 is the strict prerequisite for 2 and 4. Task 3 can run in parallel with
1/2/4. Task 5 needs 2 (70 probe) + 4 (Css path) + 3 (attr path). Task 6 is last.
```
```
