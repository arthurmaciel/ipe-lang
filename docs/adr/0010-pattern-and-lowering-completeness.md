Status: Accepted
Date: 2026-07-09

# 0010. Pattern & lowering completeness (interp literals, function payloads, nested sub-patterns, local type shadowing)

## Context

Four independent lowering/canonicalisation gaps each let malformed or unhandled
shapes through, several as exit-0-then-cargo-fail holes. They share a theme:
recognition or region-threading that worked for one shape was not applied
uniformly to its siblings.

## Decision

- **A — literals in string interpolation.** The `{{…}}` argument resolver
  already recognized numeric literals before the bare-identifier fallback (which
  would otherwise intern a literal as a variable name and leak an unresolved
  `VarLocal` past canonicalisation). Extend the same recognizer, *before* the
  ambiguous `.`-split branch, to string/bool/char literals. Rejected: delaying
  recognition until after identifier/access resolution (too late — a literal
  containing a `.` mis-splits), or teaching post-canon `VarLocal` to recognize
  literals (violates the invariant that canon leaves no unresolved locals).

- **B — function payloads in `Maybe`/`Result`/constructor payloads.** Two
  lowering gates rejected function-valued payloads unconditionally, on the stale
  assumption that a function field can't satisfy the derives set. The `#87`
  backend `enum_derivable` fixpoint now gracefully degrades non-derivable enums
  (skip `#[derive]`, hand-write `SkyStringify` rendering `"<fn>"`). So add
  `Maybe`/`Result` to the opaque-boxed-wrapper exemption and remove the
  declaration-time gate, relying on that machinery. Rejected: re-inventing a
  backend clone gate (already exists), or syntactically inspecting arity
  (violates "no textual surgery"). The region-based
  `reject_function_through_type_var` gate stays — a function through a genuinely
  *polymorphic* type var into an ordinary user enum still rejects.

- **C — record/list sub-patterns nested in constructor payloads.** Insert
  regions for every constructor sub-pattern in `constrain_pattern`'s `PCtor`
  arm (mirroring lambda params), and make the payload-pattern lowerers instance
  methods so they can consult `self.region_ty()`. Nested records reuse
  `lower_record_pat` unchanged. Nested list patterns — which Rust cannot
  slice-pattern inline on a `Vec` field — lower to a plain `Pat::Var` binder plus
  an arm-level guard checking length/shape, with elements recovered by indexing
  in an IR-level `Expr::Let` prelude (never rendered text). Rejected: elevating
  list patterns to the top-level type system (out of scope), or a scrutinee-level
  slice binding (forbidden "textual surgery").

- **D — local type shadowing a dep-imported type.** Dep-vs-dep name clashes were
  rejected (`SKY-N0012`) but a local `type` shadowing a dep-imported type was
  silently skipped, leaving ctors pointing at the local type while
  `type_home_map` pointed at the dep — surfacing as a confusing downstream type
  mismatch. Add a pre-pass local-vs-dep check emitting the *same* `SKY-N0012`,
  run before `type_home_map` is mutated so a same-module duplicate still gets the
  better per-module span. Rejected: lazy downstream detection (already the buggy
  status quo), or a new bespoke diagnostic (the existing one is correct).

## Consequences

- **Invariants that must keep holding:** canonicalisation leaves no unresolved
  `VarLocal`; the interp resolver distinguishes literals from identifiers
  *before* the `.`-split. Regions inserted by the constrainer persist through
  lowering at every nesting level. A nested list pattern is *refutable*
  (`Just []` ≠ `Just (h :: t)`), so such a program must have a fallback arm and a
  non-matching length falls through the guard, never panics. The check order is
  strict (dep pre-pass → local-vs-dep → per-module loops) and applies to both
  unions and aliases; two modules independently declaring the same name with no
  import between them stay legal.
- The Task-arity ICE (Item E of the original spec) was carried forward to a
  separate follow-up plan and is not part of this decision.
