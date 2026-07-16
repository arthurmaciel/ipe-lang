# Architecture Decision Records

An ADR captures **one architectural decision** and the reasoning behind it: the
*why*, not the *how*. Where a spec or plan describes a build procedure (obsolete
the moment the code lands, and misleading once it drifts), an ADR records the
decision, the alternatives that were rejected, the constraint that forced the
choice, and the invariant that must continue to hold. "Already implemented" is
exactly when an ADR earns its keep — the code is now the source of truth for
*how*, and the ADR preserves the *why* the code can't.

## File convention

- One decision per file, named `NNNN-kebab-slug.md`.
- `NNNN` is a zero-padded, sequential number (`0001`, `0002`, …). Never reuse a
  number.
- `0000-template.md` is the template; copy it for each new ADR.

## Front-matter

Every ADR opens with:

```
Status: Accepted | Superseded by NNNN | Deprecated
Date: YYYY-MM-DD
```

And carries three sections:

- **Context** — the forces in play: the problem, the constraints, the
  invariants, the relevant prior art.
- **Decision** — what was decided, and the alternatives that were rejected (and
  why).
- **Consequences** — what follows: what becomes easier, what becomes harder,
  what must keep holding true.

## Immutability

**ADRs are immutable.** You never edit a decision once it is Accepted. When a
decision changes, write a *new* ADR that supersedes the old one:

- The new ADR's Context references the old one.
- The old ADR's Status becomes `Superseded by NNNN` (this single-line status
  edit is the only change ever made to a decided ADR).

This keeps the decision history honest: the record of *what we believed and why,
at the time* survives even after we change our minds.
