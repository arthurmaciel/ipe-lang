Status: Accepted
Date: 2026-07-28

# 50. `do` / `parallelDo` notation desugars to `Task.andThen` / `Task.parallel`

## Context

Sequential effectful code drifts into nested-lambda pyramids: each step of a
multi-step effect is another `Task.andThen (\x -> …)`, indenting rightward with
depth. This is a server / CLI / script phenomenon — Node-style sequential I/O
with no UI to update between steps. The routed-app effect surface decomposes
sequential effects into message transitions instead, so the pyramid does not bite
there; but the non-routed surface (an HTTP server, a CLI, a background job) has no
such decomposition and the nesting is real.

An effect is a value of type `Task e a` — a *suspended effect* with no visible
constructors. Unlike `Result` and `Maybe`, which you can `case` on, the only way
to reach a `Task`'s result in direct style is to run it. So `Task` is the one
type where run-and-bind sugar adds a genuinely new capability rather than merely
saving nesting. Independent effects have a second shape: run several at once and
collect their results, rather than threading them one after another.

The constraint: add direct-style sequencing without introducing a Monad
typeclass, new IR, or any runtime support — the emitted behaviour must be
identical to the `Task` combinator chain a user writes by hand.

## Decision

Add two keyword-introduced, layout-delimited blocks, both **`Task`-only** and
both pure syntactic sugar:

- **`do`** — a sequential block. Three line forms distinguished by operator
  alone: `p <- e` runs the `Task` `e` and binds its result; `p = e` is a pure
  bind (no `let … in`, reusing Ipê's existing definition-binding shape); a bare
  line runs a `Task` for effect and discards it. A required trailing expression is
  the block's result (auto-wrapped in `Task.succeed` when pure).
- **`parallelDo`** — runs its aligned, same-typed tasks concurrently and collects
  their results as a `List`, usable on its own or bound inside a `do`
  (`results <- parallelDo …`).

Both desugar in the parser to existing nodes:

- `⟦(p <- e); rest⟧ = e |> Task.andThen (\p -> ⟦rest⟧)`
- `⟦(p = e); rest⟧ = let p = e in ⟦rest⟧`
- `⟦e; rest⟧ = e |> Task.andThen (\_ -> ⟦rest⟧)` (bare discard)
- a `parallelDo` of aligned tasks desugars to `Task.parallel`.

Because the desugar targets today's `Task.andThen` / `Task.parallel` / `Let`
nodes, canon, type inference, lowering, and emit are untouched — the whole
feature is a parser production plus a desugar fold. Failure short-circuits at the
first `Err` (that is just `Task.andThen` semantics), and the desugar stamps its
synthetic nodes with the block's own span so a diagnostic never points at a
node the user did not write.

Alternatives rejected:

- **Extend the sugar to `Result` / `Maybe`.** Both already have visible
  constructors, `andThen`, and applicative combinators — two composition tools
  each. Run-and-bind sugar over them is redundant, and admitting it invites a
  general Monad abstraction the language deliberately avoids. `do` over a
  non-`Task` is a compile error.
- **A function-shaped `Task.block` / `perform` keyword.** `Task.block` disguises
  a syntactic form as a qualified name (it can't be aliased or passed), and
  `perform` collides head-on with the existing `Cmd.perform` / `Task.perform`
  that *run* a `Task`. A distinct keyword is honest about being a form.
- **A single `!` punctuation mark carrying bind + discard + pure-by-absence.** One
  mark meaning three things is easy to misread; the operator-per-line spelling
  (`<-`, `=`, bare) makes each line's role legible.
- **Eliminating `let … in` language-wide** so blocks need no special pure-bind
  form. That is a separate, much larger identity decision; here `do` coexists
  with `let … in`, and the bare `p = e` reuses the definition-binding shape rather
  than inventing a bare `let` statement.

## Consequences

- Direct-style sequential effect code is available on the non-routed effect
  surface, with no new IR, no runtime, and no Monad typeclass — the emitted `Task`
  behaviour is identical to a hand-written combinator chain.
- The `<-` / `=` / bare distinction keeps "effectful vs pure" legible at a glance
  and lets the compiler police the boundary (a pure value on a bare discard line,
  or a `Task` bound with `=`, is diagnosable).
- The invariant that must keep holding: a `do` / `parallelDo` block is *only*
  sugar. Anything that would make it observably differ from its `Task.andThen` /
  `Task.parallel` expansion — new runtime behaviour, an error channel a plain
  chain lacks — is out of bounds.
- This is a deliberate syntactic divergence from the Elm surface, recorded in
  `docs/divergences-from-sky.md`.

## Conventions

ADRs describe Ipê on its own terms. This decision is stated as a standalone Ipê
decision, without reference to any prior or external implementation.
