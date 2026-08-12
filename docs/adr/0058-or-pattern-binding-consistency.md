Status: Accepted
Date: 2026-08-12

# 0058. Or-pattern alternatives must bind the same names at the same types

## Context

A `case … of` arm may use an or-pattern — `p1 | p2 | …` — which matches when
*any* alternative matches (`POr` in `src/compiler/canon/src/ast.rs`). The body
after `->` then runs, and it may reference variables bound by the pattern.

But at run time only one alternative actually matched, and which one is not
known when the body is type-checked. So the body must see a *consistent* set of
bindings no matter which alternative fired. Two shapes force the question:

- `Just x | Nothing -> …` — `Just x` binds `x`; `Nothing` binds nothing. If
  `Nothing` matched, what is `x`?
- `Error s | Success n -> …` (a reader might reach for `Error String |
  Success Int`, but in *pattern* position the payloads are binders, not types) —
  the two alternatives bind different names; even reusing one name
  (`Error x | Success x`) would demand `x` be two types at once.

Allowing either would let the body reference a variable that is sometimes
unbound, or use it at a type it does not have — an unsound program the compiler
must never accept.

## Decision

**Every alternative of an or-pattern binds the identical set of variable names,
and each shared name has the same type across all alternatives.** Enforced in
two stages (`ast.rs:320-322`):

1. **Name-set equality** — proven fail-fast in canon: all alternatives must bind
   exactly the same set of names.
2. **Per-name type equality** — checked after the solve in the type layer: each
   shared name unifies to one type across alternatives.

So `Just x | Nothing` and `Error s | Success n` are rejected. What is accepted is
every shape that keeps the binding set consistent:

- nullary variants — `Red | Green | Blue -> …` (bind nothing);
- wildcards — `Just _ | Nothing -> …` ("either", no payload read);
- a shared name at a shared type — `Circle r | Square r -> area r` when both
  payloads have that type;
- literals — `1 | 2 | 3 -> …`, `'a' | 'b' -> …`.

**Alternatives considered and rejected:**

- *Permit differing bindings, and treat a name as unavailable in the body when
  an alternative that does not bind it matched.* Rejected: the body cannot
  statically know which alternative fired, so a reference to such a name has no
  sound meaning; it also silently changes what code is legal based on run-time
  control flow.
- *Union the differing payloads into the body's binding.* Rejected: there is no
  principled single type for `String`-or-`Int`, it would push an ad-hoc sum into
  inference, and it obscures rather than clarifies the author's intent — the
  clearer program writes one arm per variant.

## Consequences

- The body of an or-pattern arm always sees one well-defined, well-typed set of
  bindings — the arm is sound by construction, and a reader reasons about it
  without tracking which alternative matched.
- Exhaustiveness is unaffected: the usefulness algorithm expands an or-pattern
  into the row-union of its alternatives (`src/compiler/types/src/exhaust.rs`),
  a step orthogonal to binding consistency.
- The load-bearing invariant is the pair of checks above (canon name-set
  equality + post-solve per-name type equality); either being weakened would
  re-open the unsound programs this decision forecloses.
- The rejection is a common authoring mistake, so its diagnostic should read
  like a helpful explanation ("each option in a `|` pattern has to bind the same
  names…"), not a terse mismatch — a content item for the friendly-diagnostics
  work.

## Conventions

ADRs describe Ipê on its own terms. This decision is stated as a standalone Ipê
rule, derived from the soundness requirement that a case arm's body sees a fixed
binding environment — not from any external implementation or parity.
