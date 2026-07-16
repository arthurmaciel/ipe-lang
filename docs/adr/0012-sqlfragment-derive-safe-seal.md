Status: Accepted
Date: 2026-07-09

# 0012. SqlFragment is a fully-derivable, redacting type — no capability denylist

## Context

The stdlib needs a strongly-typed SQL-WHERE builder (`SqlFragment` / `Sql.*`
combinators) to replace the raw-string `unsafeFindWhere` kernel — a live SQL
injection surface. The design question mirrors ADR 0006's for `Secret`: should a
security-sensitive type be made *non-derivable* (so equality/printing become
compile-time errors, forcing explicit escape kernels), or *safe by
construction*? `SqlFragment` embeds `SqlParam` bind values, some of which may be
`Secret`.

## Decision

`SqlFragment` is fully derivable (`Clone`, `PartialEq`) with a hand-written
`Debug` that shows SQL text plus a bind *count* only — never bind *values*.
Equality is safe because every constituent `SqlParam` field type already
implements `PartialEq`, and `Debug` is redacting by design, so there is no
second path a caller could accidentally invoke. This avoids inventing a
per-trait capability-table (`IrTypeCaps` / `ty_is_equatable`) mechanism.

Rejected alternative: marking `SqlFragment` non-derivable via a per-trait
denylist. That created a *derived-blast-radius* problem — a record merely
*containing* a `SqlFragment` would lose **all** its derives, not just the one
denied, the same exit-0-then-cargo-fail class ADR 0006 avoids for `Secret`.

## Consequences

- `frag == frag` and `toString frag` are both allowed and always safe; the only
  path to bind-value disclosure is the single `reveal` call on a contained
  `Secret` (per ADR 0006).
- **Constraint / invariant that must keep holding:** every `SqlParam` variant
  field type must implement `PartialEq` for `SqlFragment`'s derive to stay safe.
  If a future variant adds a non-`PartialEq` field, the fallback is the denylist
  design *for `SqlFragment` only* (`Secret` is unaffected either way) — never a
  silent loss of derives on containing records.
- This keeps the project's "safe by construction, not by escape hatch" posture
  consistent across both security newtypes (`Secret`, `SqlFragment`).
