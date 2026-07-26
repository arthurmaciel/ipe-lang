# Dropping `Task.run` + `Task.perform` from the Ipê surface (#128)

> Backlog item #128 (Post-completion): "Drop `Task.run` + `Task.perform`
> from the Ipê surface (#116 companion). Departure — first consumer of
> the CI example-patch-queue." Spec+plan written 2026-07-10.
> Design-only; no code has changed.
>
> **One-line decision:** after #116's auto-run entry contract lands,
> the two *surface* bindings `Task.run` and `Task.perform` are removed
> from the resolver with a dedicated removed-surface diagnostic +
> `ipe fix` codemod; the **internal** `task_run` runtime kernel and the
> `KernelFn::TaskRun` lowering path stay (the entry boundary and the
> `let _ = TaskExpr` auto-force still emit them). `Task.perform` is
> deleted outright and its name is **reserved** for a possible future
> Elm-shaped `Task.perform/attempt` (C.4).

## Problem statement

Ipê exposes two synchronous Task-forcing kernels:

- `Task.run : Task e a -> Result e a` — `upstream:sky-stdlib/Sky/Core/Task.sky:85-88`
- `Task.perform : Task e a -> Result e a` — same file, lines 90–96;
  documented there as "the legacy name" for the identical operation.

Both exist so that (a) `main` can end in `|> Task.run` and (b) code can
force a Task mid-flow into a `Result`. Elm has neither: effects stay in
`Task`/`Cmd` until the runtime boundary executes them, which is the
architecture Ipê wants (`docs/divergences-from-elm.md` ER/E-series
framing). #116 (spec: `docs/architecture/adopt-from-sky-v0172.md`,
Option C) removes reason (a): a `main` that evaluates to a Task is
auto-run at the entry boundary, adopting the reference's own v0.17.3
`pipeCollapsesTask` direction. #128 finishes the job: with (a) gone, the
surface bindings only remain useful for (b) — synchronous mid-flow
forcing — which is exactly the escape hatch that lets effectful code
masquerade as pure and that Elm deliberately does not offer.

Current inventory in the Rust port (all confirmed 2026-07-10):

| Layer | Location |
|---|---|
| Kernel variants | `src/compiler/kernels/src/lib.rs:485-487` (`TaskRun`, `TaskPerform`), decls at 1613–1614 (both `d("Task","run"/"perform", 1, Pure, "task_run")`), `ALL` at 2589–2590 |
| Constrain schemes | `src/compiler/types/src/constrain.rs:3564-3571` — both `fun(task(var(0)), result(error_ty(), var(0)))` (error channel pinned to `Error`) |
| Resolver (surface) | `src/compiler/lower/src/lower.rs:8520-8521` — `("Task","run")` / `("Task","perform")` → `Callee::Kernel(…)` |
| Discard-context detection | `src/compiler/lower/src/lower.rs:7052-7053` |
| Backend naming | `src/compiler/backend/rust/src/naming.rs:583-585` — both → `task_run` |
| Entry elision | `src/compiler/backend/rust/src/emit_expr.rs:6659-6677` — `Call(TaskRun\|TaskPerform, [t])` at `sky_main` already elides the wrapper |
| Runtime | `src/runtime/rust/src/task.rs:191-195` — `task_run` = `block_on`, panic→Err, total |

Corpus pressure: 95 occurrences across 35 files in `upstream:examples/`
(top: `17-skymon` 18, `38-composite-ui-multibackend` 6, `16-ipehess` 6,
`00-standard-libs` 6); 7 occurrences in the port's own
`tests/golden/` corpus (`http_stream_id` 4,
`wildcard_lambda_pany` 2, `poly_task_on_error` 1). This is
why #128 is "the first consumer of the CI example-patch-queue"
(`docs/divergences-from-sky.md` §6.9, accepted 2026-07-05).

## Decision

### D1 — Hard ordering: #116 first, #128 second, never merged

#128 is unshippable before #116: without auto-run, removing `Task.run`
leaves no way to write a CLI `main` at all. They also must NOT land as
one change — #116 is output-neutral parity adoption (the reference
itself is heading there), #128 is an Ipê departure. Keep the departure
in its own commit with its own divergence entry.

### D2 — Removal = dedicated diagnostic, not a name-not-found hole

Do **not** simply delete the resolver arms at `lower.rs:8520-8521` —
that would demote a removed, well-known surface to a generic
unknown-qualified-name error. Instead the resolver keeps *recognising*
`("Task","run")` and `("Task","perform")` and maps them to a new
removed-surface diagnostic (allocate the next free IPE-N code;
explain page in `src/compiler/diagnostics/explain/`) whose message is a
teacher, per the compiler-as-kind-teacher rule:

- for `expr |> Task.run` / `Task.run expr` in `main` or a top-level
  binding: "`Task.run` was removed — `main` runs Tasks automatically;
  delete the `|> Task.run`";
- for `let _ = Task.run t`: "discarded Tasks are auto-forced; write
  `let _ = t`";
- for a genuinely mid-flow `let r = Task.run t in …`: "synchronous
  forcing was removed; keep the pipeline in Task — use `Task.andThen`,
  `Task.onError`, `Task.mapError`, or move the decision into the Task"
  (fix-it only where mechanical, see D4).

"Make invalid states unrepresentable" applied to the surface: the name
exists in the diagnostic table precisely so it can never silently mean
anything else.

### D3 — Internal kernel machinery stays; only the surface goes

`KernelFn::TaskRun`, its constrain scheme, naming row, emitter arms,
and the runtime `task_run` function all remain: the #116 auto-run entry
lowering and the `let _ = TaskExpr` auto-force lowering are their
callers (compiler-synthesised, not user-reachable). What is removed:

- the surface resolver mapping (replaced per D2);
- `KernelFn::TaskPerform` entirely (variant, decl, `ALL` row, scheme
  `constrain.rs:3570-3571`, naming arm, pretty arm, discard-context
  arm) — it was a pure alias with zero independent behaviour;
- doc/illustrative uses inside explain pages `IPE-L0119.md:50` and
  `IPE-L0126.md:33` (rewrite the examples to the auto-run shape).

`Cmd.perform` is untouched — it is a different operation
(`Task err a -> (Result err a -> msg) -> Cmd msg`,
`upstream:sky-stdlib/Std/Cmd.sky:40-45`), not an alias. The **name**
`Task.perform` is reserved: do not rebind it in #128; if C.4 Elm-core
coverage later wants Elm's `Task.perform : (a -> msg) -> Task Never a
-> Cmd msg` / `Task.attempt`, the name is clean for it. Note this in
the divergence entry so nobody "helpfully" re-adds the alias.

### D4 — Migration: `ipe fix` codemod + CI patch queue

Ship a `ipe fix` migration in the same change (the patch-queue design
says mechanical departures ship with their migrator, and CI generates
the example patches BY RUNNING the migrator — the queue doubles as the
migrator's E2E test):

- mechanical rewrites: trailing `|> Task.run` on `main`/top-level
  Task-typed bindings → deleted; `Task.run t` in discard position →
  `t`; identical for `Task.perform`;
- non-mechanical shapes (`let r = Task.run t in case r of …`) are NOT
  auto-rewritten — the codemod reports them; the upstream-example
  patches for those files are written by hand once and live in
  `tests/example-patches/…` per §6.9.

Oracle policy (per §6.9): #128 is **output-neutral** — deleting a
forced wrapper under auto-run semantics changes no observable output —
so patched examples keep byte-equivalence against the Go oracle running
the unpatched source. No `oracle_divergence` flags for this item; the
departure is recorded in `docs/divergences-from-sky.md` §planned →
promoted to a live entry when it lands.

### Alternatives considered and rejected

1. **Keep `Task.run`, drop only `Task.perform`.** Rejected: leaves the
   sync-forcing escape hatch open, which is the item's actual target;
   half-departures cost a divergence entry each anyway.
2. **Deprecation warn for one release instead of removal.** Rejected:
   Ipê has no published releases yet; pre-push is the cheapest moment
   this removal will ever have (BACKLOG places it Post-completion,
   after the corpus gate exists to prove the patches).
3. **Silent resolver deletion (name-not-found).** Rejected per D2 —
   hostile DX, violates the kind-teacher rule.
4. **Rebind `Task.perform` to Elm's shape in the same change.**
   Rejected: couples a removal to a new-feature design; reserved-name
   note keeps the door open for C.4.

## Implementation plan (for a cold swarm lane)

Order of operations (after #116 is landed and green):

1. Add the removed-surface diagnostic code + explain page (teacher
   messages per D2, with the three shape-specific hints).
2. `src/compiler/lower/src/lower.rs:8520-8521`: map both names to the new
   diagnostic (carry the call-shape context needed to pick the hint —
   the lowerer knows discard/entry/mid-flow context via the existing
   sync-discard detection at 7052–7053 and the entry detection the
   #116 work adds).
3. Delete `KernelFn::TaskPerform` everywhere
   (`sky_kernels/src/lib.rs:486-487,1614,2590`;
   `constrain.rs:3570-3571`; `naming.rs:585` arm;
   `sky_ir/src/pretty.rs:521`; `lower.rs:7053` half). Exhaustive-match
   friction is the checklist: rustc will list every arm.
4. `ipe fix` codemod (mechanical rewrites per D4) + its tests.
5. Regenerate the port's own 7 golden usages via the codemod
   (`tests/golden/http_stream_id`, `poly_task_on_error`,
   `wildcard_lambda_pany`) — these become the first migrator
   fixtures.
6. Generate the upstream-example patch queue entries
   (`tests/example-patches/…`) by running the codemod over the 35
   upstream files; hand-write the non-mechanical residue.
7. Rewrite the two explain-page examples (IPE-L0119, IPE-L0126).
8. Promote the §6.9 planned-divergence to a live `divergence:` entry
   (removal + reserved-name note + rationale).

## Test plan

- `i128_removed_task_run_entry` — `main = t |> Task.run` → assert the
  new diagnostic with the delete-the-pipe hint (golden stderr).
- `i128_removed_task_run_discard` — `let _ = Task.run t` → assert the
  auto-force hint variant.
- `i128_removed_task_run_midflow` — `let r = Task.run t in …` → assert
  the keep-it-in-Task hint variant.
- `i128_removed_task_perform` — same three shapes for `Task.perform`.
- Codemod unit tests: each mechanical rewrite idempotent (`ipe fix`
  twice = once), non-mechanical shape reported not rewritten.
- E2E (`IPE_E2E=1`): the three regenerated goldens (step 5) build, run,
  and stay byte-equivalent to their pre-existing `expected_go.txt`
  (output-neutrality proof).
- Sweep: patched upstream examples build+run with outputs equal to the
  Go oracle on unpatched sources (the §6.9 contract); patch-apply
  failure fails CI loudly by design.
- Negative pin: a test asserting `Cmd.perform` still resolves (guards
  against over-eager deletion).
