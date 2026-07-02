---
name: design-swarm
description: "Turn an open, architecturally-uncertain design question into ONE vetted design spec using a parallel brainstorming panel — 1 asker (grouped, cited questions incl. blocking contradictions) -> N independent reasoners (each answers everything + proposes a complete design) -> orchestrator-mediated cross-critique -> orchestrator synthesis into a single spec -> adversarial security/soundness design review. Design-only: it produces a spec, never code or a build. Use when the user asks to design/brainstorm a new subsystem or feature where the architecture is not obvious and independent perspectives raise confidence. Trigger: /sky-compiler:design-swarm."
---

# design-swarm

Produce **one** authoritative design spec for a hard, open-ended question by
running a *panel* of independent reasoners in parallel and converging them —
instead of one context reasoning alone. This is the design front-half of
`sky-rust-backend:autonomous-swarm` extracted to stand alone: it ends at an
approved spec, which can then feed `superpowers:writing-plans` or the build
phases of `autonomous-swarm`. It writes **no code and runs no build** — it is
read-only + doc-only, so it is safe to run in parallel with a busy build lane.

**Prime directive:** the orchestrator is the single source of truth and the only
message bus. Reasoners never peer-chat; every hand-off goes through the
orchestrator. Convergence across *independent* reasoners is the confidence
signal; divergence flags the real decisions.

**Two fundamental rules are non-negotiable in every design + review:** PARSE,
DON'T VALIDATE and MAKE INVALID STATES UNREPRESENTABLE. PRINCIPLES order:
security > correctness > soundness > efficiency > completeness > readability.

## When to use / not

| Use it | Don't |
|---|---|
| Open architecture, several viable shapes, the choice matters | The design is obvious — just write the spec (or use `superpowers:brainstorming` solo) |
| A subsystem spanning multiple concerns where blind spots are costly | A small, local change |
| You want independent perspectives to de-risk a decision before building | You already know the answer and only need a plan → `superpowers:writing-plans` |

## Pipeline (run in order; each phase gates the next)

```
0 Frame        : orchestrator states the question, scope, hard constraints, non-goals
1 Ask          : 1 asker -> grouped, CITED question list incl. §0 blocking contradictions (proposes NO answers)
2 Reason       : N independent reasoners (parallel) — each answers EVERY question + proposes a COMPLETE design
3 Cross-critique: orchestrator sends each reasoner the OTHERS' designs -> "where do you disagree + reconciled position?"
4 Synthesize   : orchestrator merges converged decisions + user overrides into ONE spec (docs/architecture/<topic>.md)
5 Review       : adversarial security/soundness design review of the spec -> GO / NO-GO + gap list
6 Handoff      : present the spec + the open decisions the USER must make; offer writing-plans / autonomous-swarm
```

### Phase 0 — Frame (orchestrator, inline)
State the question crisply: what is being designed, the scope boundary, the hard
constraints (the two fundamental rules + the principles order + any project
invariants it must not break), and explicit non-goals. Ground it in the real
codebase — cite the files/subsystems it touches so the panel reasons about the
actual system, not an imagined one.

### Phase 1 — Ask (one asker, run FIRST)
The asker explores the target + its boundary and writes a **comprehensive,
grouped, cited** question list — including a **§0 "blocking contradictions"**
group (places where the request conflicts with an existing invariant or with
itself). It proposes NO answers. This list is the panel's shared prompt.

### Phase 2 — Reason (N independent reasoners, parallel)
Each reasoner independently answers EVERY question and proposes a COMPLETE
design. Independence is the point — do NOT show them each other's work yet.
Default N=3 (2 converges on easy problems; 3 + cross-critique catches more on
genuinely hard architecture; diminishing returns past 3). Each reasons under
the principles order + the two fundamental rules and returns a structured design.

### Phase 3 — Cross-critique (orchestrator-mediated)
There is no peer channel — the orchestrator IS the bus. Send each reasoner the
*other* designs and ask: "where do you disagree, and what is your reconciled
position?" Collect. Convergence → lock it; persistent divergence → a real
decision for the synthesis (or the user).

### Phase 4 — Synthesize (orchestrator)
Merge the converged decisions + any user overrides into ONE authoritative spec
at `docs/architecture/<topic>.md`. Lock each decision with a one-line rationale.
The spec — not the raw reasoner outputs — is the deliverable. Use
`superpowers:writing-plans` conventions for structure where an implementation
plan is in scope.

### Phase 5 — Review (adversarial, blocking)
An independent security/soundness guardian reviews the spec: does it obey the
two fundamental rules? Does it hold the principles order? Any invalid state left
representable, any unparsed boundary, any panic/soundness/security hole in the
proposed shape? GO / NO-GO + a concrete gap list. NO-GO → the orchestrator
tightens the spec + re-reviews.

### Phase 6 — Handoff
Present the spec + the open decisions the USER must make (the genuine forks the
panel could not close from first principles). Offer the next step:
`superpowers:writing-plans` for an implementation plan, or
`sky-rust-backend:autonomous-swarm` to build it.

## Execution (as a Workflow)

Run the panel deterministically with the `Workflow` tool (per
`memory: backend-wiring-protocol`). Skeleton:

```
phase('Ask')      const questions = await agent(ASKER_BRIEF, {schema: Q_SCHEMA})
phase('Reason')   const designs = await parallel(range(N).map(i => () =>
                    agent(reasonerBrief(i, questions), {schema: DESIGN_SCHEMA})))   // INDEPENDENT
phase('Critique') const recon = await parallel(designs.map((d,i) => () =>
                    agent(critiqueBrief(i, designs), {schema: RECON_SCHEMA})))       // orchestrator = bus
phase('Synthesize') const spec = await agent(synthBrief(questions, designs, recon)) // writes the doc
phase('Review')   const verdict = await agent(reviewBrief(spec), {agentType:'security-soundness-guardian'})
return { spec, verdict }
```

Reasoners run under `parallel()` (independent, barrier). The asker precedes them;
the cross-critique is a second `parallel()` fed the collected designs; synthesis
+ review are single agents. Guardian bookends: only the review spends guardian
tokens (design reasoning can be a capable general model). No executor phase — if
building follows, hand off to `autonomous-swarm`.

## Non-negotiables
- **Read-only + doc-only.** No code edits, no build. The single artifact is the spec.
- **Independence before convergence.** Never show reasoners each other's work before Phase 3.
- **One synthesized spec**, not a pile of reasoner plans — executors/planners need a single source of truth.
- **The two fundamental rules + principles order** stated in every reasoner brief AND enforced in the review.
- **Blocking contradictions surfaced first** (asker §0) — a design built over an unresolved contradiction is wasted.

## Tuning knobs
| Knob | Guidance |
|---|---|
| Reasoner count | 3 for hard architecture (+ cross-critique); 2 for moderate; past 3 = diminishing returns |
| Asker depth | Exhaustive + cited; the §0 blocking-contradictions group is the highest-value output |
| Review effort | High — this is the soundness gate on the design before any build is authorised |
| Model | Reasoners: a capable general model. Review: security-soundness-guardian. |
