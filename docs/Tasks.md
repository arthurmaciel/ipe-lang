# Task combinators — the three-axis taxonomy

> Coordinator-ratified design (2026-07-05). Governs the Task composition
> surface; `parallel2..5` is a post-parity addition (#131), `do` is Idea 7
> (deferred post-parity). Everything else is shipped.

Three orthogonal axes classify every way of combining Tasks:

1. **Value dependence** — does a later computation need an earlier one's
   *result value*?
2. **Execution dependence** — do the effects run in order (sequential) or
   concurrently (parallel)?
3. **Arity shape** — heterogeneous (different types, fixed arity) or
   homogeneous (`List` of one type, any length)?

**Key law: dependent values ⇒ sequential execution.** A step that needs
another's output cannot run alongside it. So the "dependent + parallel" cells
are *logically empty*, and the table has six rows, not eight.

| Combinator | Values | Execution | Shape | Signature |
|---|---|---|---|---|
| `do` (Idea 7) / `Task.andThen` | **dependent** | sequential (forced) | heterogeneous | `(a -> Task e b) -> Task e a -> Task e b` |
| `Task.map2..5` | independent | sequential | heterogeneous | `(a -> b -> c) -> Task e a -> Task e b -> Task e c` |
| `Task.parallel2..5` (#131) | independent | **parallel** | heterogeneous | `(a -> b -> c) -> Task e a -> Task e b -> Task e c` |
| `Task.sequence` | independent | sequential | homogeneous | `List (Task e a) -> Task e (List a)` |
| `Task.parallel` | independent | **parallel** | homogeneous | `List (Task e a) -> Task e (List a)` |

(`do` is syntactic sugar over `andThen` — same row. A dependent chain of
same-typed steps is also `do`/`andThen`; `sequence` is the *independent*
special case.)

## Semantics per row

- **`do` / `andThen`** — each step may use prior results; a failing step
  stops the chain; later steps are **never started**.
- **`map2..5`** — Elm-compatible: runs in argument order; an early `Err`
  means later tasks are **never started** (their effects don't fire). Use
  when the *effects'* order matters even though the *values* are independent
  (e.g. two appends to one log).
- **`parallel2..5`** — spawn all, await all; latency = max, not sum.
  Fail-fast with **sibling abort** (same discipline as `Task.parallel`,
  #65); when several fail, the **leftmost** error is reported (positional —
  deterministic across runs, chronological would not be). Signature mirrors
  `map2..5` exactly — the families differ in the execution bit only; the
  tuple form is derivable (`parallel2 Tuple.pair ta tb`).
- **`sequence` / `parallel`** — the homogeneous counterparts of the same
  two rows; `parallel` carries the #65 sibling-abort semantics.

## Choosing

| You have… | Use |
|---|---|
| steps that feed each other | `do` / `andThen` |
| independent steps, effect order matters | `map2..5` (or `do`) |
| independent steps, want max-latency win | `parallel2..5` |
| a list of same-typed tasks, ordered effects | `sequence` |
| a list of same-typed tasks, concurrent | `parallel` |

## Contract notes

- **Data independence ≠ effect independence.** The compiler cannot verify
  that two tasks' *effects* don't interact through the outside world. The
  `parallel*` contract is explicit: *argument order is not effect order* —
  if ordering matters, use the sequential row.
- **No rollback.** Fail-fast abort skips/cancels *future or in-flight*
  siblings; effects already fired stay fired (see `Db.withTransaction` for
  all-or-nothing).
- `Task.run` / `Task.perform` (runners, not combinators) are slated for
  removal post-parity (#128) — Tasks are run by the boundary (`main`,
  handler return, `Cmd.perform`), never by user code.
