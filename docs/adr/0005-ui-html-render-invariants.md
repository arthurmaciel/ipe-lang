Status: Accepted

# 0005. UI/HTML render invariants (pseudo-class wire format, CSS escaping, typed onSubmit)

## Context

Three UI/HTML rendering + event-sink issues in the `Ipe.Ui` → `Ipe.Html` render
kernels needed closing. The code is the source of truth for the *how*; this ADR
records the durable *why* and the invariants a future change must not break.

Already correct and NOT to be re-touched: the `escape_text`/`escape_attr` split
in `html.rs` (`escape_text`: `& < > '`; `escape_attr`: that plus `"`).

## Decision

### 1. Pseudo-class rules travel as one `data-ipe-pc-rules` marker with a stable wire tag

`Ipe.Ui`'s pseudo-class sugar (`Background.hoverColor`, `Ui.onPseudo`, etc.)
builds `Attribute::AttrPseudoRule(PseudoClass, css)`. The render pipeline must
harvest every `AttrPseudoRule` on an element into ONE
`data-ipe-pc-rules` HTML attribute — the marker the downstream
`live::style_inject::apply_style_injections` pass converts into a `<style>`
block. (Previously `collect_html_attrs`'s catch-all `_ => {}` silently swallowed
it, so pseudo-class styling rendered to nothing in *every* backend — Ipe.Web,
Ipe.Webview, and any bare `render_html` caller.)

The wire format is a fixed contract shared between the encoder and decoder — do
not re-invent it:

- Tag mapping: `Hover → "h"`, `Focus → "f"`, `FocusVisible → "v"`,
  `Active → "a"`, `Disabled → "d"`.
- One entry: `"<tag>|<css>"`; multiple entries joined with `"||"`.
- Empty-`css` entries are dropped (never emitted).

The encode direction (`PseudoClass::wire_tag()`) lives **colocated with the
`PseudoClass` type** as the single source of truth, and must stay in lock-step
with `style_inject::pseudo_selector_for_tag`'s decode mapping. Ipe.Tui has no
CSS pseudo-class concept and never runs the injection pass; the marker must
simply not leak there (it is dropped, no behaviour change).

### 2. CSS value safety decodes escapes before scanning (parse, don't validate)

`SafeCssValue::parse` scanned the raw value for breakout chars
(`; { } </ /* @import`) and script-sink keywords (`expression(`, `javascript:`,
…) as literal substrings. CSS Syntax Level 3 §4.3.7 defines a general escape
mechanism (`\` + 1–6 hex digits) that decodes to a code point *anywhere* a CSS
token is lexed — so `\65 xpression(...)` → `expression(...)`,
`\75 rl(\6a avascript:...)` → `url(javascript:...)`, `\3b` → `;`. None of those
literal substrings appear in the raw string, so the raw scan let them through.

**Decision:** decode CSS escapes and re-run the **same** dangerous-pattern scan
against the decoded string, with the pattern list refactored into ONE shared
helper so the raw-value and decoded-value paths can't drift (one list, one
policy). Scope is precisely `SafeCssValue`; `SafeCssPropertyName` and
`SafeCssSelector` reject `\` outright via their charset allowlists and are
unaffected. Separately, `Css.raw`/`keyframes` bodies gate `@import`
(stylesheet-injection / CSS-level SSRF) and `expression(` (legacy IE script
sink); those are documented trusted-author escape hatches otherwise.

### 3. `onSubmit` carries a typed generic closure — no `Arc<dyn Any>`

`Ui.onSubmit` / `Ipe.Html.Events.onSubmit` were 100% non-functional at runtime
(never dispatched a Msg) and their payload was type-erased through
`Arc<dyn Any>`. Two options existed: (A) sanction the `dyn Any` divergence, or
(B) rework both functions to accept a properly-typed generic closure
`F: Fn(T) -> M`. **Option B was chosen** — it removes `Arc<dyn Any>` from the
codebase, so a "form payload of the wrong type" is
unrepresentable rather than a runtime downcast. This is the
make-invalid-states-unrepresentable choice over the parse-at-runtime one.

## Consequences

- The pseudo-class wire tags (`h`/`f`/`v`/`a`/`d`) and the `|` / `||` framing are
  a stable contract between `ui::element` (encode) and `live::style_inject`
  (decode). Changing one side without the other silently breaks pseudo-class
  styling — `wire_tag()` and `pseudo_selector_for_tag` must move together.
- CSS value safety has exactly one pattern list, checked against both raw and
  escape-decoded values. Any new breakout/script-sink pattern is added once;
  any new CSS evasion vector must be handled by extending the *decode* step, not
  by adding a second scanner.
- `onSubmit` (and the same-shaped typed-payload event sinks) stay generic over
  the Msg type; reintroducing `Arc<dyn Any>` anywhere in the event path is a
  regression against PRINCIPLES.md.
