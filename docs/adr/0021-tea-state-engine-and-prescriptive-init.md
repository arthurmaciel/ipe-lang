Status: Accepted
Date: 2026-07-13

# 0021. TEA is a state engine; init signatures are prescriptive per app shape

## Context

The upstream Sky reference leaves `init`'s argument a free type variable (a
Go-runtime accommodation for its untyped `map[string]any` request). Ipê carries
a *typed* `LiveReq`, so that rationale doesn't transfer, and the divergence
needed a principled justification. The trigger was #180 (`26-ui-showcase`'s
`init : {}` vs Ipê's `Live.app` scheme requiring `init : LiveReq`). Two deeper
questions followed: what is TEA, and does `init` belong in every app shape?

## Decision

Adopt a narrower, sounder principle: **TEA is a *state engine* — a single
`Model` evolved by pure `update` over typed `Msg`s, with every effect reified as
data (`Cmd`/`Sub`). `view` is an *optional projection* of the Model, not part of
TEA's core.** This lets CLI (`Task.run` pipeline) and Http.Server (`listen`
router) be correct *non-TEA* shapes without forcing `init` onto them.

For reactive shapes (Ipe.Live, Ipe.Tui, Ipe.Webview) `init` is mandatory, and
its argument is **prescriptive, not inferred**: `init : LiveReq -> (Model, Cmd
Msg)` for Live (per-session request context), `init : () -> (Model, Cmd Msg)`
for Tui/Webview (no non-ambient per-invocation context). The **effects-authority
rule:** `init`'s argument carries ONLY context that is specific to this init
invocation AND not reachable through the ambient `System`/effects stdlib; all
ambient input (env, args, cwd) is reached via `System.*` from anywhere.

Rejected alternatives:

- **The reference's permissive free-tvar** — Ipê's typed `LiveReq` makes the Go
  rationale inapplicable; being prescriptive is both more Elm-faithful and
  make-invalid-states-unrepresentable.
- **The ocap model** (threading an `Env` capability through `init` exclusively,
  no ambient authority) — filed as post-parity exploration, not adopted; it is a
  language-wide redesign that would kill the ambient `System.*` API.

This is a **sanctioned divergence from Elm**, which needs `flags` as an init arg
only because browsers sandbox JS; Ipê runs natively with a real `System` API, so
flags-as-init-arg is redundant.

## Consequences

- **Invariant that must keep holding:** every reactive cfg's `init` signature is
  fixed by app shape, not inferred. Examples ported from upstream carry
  Ipê-specific patches (`init : {} ->` becomes `init : LiveReq ->`), tracked in a
  reviewable layer so upstream examples stay source-faithful.
- The architecture keeps the door open for future headless reactive shapes (TEA
  minus `view`) as a clean subtraction — `view` stays an optional field of
  view-bearing shapes rather than core to the engine.
