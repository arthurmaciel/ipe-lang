Status: Accepted
Date: 2026-09-02

# 0067. Program-as-data: partition into patchable data vs compiled logic

## Context

ADR 0066 chose the data-patch tier as the dev-loop speed lever. That choice
needs a mechanism: *how* does a program become partly patchable data without dev
behaviour diverging from production behaviour? A dev fast-path that runs
different code from production is a correctness hazard — the developer would be
testing something the shipped program never does.

The starting point is a server-driven architecture: the whole update loop runs
where the model is held as data, and each event re-renders and diffs to a patch
sent to the client. The render→diff→patch pipe and the indirection from a client
event to a concrete message already exist. The question is how far the
"patchable data" boundary can extend into the program while keeping dev and
production provably identical.

## Decision

**Partition every program into static parts the runtime holds as patchable data
and dynamic parts that stay compiled logic**, using one dev-equals-production
trick throughout:

> Emit compiled code that *reads a data-table entry*. Production **bakes** the
> entry; dev **patches** it over the live socket. The same compiled
> read/apply/render routine runs in both — only the table *contents* differ.

Because the interpreter *is* the compiled read-from-data routine, run identically
in dev and production, dev equals production by construction. Each data-driven
part is conformance-tested so that interpreting the data equals running the baked
specialization; any divergence is a test failure, not a production surprise.

The partition is applied part by part — view structure and appearance as
templates with typed holes for model-derived parts, simple update arms as
transition descriptions, subscriptions as descriptions, an additive model-field
change as extend-with-defaults in the schema-tagged codec (so adding a field
keeps live state), additive message variants, session-scoped init, and effect
*wiring* as data over compiled effect bodies. Every mechanism moves more of the
program into static data; whatever cannot be proven static stays compiled.

The classifier is deliberately **biased toward compiled**: a part is treated as
patchable data only when the compiler can *prove* it carries no model-derived
logic. A misclassification that recompiles is merely slow; a misclassification
that hot-swaps a logic change is a correctness bug. So the unprovable case always
falls back to recompile.

Handlers are never serialized as closures: a static handler is a template
constant carrying an opaque handler id, and a model-dependent handler is a hole
the server's per-render handler map fills. The model-dependent part lives in that
map, not in the transported data.

Rejected alternative — a dev-only interpreter separate from the production code
path. It would make dev fast but would run different logic from production,
breaking the dev-equals-production guarantee that makes the fast path safe to
trust.

## Consequences

A dev edit to a provably-static part (appearance, static view structure, a
simple update arm, a subscription, an additive model field or message variant) is
a data patch with no recompile; only genuinely-new compiled logic triggers a
build. The same server-held model-as-data enables replaying recorded messages
through update (time-travel) and inspecting or editing live state — capabilities
that fall out of the partition rather than being built separately.

The invariant that must hold: every data-driven part has a conformance test
proving interpret-the-data equals run-the-baked-specialization, the read/apply
routines are bounded by construction, and the static/dynamic classifier stays
biased toward "compiled" on anything unprovable. Relaxing that bias, or shipping a
data-driven part without its conformance test, would let a logic change hot-swap
silently — the exact correctness failure this decision is structured to prevent.
