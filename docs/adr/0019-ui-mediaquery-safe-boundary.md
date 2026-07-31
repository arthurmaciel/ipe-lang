Status: Accepted

# 0019. Ui.mediaQuery reuses the shared CSS-safety collector behind a typed SafeCssMediaQuery boundary

## Context

The infrastructure to *consume* media-query markers (`data-ipe-mq-q` +
`data-ipe-mq-rules` → element-id-scoped `<style>@media …</style>`) already
shipped in `live/style_inject.rs`. The missing piece was the *producer*: a
kernel + runtime helper building the wrapper element that carries those markers.
The design question was whether to invent new media-query compilation machinery
or reuse the existing CSS collector — and how to make the raw query string safe,
since it is spliced into `@media {query} {` inside a raw `<style>` body (an
attacker could close the prelude, terminate the element, open a comment, or
smuggle `@import`). This is a new CSS injection boundary distinct from the
inline `style=""` and pseudo-class paths of ADR 0005.

## Decision

Reuse the existing infrastructure with no new mechanism: `ui_media_query_` wraps
a child in a `Node` carrying the two marker attributes, and the rules string
comes from `render::build_style_string(&attrs)` — the **same collector** used
for inline `style=""` and `Ui.onPseudo`, so every CSS value inherits ADR 0005's
hardening. The only new piece is a thin newtype `SafeCssMediaQuery::parse` for
the *query* string, delegating to the shared `has_dangerous_css_pattern` +
`css_unescape` re-scan pair `SafeCssValue` uses (same policy, new boundary). Fail
mode is fail-closed drop: a poisoned query silently omits both markers, leaving
the child intact, so the DOM shape stays stable (always a wrapper `<div>`) and
the Web diff never sees a gate-dependent structural change.

Rejected alternatives:

- **Reuse `SafeCssSelector`** — Media Queries Level 4 range syntax legitimately
  uses `<`/`<=`, which the selector gate forbids; borrowing it would break valid
  queries.
- **Use `SafeCssValue` directly** — muddles the parse-don't-validate story; each
  CSS boundary should carry its own proof type in the type system.
- **A bespoke validator** — violates "one policy, one place"; the danger-pattern
  set is shared.

## Consequences

- **Invariant that must keep holding:** the media-query boundary shares the one
  CSS danger-pattern policy (ADR 0005 §2), so any new evasion vector hardened in
  one CSS consumer strengthens all of them; the wrapper element is always emitted
  regardless of the gate outcome (structural stability for the diff).
- `Ui.breakpoint` (which `breakpointToQuery` delegates to) is un-stubbed for
  free — its earlier eager-passthrough stub becomes a one-line delegation to
  `ui_media_query_`, with zero additional mechanism.
