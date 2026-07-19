Status: Accepted

# 0009. Kernel-registry integrity: single-source arity + sealed Match combinators

## Context

The kernel registry carried several classes of hand-maintained redundancy with
no machine-enforced consistency, each an exit-0-then-cargo-fail hole (a
well-typed program that `ipe` accepts but the emitted Rust fails to compile):

- Canon's `qual_vars` could carry an unresolved kernel name
  (`VarHome::Kernel(None, …)`) when every non-excluded qualifier should map to a
  concrete `StdlibKernel` id.
- `callee_arity` kept a hand-written per-variant arity table that could drift
  from `StdlibKernel::decl().arity` with no cross-check, so a program could pass
  the type-checker's scheme yet saturate/eta-expand against the wrong arg count
  at IR level.
- `Match::from_parts_unchecked` was `pub`, letting a body-only rewrite lose or
  reorder arms and forge an invalid match expression (an invalid-states
  finding).

## Decision

- **Make the registry the single source of truth for arity.** Assert in
  `canon_equals_registry` that every non-excluded `qual_vars` member carries
  `Some(id)`, panicking loudly on drift. Delete the `Option<StdlibKernel>`
  wrapper and redundant `module`/`name` fields from `VarKernel`/`VarHome::Kernel`
  so every `VarKernel` carries a concrete `StdlibKernel` *by construction*, and
  fold the unused paths into `unreachable!()` compiler-bug diagnostics. Collapse
  `callee_arity`'s hand table to `Ok(usize::from(k.decl().arity))`, deleting
  ~1,200 lines and the legacy ~1,000-line string-match table in `lower.rs`.
- **Seal Match construction.** Replace the `pub from_parts_unchecked` escape
  hatch with two combinators — `map_bodies` (infallible) and `try_map_bodies`
  (fallible) — that transform the scrutinee and arm bodies only, leaving
  patterns untouched by closing over the original arms.

Rejected alternative: keeping the parallel hand-written arity/kernel tables and
adding "just one more" cross-check test each time drift is found — the tables
are structurally redundant, so the only durable fix is to delete them and derive
from the one authoritative source.

## Consequences

- After the wrapper is deleted, an unresolved kernel is *unrepresentable*:
  future regressions are type errors at registry-construction time, not runtime
  gaps. `callee_arity` drift is structurally impossible; its test becomes a
  source-scan assertion that the delegating one-liner persists.
- A Match expression can never again be reconstructed with missing or reordered
  arms after a body-only rewrite — the shape is sealed by the combinator's
  closure over the original patterns. All five passes that used the old escape
  hatch (clone-capture, multiuse-clone, var-to-apply, clone-free-target,
  substitute-var) stay byte-identical.
- **Invariant that must keep holding:** anything the compiler needs to know
  about a kernel (arity, home, id) is read from `StdlibKernel::decl()`/the
  registry, never re-declared alongside it; any code that transforms a match
  must go through `map_bodies`/`try_map_bodies`, never a raw parts constructor.
