Status: Accepted
Date: 2026-07-09

# 0011. Emitter clone/borrow discipline (alias double-move, decoder-thunk destructures, field-access copy elision)

## Context

Three emitter borrow-discipline gaps produced `cargo` E0382/E0507 partial-move
errors or gratuitous deep copies. All three sit downstream of the non-`Copy`
move seal (ADR 0002 / 0007) and are about *where* the emitter must clone versus
where it must not.

## Decision

- **§1 — refutable match-arm alias over a non-`Copy` payload.** `case m of Just
  ((a, b) as w) -> …uses a, b AND w` in a whole-value (by-move) arm emits
  `Just(w @ (a, b))`, and Rust's default by-move binding makes `w` and `a`/`b`
  fight over the tuple (E0382). Add a predicate `is_dispatch_free(pat)` (true for
  `Var`/`Wildcard`/`Tuple`/`Record` nesting; false for `Ctor`/literal/`Slice`).
  Reject aliases whose inner pattern needs dispatch with `SKY-L0127`; for
  dispatch-free inners, rewrite the alias into bind-then-destructure-from-a-clone
  (the same shape `#96`'s `emit_binding_stmts` proved sound for irrefutable
  binders). Rejected: matching the scrutinee by reference throughout the arm (a
  far larger redesign) or a full last-use liveness pass (out of scope for a
  mechanical fix). STR/LIST-mode arms already match by ref and are unaffected.

- **§2 — decoder-thunk coverage for tuple/record destructures.** `#89` wrapped a
  single Decoder-bound name in a zero-arg thunk (Decoders are `!Clone`) but a
  destructure binding Decoder-typed components (`let (d1, d2) = buildPair ()`)
  fell to the plain destructure path and double-moved. Generalize the thunk to
  *any* destructure binder: mint one zero-arg thunk over the whole value, and at
  each free read re-destructure a fresh thunk call keeping only that one name
  (reusing `Expr::Destructure` as the projector — no new IR node). Gate whenever
  the aggregate type is-or-contains `IrType::Decoder` (same unconditional gate as
  `#89`, avoiding "some names via thunk, others direct" in one binding). Rejected:
  new element/field accessor IR nodes, thunking only Decoder elements (would
  split one `let` into heterogeneous bindings, not representable), or inlining the
  thunk per read (re-runs `buildPair ()` each time).

- **§3 — copy elision on record field access.** `#139` added an unconditional
  `.clone()` to every field access; rustc does *not* elide it for heap types
  (String/Vec), so every `String` field read was a deep copy, O(n²) in
  per-element render loops. Add `field_ty: IrType` to `Expr::Access`, solve it at
  lowering, and emit bare access for provably-`Copy` types
  (Int/Float/Bool/Char/Unit/Order/Decimal/ErrorKind), `.clone()` otherwise.
  Rejected: a general last-use liveness pass (larger, higher risk of the unsafe
  direction), or an Arc/Rc record representation (ripples through every
  construction/update site).

## Consequences

- **Invariants that must keep holding:** the backend must never emit an alias
  over a dispatch-*needing* inner without `SKY-L0127` having rejected it upstream
  (backend-side `is_dispatch_free` checks are defensive, not the primary gate).
  A `Generic(_)` field type conservatively returns `false` from the Copy
  predicate (it may monomorphize to Copy, but the backend has no per-call-site
  visibility) — always clone; user enums derive `Clone` not `Copy`, so also
  clone. When `ir_type_from_ty` can't solve a field type, fall back to
  `IrType::Generic` to keep the `.clone()` — never a regression versus the old
  unconditional behavior.
- The decoder-thunk rewrite shares the shadow-walk rules with `#89`'s
  `rewrite_var_to_apply` (both stop at `Let`/`Destructure`/`Lambda`/`Match`);
  capture analysis runs on the original canon expression, not the lowered IR.
