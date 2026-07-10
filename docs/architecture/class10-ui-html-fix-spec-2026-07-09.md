# Class 10 — UI/HTML rendering + event sinks: implementation spec

> Scope: backlog #113, #105, #109, #156 (per
> `docs/architecture/campaign-classification-2026-07-09.md` Class 10).
> Read-only research completed 2026-07-09 against current `master`. Every
> line number below was re-verified by direct file reads — the backlog's
> original line citations (written against an older tree) have drifted and
> are superseded by the numbers in this doc.
>
> **Already fixed, no action needed (confirmed this session):** the
> `escape_text`/`escape_attr` split in `runtime/src/sky_runtime/html.rs`
> (lines 439-455) already matches Go's `html.EscapeString` byte-for-byte —
> `escape_text` (text nodes: `& < > '`) is correctly separated from
> `escape_attr` (`escape_text(t).replace('"', "&#34;")`, attribute-value
> sinks). Do not re-touch this.

## Summary of findings

| Item | Real bug confirmed? | Blast radius | Depends on |
|---|---|---|---|
| #113 pseudo-class no-op | Yes — worse than the backlog title suggests: broken in **every** backend (Live, Webview, and any bare `render_html` caller), not just a "static" sink | `runtime/src/sky_runtime/ui/render.rs` only | none |
| #105 Std.Css hardening | Yes — two independent, narrow gaps | `crates/skyc/stdlib/Std/Css.sky` (part 1), `runtime/src/sky_runtime/css_safety.rs` (part 2) | none |
| #109 / #156 onSubmit no-op + dyn Any | Yes — confirmed **100% non-functional at runtime today**: `Ui.onSubmit` / `Std.Html.Events.onSubmit` never dispatch a Msg on submit, in any example that uses them (19-skyforum, 37-composite-live-shop, 12-skyvote, 27-multi-session-chat, 28-streaming-chat all call it) | `runtime/src/sky_runtime/html.rs`, `runtime/src/sky_runtime/ui/helpers.rs`, doc-only touch-ups in `crates/sky_kernels/src/lib.rs`, `crates/sky_types/src/constrain.rs`, `crates/sky_backend_rust/src/emit_expr.rs` | see §4.6 sequencing note vs. Class 5 |

---

## 1. #113 — pseudo-class attrs render to nothing

### 1.1 Root cause

`Std.Ui`'s pseudo-class sugar (`Background.hoverColor` / `Border.hoverColor`
/ `Font.hoverColor` / `Ui.onPseudo`, etc.) is implemented as native Rust
kernels (not compiled Sky source — `runtime/src/sky_runtime/ui/render.rs`
is a hand-written "Phase 0" `Element<M> → Html<M>` render kernel). Each
pseudo helper constructs `ui::element::Attribute::AttrPseudoRule(PseudoClass,
String)` (see `runtime/src/sky_runtime/ui/helpers.rs:660-690` and
`:709-...` for the `Background.*` / `Border.*` builders).

The render pipeline has exactly ONE place that turns `Attribute<M>` into
`html::Attribute<M>`: `render.rs`'s `build_style_string` (style-only attrs)
and `collect_html_attrs` (everything else). Both currently **drop**
`AttrPseudoRule` outright:

- `build_style_string` (`render.rs:261-267`): correctly excludes it from the
  style string (it isn't a plain CSS declaration) — this part is fine.
- `collect_html_attrs` (`render.rs:284-326`): has match arms for
  `AttrClass` / `AttrAttribute` / `AttrEvent` / `AttrDescribe`, then a
  catch-all `_ => {}` that silently swallows `AttrPseudoRule` (and
  `AttrNearby`, which is handled separately by `render_nearby_overlays` —
  that part is fine too).

So `AttrPseudoRule` never becomes a `data-sky-pc-rules` marker attribute on
the `Html` tree at all. This means the DOWNSTREAM consumer —
`runtime/src/sky_runtime/live/style_inject.rs`'s `apply_style_injections` /
`build_pc` (already correctly implemented, Go-parity-ported, well tested) —
never finds anything to convert into a `<style>` block, because the marker
it looks for was never produced. `apply_style_injections` IS called before
`render_html` in every real code path that has it available
(`runtime/src/sky_runtime/live/mod.rs:135,627,1232,1281`,
`runtime/src/sky_runtime/webview.rs:171`) — so this is not a "live vs.
static" split bug, it's a single missing encoder upstream of a correct,
already-wired consumer. **Every** `Ui.layout` output loses
`Background.hoverColor` / `Border.focusColor` / `Ui.onPseudo` etc. today,
in Sky.Live, Sky.Webview, and any bare `ui::render::ui_layout` +
`html::render_html` caller (e.g. Sky.Tui, which doesn't call
`apply_style_injections` at all and never will — Tui has no CSS pseudo-class
concept, so the marker just needs to not leak as an inert `data-*` attr
there, which it currently doesn't because it's dropped anyway; no behavior
change needed for Tui).

### 1.2 Wire format (already fixed by the reference; do not re-invent)

Confirmed against the reference Haskell/Go implementation
(`~/Documentos/comp/sky/sky-stdlib/Std/Ui.sky:1550-1585`) and the Rust
decoder that already exists (`style_inject.rs:209-251`,
`pseudo_selector_for_tag`):

- Tag mapping: `Hover → "h"`, `Focus → "f"`, `FocusVisible → "v"`,
  `Active → "a"`, `Disabled → "d"`.
- One entry: `"<tag>|<css>"` (`tag ++ "|" ++ css`, no forced whitespace).
- Multiple entries on one element: joined with `"||"`.
- Empty `css` entries are dropped (never emitted).
- The whole thing becomes ONE `html::Attribute::Attr("data-sky-pc-rules",
  encoded)` HTML attribute on the element.

### 1.3 Fix

**File: `runtime/src/sky_runtime/ui/element.rs`** — add the encode-direction
sibling of `style_inject.rs`'s `pseudo_selector_for_tag`, colocated with the
`PseudoClass` type definition (single source of truth for the wire tag):

```rust
impl PseudoClass {
    /// Stable wire tag for a pseudo-class — must stay in lock-step with
    /// `style_inject::pseudo_selector_for_tag`'s decode-direction mapping
    /// (`h`/`f`/`v`/`a`/`d`) and the reference Haskell/Go
    /// `pseudoClassTag`/`pseudoSelectorForTag`.
    pub(crate) fn wire_tag(self) -> &'static str {
        match self {
            PseudoClass::Hover => "h",
            PseudoClass::Focus => "f",
            PseudoClass::FocusVisible => "v",
            PseudoClass::Active => "a",
            PseudoClass::Disabled => "d",
        }
    }
}
```

**File: `runtime/src/sky_runtime/ui/render.rs`** — in `collect_html_attrs`,
harvest every `AttrPseudoRule` into one merged marker attribute. Add a
pre-pass before (or after) the existing per-attr loop:

```rust
fn collect_html_attrs<M: Clone>(attrs: &[Attribute<M>]) -> Vec<HtmlAttribute<M>> {
    let mut out: Vec<HtmlAttribute<M>> = Vec::new();

    // Pseudo-class rules (#113): harvest every AttrPseudoRule into ONE
    // `data-sky-pc-rules` marker, mirroring the reference's
    // `collectPseudoRules` + `encodePseudoRules` (Std.Ui.sky:1550-1585).
    // Empty-css entries are dropped so we don't pollute the wire; the
    // downstream `live::style_inject::apply_style_injections` pass (already
    // correct) consumes exactly this format.
    let pseudo_encoded: String = attrs
        .iter()
        .filter_map(|a| match a {
            Attribute::AttrPseudoRule(pc, css) if !css.is_empty() => {
                Some(format!("{}|{}", pc.wire_tag(), css))
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("||");
    if !pseudo_encoded.is_empty() {
        out.push(HtmlAttribute::Attr(
            "data-sky-pc-rules".to_owned(),
            pseudo_encoded,
        ));
    }

    for attr in attrs {
        match attr {
            // ... existing arms unchanged ...
        }
    }
    out
}
```

No other call site needs touching — `render_node_as`, `ui_layout`, and
`ui_layout_with_vecs` (the 3 callers of `collect_html_attrs`) all pick this
up automatically.

### 1.4 Regression tests

**Unconditional test (no `live` feature needed)** — add to
`runtime/src/sky_runtime/ui/render.rs`'s existing `#[cfg(test)] mod tests`
(next to `border_glow_renders_box_shadow` etc.):

```rust
#[test]
fn pseudo_rule_emits_data_sky_pc_rules_marker() {
    let attrs = vec![super::super::helpers::ui_bg_hover_color_(Color::Rgba(
        0, 92, 215, 1.0,
    ))];
    let elem: Element<TestMsg> = Element::Empty;
    let html = ui_layout(attrs, elem);
    let s = render_html(&html);
    assert!(
        s.contains(r#"data-sky-pc-rules="h|background-color:rgba(0,92,215,1)""#),
        "pseudo-class marker missing/malformed: {s}"
    );
}

#[test]
fn multiple_pseudo_rules_merge_into_one_marker() {
    let attrs = vec![
        super::super::helpers::ui_bg_hover_color_(Color::Rgba(255, 0, 0, 1.0)),
        // NB: `Border.focusColor` maps to `PseudoClass::FocusVisible` (wire
        // tag "v"), not `Focus` ("f") — confirmed at
        // `helpers.rs:719-725`. Don't swap this for a "f|..." expectation.
        super::super::helpers::ui_border_focus_color_(Color::Rgba(0, 0, 255, 1.0)),
    ];
    let elem: Element<TestMsg> = Element::Empty;
    let html = ui_layout(attrs, elem);
    let s = render_html(&html);
    assert!(s.contains("h|background-color:rgba(255,0,0,1)"), "{s}");
    assert!(s.contains("||"), "entries must be || joined: {s}");
    assert!(s.contains("v|border-color:rgba(0,0,255,1)"), "{s}");
}
```

(`ui_bg_hover_color_` / `ui_border_focus_color_` confirmed present at
`helpers.rs:661` / `:719-725` respectively — no need to re-grep before
landing.)

**End-to-end test (`#[cfg(feature = "live")]`)** — add to
`runtime/src/sky_runtime/live/style_inject.rs`'s existing test module,
proving the FULL pipeline (Rust `Attribute::AttrPseudoRule` → `Html` tree →
`apply_style_injections` → final rendered `<style>` + no leaked marker),
mirroring the reference's `live_pseudo_class_test.go`:

```rust
#[test]
fn end_to_end_ui_hover_color_renders_scoped_style_and_leaves_no_marker() {
    use crate::sky_runtime::ui::element::{Attribute as UiAttribute, Color, Element};
    use crate::sky_runtime::ui::render::ui_layout;

    let attrs = vec![crate::sky_runtime::ui::helpers::ui_bg_hover_color_(
        Color::Rgba(0, 92, 215, 1.0),
    )];
    let elem: Element<()> = Element::Empty;
    let mut html = ui_layout(attrs, elem);
    apply_style_injections(&mut html);
    let out = crate::sky_runtime::html::render_html(&html);

    assert!(!out.contains("data-sky-pc-rules"), "marker must not leak: {out}");
    assert!(out.contains("<style"), "expected an injected <style>: {out}");
    assert!(out.contains(":hover"), "{out}");
    assert!(out.contains("@media (hover: hover)"), "{out}");
    assert!(out.contains("background-color:rgba(0,92,215,1)"), "{out}");
}
```

### 1.5 Verification

```bash
cd /home/arthur/Documentos/comp/sky-rust
cargo test -p sky-runtime-rust ui::render:: --lib
cargo test -p sky-runtime-rust --features live style_inject:: --lib
cargo test -p sky-runtime-rust --features full --lib
cargo clippy -p sky-runtime-rust --features full -- -D warnings
```

Then a real-app smoke check (Std.Ui already exercises `Background.hoverColor`
/ `Ui.onPseudo` — check `examples/26-ui-showcase` and any composite-ui
example) via the project's normal `skyc`/build+run verification flow before
declaring the sweep green.

---

## 2. #105 — Std.Css hardening

Two independent, narrowly-scoped gaps. `Std.Css` is 100% pure Sky
(`crates/skyc/stdlib/Std/Css.sky`); the only Rust surface is the four
`Sky.Core.CssSafety` leaf kernels backing it
(`runtime/src/sky_runtime/css_safety.rs` + thin shims in
`runtime/src/sky_runtime/css.rs`).

### 2.1 Part 1 — `@import` / `expression(` gating on `raw` / `keyframes` bodies

**Current state** (`crates/skyc/stdlib/Std/Css.sky:1519-1543`):

```elm
keyframes : String -> List String -> CssRule
keyframes name frames =
    case safeSelector name of
        Just n ->
            CssKeyframes n (List.map stripStyleClose frames)

        Nothing ->
            CssRuleDropped


raw : String -> CssRule
raw s =
    CssRaw (stripStyleClose s)
```

Both apply ONLY the `</style` breakout floor (`stripStyleClose`) — no scan
for `@import` (CSS-level SSRF / stylesheet-injection vector: an `@import
url(https://attacker/…)` inside an author-controlled-but-templated raw
fragment fetches an attacker resource in the victim's browser context) or
`expression(` (legacy IE CSS-expression script sink — dead in modern
browsers but still worth closing per the module's own documented defence-
in-depth posture, `Css.sky:63-69`: "Do NOT pass untrusted input to `raw`;
… see follow-up: optional `@import` gating on `raw`").

**Fix — add a pure-Sky gate (no new Rust kernel; `Std.Css` stays 100% Sky).**
This is a REJECT (drop to `CssRuleDropped`), not a strip, matching the
existing `rule` / `media` drop-on-fail convention and distinct from
`stripStyleClose`'s floor semantics — "gating" per the backlog wording:

```elm
-- Defence-in-depth (#105): reject a raw/keyframes body that smuggles an
-- `@import` (stylesheet-injection / CSS-level SSRF — fetches an
-- attacker-controlled resource in the victim's browser) or a legacy
-- `expression(` script sink. `raw`/`keyframes` are documented
-- trusted-author escape hatches (only `stripStyleClose`'s breakout floor
-- applies otherwise); this closes the specific follow-up already flagged
-- in this module's own header comment. Case-insensitive substring check —
-- consistent with `SafeCssValue`'s policy in css_safety.rs.
containsDangerousCssConstruct : String -> Bool
containsDangerousCssConstruct s =
    let
        lower = String.toLower s
    in
    String.contains "@import" lower || String.contains "expression(" lower


keyframes : String -> List String -> CssRule
keyframes name frames =
    case safeSelector name of
        Just n ->
            let
                stripped = List.map stripStyleClose frames
            in
            if List.any containsDangerousCssConstruct stripped then
                CssRuleDropped

            else
                CssKeyframes n stripped

        Nothing ->
            CssRuleDropped


raw : String -> CssRule
raw s =
    let
        stripped = stripStyleClose s
    in
    if containsDangerousCssConstruct stripped then
        CssRuleDropped

    else
        CssRaw stripped
```

Update the module header comment (`Css.sky:63-69`) to drop the "see
follow-up" wording once this lands (it becomes the closed state, not a
pending one).

No changes needed anywhere else — `String.contains`/`String.toLower` are
existing `Sky.Core.String` stdlib functions, `CssRuleDropped` is an existing
constructor, `Css.sky` stays pure Sky (per its own documented "100% pure
Sky" invariant), and no `sky_kernels`/`constrain.rs`/`lower.rs`/
`emit_expr.rs` wiring is touched.

**Regression tests** — add to whichever Sky-level test surface currently
covers `Std.Css` (check `crates/skyc/tests/golden_css_source.rs` and
`crates/skyc/tests/spike_std_source.rs` for the existing harness shape; if
`Std.Css` has a `tests/Std/CssSpec.sky`-style fixture use that convention
instead). Cases to cover:

- `Css.raw "@import url(https://evil.example/x.css);"` → dropped
  (`renderRule` of the result renders to `""`).
- `Css.raw "EXPRESSION(alert(1))"` → dropped (case-insensitive).
- `Css.raw ".card { color: red; }"` → passes through unchanged (benign,
  no regression on legitimate use).
- `Css.keyframes "spin" ["0% { transform: rotate(0deg) }", "100% {
  transform: rotate(360deg) } @import url(x)"]` → whole rule dropped (ANY
  frame smuggling the construct drops the rule, matching `List.any`).
- `Css.keyframes "spin" ["0% { opacity: 0 }", "100% { opacity: 1 }"]` →
  passes through unchanged.

### 2.2 Part 2 — reject CSS-hex-escaped values in `safeValue`

**Root cause.** `SafeCssValue::parse` (`runtime/src/sky_runtime/
css_safety.rs:66-106`) lowercases the value and scans for breakout chars
(`; { } </ /* @import`) and script-sink keywords (`expression(`,
`javascript:`, etc.) as literal substrings. CSS Syntax Level 3 §4.3.7
defines a general escape mechanism: `\` followed by 1-6 hex digits (with
one optional trailing whitespace consumed as the escape's delimiter)
decodes to that Unicode code point ANYWHERE a CSS token is lexed —
including inside what looks like a bare keyword. So:

- `\65 xpression(alert(1))` decodes to `expression(alert(1))`.
- `\75 rl(\6a avascript:alert(1))` decodes to `url(javascript:alert(1))`.
- Even a breakout CHARACTER can be hidden: `\3b` decodes to `;`.

None of these literal substrings appear in the RAW string, so today's scan
lets all of them through. `SafeCssPropertyName` and `SafeCssSelector` are
NOT affected — both reject `\` outright via their charset allowlists (`\`
is not in `[A-Za-z0-9-]` nor in the selector's structural allowlist), so an
escape attempt there is already rejected by the existing charset gate.
Scope is precisely `SafeCssValue`, matching the backlog wording.

**Fix** (`runtime/src/sky_runtime/css_safety.rs`) — decode CSS escapes and
re-run the SAME dangerous-pattern scan against the decoded string. Refactor
the scan into a shared helper so the raw-value and decoded-value paths can't
drift:

```rust
/// Breakout / script-sink patterns for a CSS declaration value. Checked
/// against both the raw value (see `SafeCssValue::parse`) and the
/// CSS-escape-decoded value (`css_unescape`) — one list, one policy.
const BAD_VALUE_PATTERNS: &[&str] = &[
    "expression(",
    "javascript:",
    "vbscript:",
    "url(javascript:",
    "url('javascript:",
    "url(\"javascript:",
    "url(vbscript:",
    "url(data:text",
    "url(data:application",
];

/// True when `low` (already lowercased) carries a declaration/ruleset
/// breakout character or a script-sink keyword.
fn has_dangerous_css_pattern(low: &str) -> bool {
    if low.contains(';')
        || low.contains('{')
        || low.contains('}')
        || low.contains("</")
        || low.contains("/*")
        || low.contains("@import")
    {
        return true;
    }
    let low_nows: String = low.chars().filter(|c| !c.is_whitespace()).collect();
    BAD_VALUE_PATTERNS.iter().any(|bad| low_nows.contains(bad))
}

/// Decode CSS backslash escapes (CSS Syntax Level 3 §4.3.7) for DETECTION
/// purposes only — the decoded string is never emitted; `SafeCssValue`
/// keeps the caller's ORIGINAL string on success. A value that hides a
/// blocked keyword or breakout char behind a hex escape
/// (`\65 xpression(...)`, `\3b` for `;`) decodes to the literal form here,
/// so `has_dangerous_css_pattern` catches it on the second pass.
///
/// Best-effort / fail-closed, not a spec-complete CSS tokenizer: an escape
/// that decodes to an invalid Unicode scalar value is dropped rather than
/// reconstructed (never helps an attacker hide a keyword; erring toward
/// "scan sees less" is the safe direction for a detector, not a renderer).
fn css_unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let mut hex = String::new();
        while hex.len() < 6 {
            match chars.peek() {
                Some(h) if h.is_ascii_hexdigit() => {
                    hex.push(*h);
                    chars.next();
                }
                _ => break,
            }
        }
        if !hex.is_empty() {
            // One trailing whitespace char is consumed as the escape's
            // delimiter (CSS Syntax §4.3.7), if present.
            if matches!(chars.peek(), Some(w) if w.is_whitespace()) {
                chars.next();
            }
            if let Ok(cp) = u32::from_str_radix(&hex, 16) {
                if let Some(ch) = char::from_u32(cp) {
                    out.push(ch);
                }
            }
            continue;
        }
        // `\` followed by a non-hex char: CSS escapes that literal char.
        if let Some(next) = chars.next() {
            out.push(next);
        }
        // trailing lone backslash (no following char): dropped.
    }
    out
}

impl<'a> SafeCssValue<'a> {
    pub(crate) fn parse(v: &'a str) -> Option<Self> {
        let low = v.to_ascii_lowercase();
        if has_dangerous_css_pattern(&low) {
            return None;
        }
        // Defence-in-depth: decode CSS backslash escapes and re-scan so a
        // hex-escaped bypass of the check above is caught too.
        let decoded_low = css_unescape(&low);
        if has_dangerous_css_pattern(&decoded_low) {
            return None;
        }
        Some(SafeCssValue(v))
    }

    pub(crate) fn as_str(&self) -> &str {
        self.0
    }
}
```

**Regression tests** — add to `css_safety.rs`'s existing `#[cfg(test)] mod
tests`:

```rust
#[test]
fn value_rejects_hex_escaped_expression_and_scheme_sinks() {
    // `\65` = 'e' (space delimiter consumed) → decodes to
    // "expression(alert(1))".
    assert!(SafeCssValue::parse("\\65 xpression(alert(1))").is_none());
    // `\75`='u', `\6a`='j' → decodes to "url(javascript:alert(1))".
    assert!(SafeCssValue::parse("\\75 rl(\\6a avascript:alert(1))").is_none());
    // `\3b` = ';' (hex-escaped breakout char, not just a script-sink kw).
    assert!(SafeCssValue::parse("red\\3b  malicious").is_none());
    // benign values with no escapes still pass unchanged.
    assert_eq!(
        SafeCssValue::parse("#ff6600").map(|v| v.as_str().to_owned()),
        Some("#ff6600".to_owned())
    );
    assert_eq!(
        SafeCssValue::parse("rgba(0,0,0,0.2)").map(|v| v.as_str().to_owned()),
        Some("rgba(0,0,0,0.2)".to_owned())
    );
}
```

Also add the mirrored case to `runtime/src/sky_runtime/css.rs`'s test module
(`safe_value_drops_injection_keeps_benign`) so the `Css.safeValue` Sky-facing
kernel shim is covered too:

```rust
assert!(matches!(
    safe_value("\\65 xpression(alert(1))".into()),
    SkyMaybe::Nothing
));
```

### 2.3 Verification

```bash
cargo test -p sky-runtime-rust css_safety:: --lib
cargo test -p sky-runtime-rust --features config css:: --lib   # css.rs kernel shims
# Std.Css Sky-level regressions (adjust to whichever harness owns it):
cargo test -p skyc golden_css_source
cargo test -p skyc spike_std_source
cargo clippy -p sky-runtime-rust --features full -- -D warnings
```

---

## 3. #109 / #156 — `Ui.onSubmit`/`Std.Html.Events.onSubmit` runtime no-op + `dyn Any`

These two backlog items describe the SAME bug from two angles and should
land together: #109 is the visible symptom (onSubmit silently does
nothing + a doc comment claiming otherwise); #156 is the architectural
root cause (an undocumented `Arc<dyn Any>` exception to PRINCIPLES.md's
no-`dyn Any` rule) and asks for an either/or decision. This section makes
that call.

### 3.1 Confirmed: this is a live, severe correctness bug, not just stale docs

- `Ui.onSubmit` (`runtime/src/sky_runtime/ui/helpers.rs:1044-1049`) and
  `Std.Html.Events.onSubmit` (`runtime/src/sky_runtime/html.rs:847-849`,
  backing `sky_kernels::KernelFn::HtmlOnSubmit` — the ONLY kernel with
  `html_event_shape() == HtmlEventShape::Raw`, confirmed at
  `crates/sky_kernels/src/lib.rs:3619-3636`) both construct
  `Event::OnRaw(name, Arc::new(payload))`, type-erasing the handler behind
  `Arc<dyn std::any::Any + Send + Sync>`.
- `HandlerIndex::resolve` AND `HandlerIndex::resolve_form`
  (`runtime/src/sky_runtime/live/dispatch.rs:27-45`) both hard-code
  `Event::OnRaw(_, _) => None` — there is no downcast anywhere in the
  codebase. Confirmed by an exhaustive repo-wide search: every reference to
  `OnRaw` is either the enum definition, its two dead-end construction
  sites, or a doc comment claiming a downcast happens that does not exist
  in code.
- `Event::OnForm` — the variant that DOES dispatch correctly via
  `resolve_form` (tested: `dispatch.rs`'s `resolves_onform` test passes) —
  is constructed ONLY inside `dispatch.rs`'s own `#[cfg(test)] mod tests`.
  There is no production code path that ever builds an `Event::OnForm`
  today.
- Net effect: **every Sky app calling `Ui.onSubmit` or
  `Std.Html.Events.onSubmit` today has a form that silently does nothing on
  submit** — no Msg dispatch, no error, no log. Confirmed via `Read` (not
  grep, which is unreliable for the substring `onSubmit` in this sandbox —
  see note below) that this is exercised by real example apps:
  `examples/19-skyforum/src/View/Login.sky:35` (`Ui.onSubmit DoSignIn`),
  `examples/37-composite-live-shop/src/View/SignIn.sky`, and
  `Std.Html.Events.onSubmit` in `examples/12-skyvote`,
  `examples/27-multi-session-chat`, `examples/28-streaming-chat`.
- `runtime/src/sky_runtime/live/form.rs:29-33`'s own doc comment says "The
  codegen-emitted `onSubmit` closure calls this [`decode_form_or_warn`]" —
  confirming this WAS the intended design and was never fully wired up in
  the emit path; it isn't a new idea, it's finishing what the runtime side
  already assumed was true.

  Tooling note: `rg`/Bash-tool grep output in this sandbox intermittently
  mangles/redacts the literal substrings `onSubmit` and `CssSafety` (e.g.
  rendering as `ln` / `n`) in this environment for reasons unrelated to the
  actual file contents. Every finding in this section was cross-checked
  with the `Read` tool against the real file bytes; do not trust `rg`
  output alone for these two substrings when re-verifying.

### 3.2 The either/or, and the recommendation

**Option A — sanction the divergence.** Keep `Arc<dyn Any>`, document it as
an intentional mirror of Go's `reflect`-based dispatch (as the stale
comments already half-claim), and add the missing downcast at
`resolve`/`resolve_form`. Problem: `HandlerIndex<M>` is generic only over
`M` (the app's Msg type) — the erased payload's concrete argument type `T`
(the record `Ui.onSubmit`'s handler decodes into, e.g. `AuthCreds`) is
NOT part of any type parameter in scope at the dispatch call site, so there
is no `T` to downcast TO without inventing a second type-erased registry
keyed by `TypeId` (a bigger, riskier change) or requiring every call site
to pre-register its concrete type via some side channel. This also leaves
`Arc<dyn Any>` in the codebase permanently — the exact thing #156 flags as
a PRINCIPLES.md violation — with a downcast that could still panic if ever
misused.

**Option B — monomorphize per concrete type at construction (like #109's
own sibling shape) — RECOMMENDED.** The dispatch machinery for
form-submit-shaped events ALREADY EXISTS and is ALREADY CORRECT: `Event::
OnForm` + `HandlerIndex::resolve_form` + `live::form::decode_form_or_warn`
are fully implemented, tested (`dispatch.rs`'s `resolves_onform`,
`form.rs`'s round-trip tests), and reachable. The bug is entirely that
`ui_on_submit_` / `html_on_raw_` construct the WRONG variant. Fix: change
BOTH functions to accept a properly-typed generic closure — `F: Fn(T) -> M
+ Send + Sync + 'static` where `T: serde::de::DeserializeOwned` — instead
of `A: Any + Send + Sync`, and have them build `Event::OnForm` directly,
closing over `decode_form_or_warn::<T>`. `T` is resolved by ordinary Rust
generic inference from the emitted handler closure's own concrete type at
the call site (exactly as `f(x)` already infers `x`'s type from `f`'s
signature in any generic Rust function — this is not a new inference
pattern, every sibling `Ui.onChange`/`Html.onInput` emit-site wrapper
already relies on the same class of inference). **No `emit_expr.rs` call-
site changes are required at all** — the codegen already emits
`ui_on_submit_({f_s})` / `html_on_raw_({name}, {payload_s})` passing the
handler expression directly; changing only the two runtime function
SIGNATURES is sufficient. This is lower risk (reuses existing tested
dispatch code, zero new runtime dispatch logic), strictly smaller (2 files
of real changes vs. Option A's new TypeId registry), and it FULLY closes
the PRINCIPLES.md violation (removes `Arc<dyn Any>` from the codebase
entirely — not just stops constructing it defensively).

**Recommendation: Option B.** Implement below.

### 3.3 Fix — `runtime/src/sky_runtime/html.rs`

Generated record structs already conditionally derive `serde::Deserialize`
via the existing `#93` seal (`crates/sky_backend_rust/src/emit_types.rs:
557-571`, `rec.is_serde && ctx.uses_live` — a structural fixpoint over the
record's fields, independent of whether the record happens to be the app's
Model, so an `onSubmit` handler's argument record gets this automatically
as long as its fields are themselves serde-derivable, which is true for
ordinary form-shaped records). This closes the only real prerequisite for
Option B.

`html_on_raw_`'s only caller is `Std.Html.Events.onSubmit` (confirmed —
`HtmlOnSubmit` is the sole `HtmlEventShape::Raw` kernel; there is no
separate `Std.Html.Events.on` in this codebase — the historical doc
comment mentioning `on` is itself stale and is corrected below).

Replace (`html.rs:842-849`):

```rust
/// `Std.Html.Events.{onSubmit,on}` (#107) — a heterogeneous-payload event whose
/// handler type is DECOUPLED from `M` (a form's `onSubmit DoSignIn` must not
/// leak `LoginForm -> Msg` into the surrounding `Html msg`). The payload is
/// type-erased behind `Arc<dyn Any>`; the wire driver downcasts it at dispatch.
#[must_use]
pub fn html_on_raw_<M, A: std::any::Any + Send + Sync>(name: String, payload: A) -> Attribute<M> {
    Attribute::EventAttr(Event::OnRaw(name, std::sync::Arc::new(payload)))
}
```

with:

```rust
/// `Std.Html.Events.onSubmit` (#107) — a heterogeneous-payload event whose
/// handler argument type `T` is DECOUPLED from `M` at the Sky type level (a
/// form's `onSubmit DoSignIn` must not force `LoginForm` into the surrounding
/// `Html msg`'s type). `T` stays a free HM variable in `constrain.rs`'s
/// scheme; at CODEGEN time Rust's ordinary generic inference recovers the
/// CONCRETE `T` from the handler closure `f`'s own monomorphized signature —
/// no runtime type erasure, no `Arc<dyn Any>`, no downcast (#109/#156, closes
/// the PRINCIPLES.md no-`dyn Any` exception this used to be). Builds
/// `Event::OnForm` directly, reusing the already-correct, already-tested
/// `HandlerIndex::resolve_form` + `decode_form_or_warn` dispatch path.
///
/// Despite the historical name (kept to avoid an unrelated rename touching
/// `naming.rs` / emit-site literals / parity tooling), this is no longer a
/// "raw" escape hatch — it fully participates in typed dispatch.
#[cfg(feature = "live")]
#[must_use]
pub fn html_on_raw_<M, T, F>(name: String, payload: F) -> Attribute<M>
where
    T: serde::de::DeserializeOwned,
    F: Fn(T) -> M + Send + Sync + 'static,
{
    Attribute::EventAttr(Event::OnForm(
        name,
        std::sync::Arc::new(move |fd: FormData| {
            crate::sky_runtime::live::form::decode_form_or_warn::<T>(fd).map(|t| payload(t))
        }),
    ))
}

/// Non-`live` builds (Sky.Tui without the HTTP wire) have no `FormData`
/// decode path. `Std.Html.Events.onSubmit` was already inert everywhere
/// before this fix (the `OnRaw` path never dispatched in ANY backend), so
/// degrading to a structural no-op attribute here is not a regression for
/// Tui — it was never functional there and Tui has no form-submit wire
/// concept. Kept `#[must_use]`-free (matches `Attribute::NoAttr`'s existing
/// callers elsewhere).
#[cfg(not(feature = "live"))]
pub fn html_on_raw_<M, T, F: Fn(T) -> M>(_name: String, _payload: F) -> Attribute<M> {
    Attribute::NoAttr
}
```

Then, since `html_on_raw_` was the LAST production constructor of
`Event::OnRaw`, remove the variant entirely (it becomes 100% dead code —
confirmed by an exhaustive `OnRaw` search across `runtime/` and `crates/`:
every hit is either the enum definition/derived-trait plumbing, the two
constructors being fixed here, or the dispatch no-op arm, with zero test
constructions and zero references anywhere else in the tree). Update the
`Event<M>` enum (`html.rs:32-57`):

```rust
/// Variant names mirror the Sky stdlib `Std.Html.Attributes.Event` ADT
/// (`OnMsg | OnString | OnBool | OnForm`). `OnString`/`OnBool` carry
/// `Arc<dyn Fn(..) -> msg>` (not bare fn pointers) so the handler can be a
/// CAPTURING closure — exactly as the Go backend allows. A faithful Sky.Live
/// app's `onChange = \s -> toMsg (parse s default)` captures locals; a bare
/// fn-pointer field rejected that. Bare ctors / non-capturing fns coerce into
/// `Arc::new` fine; capturing closures box into the trait object.
///
/// `Ui.onSubmit` / `Std.Html.Events.onSubmit` (the heterogeneous-payload
/// handler whose argument type is decoupled from `msg`) construct `OnForm`
/// too — `ui_on_submit_` / `html_on_raw_` close over a
/// `decode_form_or_warn::<T>` call for the CONCRETE record type `T`, recovered
/// by ordinary Rust generic inference on the handler closure's own signature
/// at the codegen call site, never `Arc<dyn Any>` (#109/#156 — the former
/// `OnRaw` variant, which erased the payload behind `Arc<dyn Any>` and was
/// consequently NEVER dispatchable in any backend, is deleted).
#[derive(Clone)]
pub enum Event<M> {
    OnMsg(String, M),
    OnString(String, std::sync::Arc<dyn Fn(String) -> M + Send + Sync>),
    OnBool(String, std::sync::Arc<dyn Fn(bool) -> M + Send + Sync>),
    /// Form-submit handler. Returns `Option<M>`: a malformed/incomplete form
    /// (decode failure) yields `None` so the live loop dispatches no Msg (see
    /// `decode_form`).
    OnForm(
        String,
        std::sync::Arc<dyn Fn(FormData) -> Option<M> + Send + Sync>,
    ),
}
```

Remove the `OnRaw` arms from `name()` (drop the `| Event::OnRaw(n, _)` line),
`kind_name()` (drop `Event::OnRaw(n, _) => (4, n),` — renumber is NOT
required, the tags just need to stay distinct, but renumbering to `0..=3`
is fine too since nothing persists these tags across a binary boundary),
and `kind_tag()` (drop `Event::OnRaw(..) => "OnRaw",`).

### 3.4 Fix — `runtime/src/sky_runtime/ui/helpers.rs`

Add `FormData` to the existing import at line 526:

```rust
use crate::sky_runtime::html::{Attribute as HtmlAttribute, Event, FormData};
```

Replace (`helpers.rs:1036-1049`):

```rust
/// `Ui.onSubmit : (a -> msg) -> Attribute msg`
///
/// Stores the handler type-erased as `Event::OnRaw("submit", Arc<dyn Any>)`.
/// The Sky.Live dispatch layer downcasts + JSON-decodes form data into the
/// typed record at runtime, matching the Go backend's `json.Unmarshal` path.
/// `A: Any + Send + Sync` is always satisfied by emitted Sky function types
/// (they are `'static` enum constructors or pure closures with no borrows).
pub fn ui_on_submit_<M, A: std::any::Any + Send + Sync>(f: A) -> Attribute<M> {
    Attribute::AttrEvent(HtmlAttribute::EventAttr(Event::OnRaw(
        "submit".into(),
        std::sync::Arc::new(f),
    )))
}
```

with:

```rust
/// `Ui.onSubmit : (a -> msg) -> Attribute msg`
///
/// Builds `Event::OnForm` directly: the handler `f`'s argument type `T` is
/// recovered by ordinary Rust generic inference from `f`'s own monomorphized
/// signature at the codegen call site (never type-erased at runtime). The
/// Sky.Live dispatch layer (`HandlerIndex::resolve_form`) decodes the wire
/// `FormData` into `T` via a re-encoded x-www-form-urlencoded round trip
/// (`live::form::decode_form_or_warn` — type-directed per-field coercion, NOT
/// a JSON path), matching the Go backend's `json.Unmarshal` semantics at the
/// record-shape level (case-insensitive field-name match, missing field ⇒
/// zero value). `F: Fn(T) -> M + Send + Sync + 'static` is always satisfied
/// by emitted Sky function types (they are `'static` enum constructors or
/// pure closures with no borrows) — a strictly narrower requirement than the
/// `A: Any` bound this replaces (#109/#156).
#[cfg(feature = "live")]
pub fn ui_on_submit_<M, T, F>(f: F) -> Attribute<M>
where
    T: serde::de::DeserializeOwned,
    F: Fn(T) -> M + Send + Sync + 'static,
{
    Attribute::AttrEvent(HtmlAttribute::EventAttr(Event::OnForm(
        "submit".into(),
        std::sync::Arc::new(move |fd: FormData| {
            crate::sky_runtime::live::form::decode_form_or_warn::<T>(fd).map(|t| f(t))
        }),
    )))
}

/// Non-`live` builds (Sky.Tui without the HTTP form wire): `Ui.onSubmit` was
/// already inert everywhere before this fix, so this degrades to a
/// structural no-op — not a regression, Tui has no form-submit wire concept.
#[cfg(not(feature = "live"))]
pub fn ui_on_submit_<M, T, F: Fn(T) -> M>(_f: F) -> Attribute<M> {
    Attribute::NoAttribute
}
```

### 3.5 Doc-only touch-ups (no behavior change, close the "stale comment" half of #109)

- `crates/sky_kernels/src/lib.rs:70-73` (`HtmlEventShape::Raw` doc): change
  "Constructs `Event::OnRaw`, which type-erases the payload behind `Arc<dyn
  Any>`, so `msg` stays free" → "`msg`/the payload type stay free at the
  Sky/HM level only; the codegen-side runtime constructor (`html_on_raw_`)
  now builds `Event::OnForm` with the concrete payload type recovered via
  Rust generic inference — never `Arc<dyn Any>` at runtime (#109/#156)."
- `crates/sky_types/src/constrain.rs:4002-4004` (comment above the
  `HtmlEventShape::Raw => fun(var(1), html_attr(var(0)))` scheme arm):
  change "the handler `var(1)` is type-erased into `Event::OnRaw`, leaving
  `msg` (`var(0)`) free" → "the handler `var(1)` stays an unconstrained HM
  var here (Sky-level polymorphism only — see `html.rs`'s `Event::OnForm`
  for the runtime-typed construction, not `Event::OnRaw`, which no longer
  exists)."
- `crates/sky_backend_rust/src/emit_expr.rs:4541-4545` and `4559-4565`
  (comments above `KernelFn::UiOnSubmit` and the shared
  `k.html_event_shape().is_some()` arm): drop "type-erased into Arc<dyn Any>
  (OnRaw "submit")" / "the `Raw` (onSubmit) form Arc-wraps the type-erased
  handler" wording; replace with a one-line note that `ui_on_submit_` /
  `html_on_raw_` now build `Event::OnForm` with the concrete type recovered
  by Rust inference on the emitted closure — no emit-site code change was
  needed for this fix, only the runtime function signatures.

### 3.6 Regression tests

**Primary end-to-end regression** — add to
`runtime/src/sky_runtime/live/dispatch.rs`'s existing `#[cfg(test)] mod
tests` (this module already exercises `Event::OnForm` + `HandlerIndex`
end-to-end via `resolves_onform`; this new test proves the REAL production
call path, not just the runtime primitive in isolation):

```rust
#[test]
fn ui_on_submit_dispatches_via_onform_not_onraw() {
    use crate::sky_runtime::ui::element::Attribute as UiAttribute;

    #[derive(serde::Deserialize, Default, PartialEq, Debug)]
    #[serde(default)]
    struct Creds {
        email: String,
        password: String,
    }

    let attr = crate::sky_runtime::ui::helpers::ui_on_submit_(|c: Creds| {
        Msg::Typed(format!("{}:{}", c.email, c.password))
    });
    let html_attr = match attr {
        UiAttribute::AttrEvent(a) => a,
        other => panic!("expected AttrEvent, got {other:?}"),
    };
    let mut t = Html::HElement("form".into(), vec![html_attr], vec![]);
    assign_sky_ids(&mut t, "r");
    let idx = build_index(&t);

    // Must dispatch via resolve_form (Event::OnForm), NOT resolve()
    // (which returns None for a submit event with no positional args).
    assert_eq!(idx.resolve("r", "submit", &[]), None);

    let mut fd = FormData::new();
    fd.insert("email".into(), "a@b.com".into());
    fd.insert("password".into(), "hunter2".into());
    assert_eq!(
        idx.resolve_form("r", "submit", fd),
        Some(Msg::Typed("a@b.com:hunter2".into()))
    );
}
```

(`Msg` here is the existing `Msg { Inc, Typed(String) }` test enum already
defined at the top of `dispatch.rs`'s test module — reuse it, don't
redefine.)

**Sibling regression for `Std.Html.Events.onSubmit`** — same shape, add to
`runtime/src/sky_runtime/html.rs`'s `#[cfg(test)] mod tests`, gated
`#[cfg(feature = "live")]` on the test fn (the module itself is
`#[cfg(test)]`-only, not feature-gated, so individual tests must self-gate):

```rust
#[test]
#[cfg(feature = "live")]
fn html_on_submit_dispatches_via_onform() {
    // Mirror of dispatch.rs's ui_on_submit_dispatches_via_onform_not_onraw,
    // exercising Std.Html.Events.onSubmit's backing fn directly.
    #[derive(serde::Deserialize, Default, PartialEq, Debug)]
    #[serde(default)]
    struct Order {
        item: String,
    }
    let attr: Attribute<String> = html_on_raw_("submit".to_owned(), |o: Order| o.item);
    // ... build a tree, assign_sky_ids, build_index, resolve_form — same
    // pattern as dispatch.rs's test, adjusted for the html::Attribute shape.
}
```

**Regression that the OLD behavior is gone (belt-and-braces):** confirm
`rg -n "OnRaw" runtime/ crates/` returns zero hits after this lands (the
`Event::OnRaw` variant, its two constructors, and the dead dispatch arm are
all removed).

### 3.7 Verification

```bash
cd /home/arthur/Documentos/comp/sky-rust
cargo build -p sky-runtime-rust --features full
cargo build -p sky-runtime-rust --features tui     # non-live: confirm the
                                                     # #[cfg(not(feature="live"))]
                                                     # fallback compiles standalone
cargo test -p sky-runtime-rust --features full --lib
cargo clippy -p sky-runtime-rust --features full -- -D warnings
rg -n "OnRaw" runtime/ crates/    # expect: no output
```

Then a real end-to-end check against an affected example (proves the fix
in the actual product, not just unit tests):

```bash
cd examples/19-skyforum && rm -rf sky-out .skycache .skydeps && skyc build src/Main.sky
# run the app, submit the sign-in form, confirm DoSignIn actually dispatches
# (session state changes / redirect happens) instead of silently no-op'ing.
```

### 3.8 Sequencing vs. Class 5 (both touch `emit_expr.rs`)

The classification doc (`campaign-classification-2026-07-09.md`, "Recommended
processing order" step 3) flags that Class 5's mechanical items (#99 /
#125 / #142 / AUD-09's O(n²) clone fix) and this item both touch
`emit_expr.rs`, and asks which should land first.

**Finding: the actual overlap is much smaller than the classification
implied.** Per §3.2-3.5 above, THIS fix requires **zero functional changes**
to `emit_expr.rs` — only doc-comment edits at two small, well-isolated spots
(`~4541-4545`, `~4559-4565`, the `KernelFn::UiOnSubmit` arm and the shared
`html_event_shape` arm). Class 5's items work in a different region of the
same file (Access-node cloning / thunk coverage / borrow-fast-path
restoration around `emit_expr.rs`'s clone-insertion helpers, per the
backlog's own line references, e.g. `#142`'s `Expr::Access` clone-
unconditional fix). Line-range overlap risk is low.

**Recommendation: land this spec (#109/#156) first.** Reasons:
1. It fixes a currently-broken, security-adjacent, user-visible feature
   (form submit silently does nothing — data loss for the end user) that
   ships in at least 5 example apps today; Class 5's items are
   internal-quality (clone/borrow discipline, no user-visible breakage).
2. Its `emit_expr.rs` footprint is comment-only (3-6 lines total across two
   spots) — trivial for Class 5's broader mechanical sweep to rebase over
   afterward, whichever order actually lands second.
3. The bulk of this fix lives in `runtime/` (`html.rs`, `helpers.rs`), a
   file Class 5 does not touch at all, so most of the diff has zero
   conflict surface regardless of order.

If Class 5 is already mid-flight when this lands, no coordination beyond a
normal rebase is expected; do not block either on the other.
