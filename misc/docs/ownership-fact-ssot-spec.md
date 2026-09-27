# Spec — one ownership fact (Copy / Clone / move / eval order) shared by lower and backend

Crates: `ipe_ir` (`ir.rs`, `analysis.rs`, `let_inline.rs`), `ipe_lower`
(`clone_class.rs`, `lower.rs`, `func.rs`), `ipe_backend_rust`
(`emit_expr/analysis.rs`, `emit_expr/expr.rs`, `emit_expr/patterns.rs`,
`emit_doc.rs`, `lib.rs`). Principles: SEAL, SSOT, make invalid states
unrepresentable (exhaustive match). Lane tier: high (emit).

## Class-closing property

Whether a value is Copy, Clone, or must move — and in what order a call's
arguments are evaluated — is computed ONCE, in `ipe_ir`, by exhaustive
structural recursion over `IrType`/`Expr` (no `_ =>` arm), carried in the
lowered `Program`, and CONSUMED by the backend. The backend never
re-derives it from its own predicate. Where a second form cannot import the
first, a `const` assertion ties them at build time.

Every instance in this class is lower and backend disagreeing about
ownership → ipe accepts, cargo fails with E0382/E0507/E0599 (SEAL), or lower
refuses (L0135/L0126) a program the emitter would compile.

## Why the instances exist (origin/main 983fdc404)

- `backend/rust/src/emit_expr/analysis.rs:46` `ir_type_is_definitely_copy`:
  hand-synced mirror of lower's `clone_class` CopyLeaf list; no Tuple arm
  (#2942). `ipe_ir::ir_type_is_copy` does not exist on main.
- `ir/src/let_inline.rs:71` `ir_type_contains_task`, and `:65`/`:76`
  `_ => false` in it and `expr_value_is_non_clone`: a composite carrying a
  Task, or a new Expr form, silently counts as Clone (#2968, #2997.2).
- Two enum-payload tables: lower (`lower.rs` ~14378) and backend
  (`backend/rust/src/lib.rs` 1717–1811); `enum_payload_table` /
  `EnumPayloadTable` absent from `ipe_ir` (#2997.1).
- Emitter Access `moves`, `move_field`, row-param gate key on
  `ir_type_has_effect_carrier`, not the clone fact; `TaskRetryWith`,
  `JsonDecList`, TEA thunks build their own `move` closures (#2952).
- L0135 seq check (`clone_class.rs` ~887) uses "effect carrier && NonClone"
  instead of the clone-ability fact; `ir_type_has_effect_carrier` ends in
  `_ => false` (#2933).
- Reversed-arg kernels: lower assumes `Callee::args_in_eval_order`
  (4395ffe9c on main); emitter reverses in three separate places
  (`emit_expr/expr.rs:590`, `emit_doc.rs:1066`, `emit_expr/kernel_calls.rs:28`)
  with no tie to a shared render shape (#2974).
- Nested string-literal arm guard binds `__sgN` by value → moves the part →
  lower refuses reuse (L0135) the programmer intends (#2963).

Stranded fixes (all unmerged, >100 behind main, no PR):
`lane/2942-copy-ssot` (1 commit, `ir_type_is_copy` in `ipe_ir`),
`lane/2933-seq-clone-fact` (7), `lane/2968-walkers` (5, one exhaustive
held-value walk), `lane/2962-enum-payload` (9, on top of 2968-walkers),
`lane/2921-fnonce-cont` (3). #2949 and #2952 are *review residuals of these
unmerged lanes* — the reviewed code is not on main.

## Members

| Issue | Status | Verified by |
|---|---|---|
| #2942 one Copy fact | real on main; fix stranded lane/2942-copy-ssot | `analysis.rs:46`; no `ir_type_is_copy` on main |
| #2949 #2942 review residuals (golden comment archaeology, tuple elision pin) | real; item 1 archaeology visible on main `g_issues/golden_i130_seal.rs:6,75,123` | `git grep clone_class_named_composite` |
| #2933 L0135 seq predicate, effect-carrier catch-all | real; fix stranded lane/2933-seq-clone-fact | `clone_class.rs` ~887 |
| #2952 emitter move predicates / kernel closures consume fact | real | depends on #2933 |
| #2968 `ir_type_contains_task` composites | real on main; fix stranded lane/2968-walkers | `let_inline.rs:71` |
| #2997 one enum payload table; `_ => false`; refusal tests | real | `let_inline.rs:76`; no `EnumPayloadTable` |
| #2974 reversed-kernel emit order tied to `EvalOrder` | real (drift risk) | three reverse sites |
| #2963 borrow nested string-literal guard slots | real | `patterns.rs` `render_arm_pat_alias_safe` |
| #2921 FnOnce continuation | FEATURE, fix stranded lane/2921-fnonce-cont; consumes the fact | — |
| #2950 FnOnce e2e per carrier | test gap of #2921 | — |
| #2880 per-kernel ArgOrder | covered by open PR #3017 (`Closes #2880`) | PR body |

## Implementation plan

1. Land `lane/2942-copy-ssot` rebased: `ipe_ir::ir_type_is_copy` (with Tuple
   arm: Copy iff every part is). Delete `ir_type_is_definitely_copy`; backend
   calls the `ipe_ir` fn. Fix #2949 items 1–3 in the same PR (present-tense
   comments; regen goldens only via `cargo run -p regen-goldens`).
2. Land `lane/2968-walkers` then `lane/2962-enum-payload`: one exhaustive
   `held_value_walk` behind every `ir_type_contains_*`; no `_ =>` arm.
3. `ipe_ir::EnumPayloadTable` built post-prune, stored in `Program`; lower's
   L0135 gate and backend `lib.rs` 1717–1811 both read it (#2997.1).
   Explicit arms in `expr_value_is_non_clone` (#2997.2).
4. Land `lane/2933-seq-clone-fact`; then #2952: `moves`/`move_field`/row-param
   gate read the lowered clone fact; `TaskRetryWith`, `JsonDecList`, TEA
   thunk emitters go through the same hoist.
5. `EvalOrder` render-shape enum in `ipe_ir` (`Generic`, `Reversed`,
   `Bespoke(order)`); one backend fn renders a call from it; the three
   reverse sites collapse into it (#2974). Rebase on PR #3017's `ArgOrder`.
6. Guard slot bound by `ref` (or typed moved/borrowed slot descriptor that
   `PartialMove` derives from) (#2963).
7. Then #2921 (FnOnce continuation) and #2950 e2e per carrier.

## Prove the refusals

- `IPE_E2E=1` fixtures: tuple/record/enum carrying Task reused by value;
  enum-wrapped Task field read through a captured record (E0507 risk);
  non-Clone capture in each carrier (`Cmd`, `Sub`, `Ui`, `WebRoute`, `Task`)
  moved into `Task.andThen`; re-invoked continuation still refused (L0126);
- L0135 driven through an enum-wrapped Task field;
- build-time: a `const` test that every `IrType` variant is classified by
  `ir_type_is_copy` (exhaustive match — adding a variant breaks the build).

Lane tier: high. Guardian: required.
