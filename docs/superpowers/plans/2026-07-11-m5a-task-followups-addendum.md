# Addendum (2026-07-11): #32 M5a Task follow-ups executed via the Class-4 spec

The original plan `2026-07-02-m5a-task-followups.md` was NOT found on this HEAD
(it appears to have been pruned before implementation). #32 was executed instead
from the CONSOLIDATED spec `docs/architecture/class4-pattern-lowering-fix-spec-2026-07-09.md`
**Item E**, which supersedes the stale plan and re-anchors every code / line
reference. A future reader must NOT resurrect the 2026-07-02 plan's original
numbers.

Renumbering that actually shipped (the spec's own re-check flagged some of its
numbers had ALSO drifted by implementation time — verified against
`crates/sky_diagnostics/src/code.rs` at implementation HEAD):

- **E1 (annotation-path arity ICE)** — the plan's `SKY-T0015` was taken by
  `RefutablePatternParameter`; the spec proposed `SKY-T0016`. `SKY-T0016` was
  still free at implementation HEAD, so **`SKY-T0016`** is what shipped
  (`TypeError::TaskArity { found: usize }`, emitted from
  `normalize_annotation_ty`'s mis-arity arm in `crates/sky_types/src/constrain.rs`).

- **E2 (ctor-field mis-arity ICE)** — reuses `TypeError::TaskArity` / `SKY-T0016`
  (no new code). A new `Lowerer::task_arity_in_canon` predicate + a Gate 0a in
  `lower_enum` (`crates/sky_lower/src/lower.rs`) fails closed BEFORE
  `ir_type_from_canon`'s `"Task"` catch-all `CompilerBug` is reachable. The spec's
  proposed `SKY-L0127` for this half was BOTH stale (already allocated to
  `a value holding a function is used more than once`) AND unnecessary — E2 rides
  on E1's type code.

- **E3 (Task/Cmd/Sub-in-ctor-payload)** — DECISION: **ACCEPT** (spec's recommended
  branch a). Verified at implementation time by building + running
  `tests/golden/m5a_ctor_task_ok` (`type Job = Job (Task Error Int)`): `skyc`
  lowering + emitted-Rust `cargo build` + `cargo run` all succeed, printing `ok`.
  #87's derive-demotion fixpoint degrades the non-derivable enum gracefully, so
  NO new rejection / no new `SKY-L` code was added — symmetric with Item B's
  function / `Result` / `Maybe` precedent.

Net taxonomy delta: **+1 code** (`SKY-T0016`); 88 → 89. Count gates bumped in
lockstep across `code.rs` (two asserts) and `skyc/src/lib.rs`
(`code_index_lists_every_code`).

Note: constructing a Task-in-payload VALUE (`Job (Task.succeed 1)`) currently
trips an UNRELATED pre-existing `SKY-T0001` (the ctor field's normalized
`Task Error a` vs the kernel's unary `Task a` do not unify at the construction
site). That is a distinct concern from #32's fail-closed ICE gate and is left for
independent tracking; the E3 acceptance fixture therefore exercises the
declaration + `case`, not value construction.
