Status: Accepted

# 0021. TEA is a state engine; init signatures are prescriptive per app shape

## Context

Ipê carries a *typed* `WebReq` as `init`'s argument, and the design needed a
principled justification. Two deeper questions followed: what is TEA, and does
`init` belong in every app shape?

## Decision

Adopt a narrower, sounder principle: **TEA is a *state engine* — a single
`Model` evolved by pure `update` over typed `Msg`s, with every effect reified as
data (`Cmd`/`Sub`). `view` is an *optional projection* of the Model, not part of
TEA's core.** This lets CLI (`Task.run` pipeline) and Http.Server (`listen`
router) be correct *non-TEA* shapes without forcing `init` onto them.

For reactive shapes (Ipe.Web, Ipe.Tui, Ipe.Webview) `init` is mandatory, and
its argument is **prescriptive, not inferred**: `init : WebReq -> (Model, Cmd
Msg)` for Web (per-session request context), `init : () -> (Model, Cmd Msg)`
for Tui/Webview (no non-ambient per-invocation context). The **effects-authority
rule:** `init`'s argument carries ONLY context that is specific to this init
invocation AND not reachable through the ambient `System`/effects stdlib; all
ambient input (env, args, cwd) is reached via `System.*` from anywhere.

Rejected alternatives:

- **A free type variable for `init`'s argument** — Ipê's typed `WebReq` makes a
  permissive free-tvar inapplicable; being prescriptive is both more Elm-faithful
  and make-invalid-states-unrepresentable.
- **The ocap model** (threading an `Env` capability through `init` exclusively,
  no ambient authority) — filed as post-parity exploration, not adopted; it is a
  language-wide redesign that would kill the ambient `System.*` API.

This is a **sanctioned divergence from Elm**, which needs `flags` as an init arg
only because browsers sandbox JS; Ipê runs natively with a real `System` API, so
flags-as-init-arg is redundant.

## Consequences

- **Invariant that must keep holding:** every reactive cfg's `init` signature is
  fixed by app shape, not inferred.
- The architecture keeps the door open for future headless reactive shapes (TEA
  minus `view`) as a clean subtraction — `view` stays an optional field of
  view-bearing shapes rather than core to the engine.
