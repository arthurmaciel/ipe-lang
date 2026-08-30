# Tasks

A `Task Error a` is a *description* of work that, when the runtime runs it, either
succeeds with an `a` or fails with a typed `Error`. It is how an Ipê program talks
about effects — reading a file, calling a service, printing a line — as ordinary
values you build and compose before anything happens.

## The mental model

Three knots.

- **A Task is a description, not the effect. Building one runs nothing.** `Io.println "hi"`
  does not print; it *builds a task that would print*. The runtime is the single
  place an effect actually happens, and it runs exactly one task: `main`.
  Everything else is plumbing that assembles that one task. This is why effects
  stay referentially transparent — you can pass a task around, put it in a list,
  run it twice, without it firing early.
- **`do` sequences dependent steps, and the first failure short-circuits.** When
  step two needs step one's result, `do` (sugar over `Task.andThen`) binds each
  with `<-` and reads top to bottom. If any step fails, the `<-` stops there — the
  steps below simply never run. You write the happy path once; failure falls
  through on its own, no per-step error check.
- **The error channel is a typed, matchable `Error` — recovery is composition.**
  A failure carries an `Error` value (classified by kind, not a bare string).
  `Task.onError` catches it and returns a *new* task, so "try, and on failure do
  X" is ordinary function composition — there is no separate `try`/`catch`
  mechanism bolted onto the language.

## A worked example: a deploy runner with rollback

The example under
[`examples/shapes/script/task-deploy-steps`](../../examples/shapes/script/task-deploy-steps/src/Main.ipe)
runs a deploy as a chain of dependent steps, then catches a failure and rolls
back.

Each step is a task that either succeeds with a log line or *fails with a typed
error* — the failure is a value, built where the check is:

```ipe
check label ok =
    if ok then
        Task.succeed ("ok   — " ++ label)

    else
        Task.fail (Error.invalidInput ("failed — " ++ label))
```

The deploy is a `do` block: each `<-` binds the previous step's success, and if a
step fails the chain short-circuits — the steps below it never run. Read it top to
bottom, not as a nested `andThen` pyramid:

```ipe
deploy migrationsOk =
    do
        build <- check "build compiled" True
        _ <- Io.println ("  " ++ build)
        migrated <- check "migrations applied" migrationsOk
        _ <- Io.println ("  " ++ migrated)
        released <- check "traffic shifted" True
        _ <- Io.println ("  " ++ released)
        Task.succeed "deploy succeeded"
```

Recovery is `Task.onError`: it catches a failure on the typed channel and hands
back a rollback task, turning a failed deploy into a clean result instead of a
crash:

```ipe
attempt name migrationsOk =
    do
        _ <- Io.println (name ++ ":")
        outcome <-
            deploy migrationsOk
                |> Task.onError rollback
        Io.println ("  => " ++ outcome)
```

And `main` is itself the one task the runtime runs — everything above merely
*described* work:

```ipe
main =
    do
        _ <- attempt "green deploy" True
        attempt "bad deploy" False
```

Running it (`ipe run`) shows the green deploy running every step, and the bad
deploy short-circuiting at the failed migration (the traffic-shift step never
runs) then rolling back:

```
green deploy:
  ok   — build compiled
  ok   — migrations applied
  ok   — traffic shifted
  => deploy succeeded
bad deploy:
  ok   — build compiled
  ! InvalidInput: failed — migrations applied
  rolling back
  => rolled back
```

## The why

Task-as-a-value is [soundness][principles] for effects: because building a task
does nothing, the type `Task Error a` fully describes *what could happen* before
anything does, and the runtime is the single, auditable place an effect fires.
There is no hidden side effect lurking in an innocent-looking expression.

The typed `Error` channel is [make invalid states unrepresentable][principles]
carried into failure: a task cannot fail with an untyped, unmatchable value, so a
handler like `onError` can classify and route the failure by kind. And `do`'s
automatic short-circuit is [ease of use][principles] — the happy path is written
once, failures propagate for free, and the code reads as a straight sequence
rather than a pyramid of nested error checks.

[principles]: ../../PRINCIPLES.md

## References

- **Per-symbol reference:** `ipe doc Ipe.Task` — every combinator with a verified
  example. `ipe doc Ipe.Task.andThen`, `ipe doc Ipe.Task.onError`, and
  `ipe doc Ipe.Task.parallel` cover sequencing, recovery, and concurrency.
- **Sibling guides:** [Results](result.md) — `Result` is a task that has already
  settled; `Task.fromResult` bridges them. [Lists](list.md) — `Task.sequence`
  turns a `List (Task Error a)` into one task. The typed failure type lives in
  `Ipe.Error` (see `ipe doc Ipe.Error`).
- **Concepts:** [The do-notation idiom](../idioms/do-notation.md) — how `<-` and
  the bare-statement form desugar to `andThen`. [The Elm Architecture](the-elm-architecture.md)
  — where tasks fit in a full `init`/`update`/`view` app. The
  [`release-preflight`](../../examples/shapes/script/release-preflight/src/Main.ipe)
  example shows `Task.parallel` for independent, concurrent steps.
