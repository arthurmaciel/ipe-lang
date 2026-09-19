# Architecture Decision Records

An ADR captures the **why** behind a design — the constraint that forced a
choice, the alternatives rejected, and the invariant that must keep holding —
where the code is the source of truth for the **how**.

## Two layers

The record is kept in two layers so the *reading* set stays small while the
*history* stays complete:

- **Canonical, consolidated ADRs — this directory (`0001`–`0008`).** Eight
  living documents, one per coherent theme, each stating the project's **current**
  decisions for its area. They are updated in place as decisions evolve — a
  superseded decision is rewritten to its latest form, not appended to. These are
  what you read, and the only ADRs anything should cite.
- **Immutable archive — `misc/docs/archive/ADR/` (untracked).** The original
  one-decision-per-file records, verbatim and never edited, kept only for
  provenance. Its `README.md` maps each original to the consolidated ADR that now
  carries its decision. Nothing in the live tree points there; consult it only to
  trace where a past decision went.

A decision that is **designed but not yet implemented** carries a
`NEEDS UPDATE AFTER IMPLEMENTATION` banner naming what to change once the code
lands — so the review after implementation is a `grep`, not a re-read
(`rg "NEEDS UPDATE" docs/adr/`).

## The eight themes

| # | theme |
|---|-------|
| [0001](0001-language-semantics-and-types.md) | Language semantics & types |
| [0002](0002-codegen-soundness-and-the-seal.md) | Codegen soundness & THE SEAL |
| [0003](0003-security-render-and-data-access-invariants.md) | Security, render & data-access invariants |
| [0004](0004-capabilities-trust-boundary-and-confinement.md) | Capabilities, trust boundary & confinement |
| [0005](0005-delivery-shapes-runtimes-hosts-targets.md) | Delivery: shapes, runtimes, hosts, targets |
| [0006](0006-dev-loop-and-program-as-data.md) | Dev-loop & program-as-data |
| [0007](0007-build-incrementality-and-release-infra.md) | Build, incrementality & release infra |
| [0008](0008-governance-cli-teacher-and-stdlib-policy.md) | Governance, CLI-as-teacher & stdlib policy |

## Writing a new decision

- A decision in an existing theme is **edited into** that theme's consolidated
  ADR (update the current form; do not append history).
- A decision that opens a genuinely new theme gets a new consolidated ADR at the
  next number, added to the table above.
- Consolidated ADRs follow the shape of the eight above: a scope statement, then a
  `## Decisions` section of short, current-decision sections. State what the
  design *is* now — no dates, no plan-position labels, no "was X now Y".
- `0000-template.md` is the per-decision template used by the archive.
