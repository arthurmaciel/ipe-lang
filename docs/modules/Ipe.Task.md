# Ipe.Task

A `Task Error a` is a **description of an effect**, not the effect itself. Reading
a file, printing a line, querying a database — each is a `Task` value that does
nothing until the runtime runs it. Your program builds up a description of what
should happen; the runtime is the single place it happens. This is what keeps the
rest of the language [pure](../guide/pure-functions.md).

## The mental model

Three knots trip up newcomers. Untangle them and the module falls into place.

- **Building a task runs nothing.** `Io.println "hi"` does not print — it returns a
  `Task Error ()` that prints *when run*. So a task you never hand to the runtime
  (never returned from `main`, an `update` `Cmd`, or another task) never happens.
  A discarded task is a silent no-op, not a side effect.
- **The error channel is implicit.** Every task is `Task Error a`: it either
  succeeds with an `a` or fails with a typed `Error`. You never thread the error
  by hand — `map` and `andThen` skip a failed task automatically, and `onError` is
  the one place you catch it. `Error` is a matchable value, never a bare `String`.
- **Sequencing vs. combining are different tools.** `andThen` sequences: the next
  step *depends on* the previous result. `map2`..`map5` combine *independent*
  values with effects run in order. `parallel` runs independent tasks *at once*.
  Choosing among them is choosing what may depend on what, and what may run
  concurrently.

## A worked example

[`examples/shapes/script/release-preflight`](../../examples/shapes/script/release-preflight)
is a plain-`main` batch program (no TEA loop): a release check that announces the
run, fires three independent checks concurrently, and prints a report.

The three checks are ordinary tasks — each a `Task Error String`:

```ipe
checkBuild : Task Error String
checkBuild =
    Task.succeed "build     ok  — artifact present"
```

`main` is a `do` block. Read it top to bottom; each line is a step the runtime
runs in order:

```ipe
main =
    let version = "1.4.0" in
    do
        Io.println ("Preflight for v" ++ version)   -- bare : run for its effect, discard the ()
        results <- Task.parallel                    -- `<-` : run all three at once, bind the List
            [ checkBuild
            , checkChangelog
            , checkGitClean
            ]
        Io.println (report version results)
```

Two forms appear. A **bare** line runs a task for its effect and discards the
result (here a `()` from printing). A **`<-`** line runs a task and binds its
success value — `results` is the `List String` that `Task.parallel` collected.
Because the checks are independent, `parallel` runs them concurrently: the latency
is the slowest check, not the sum of the three.

Written by hand without `do`, the same logic is a right-drifting
`Task.andThen` pyramid; the `do` block *is* that chain, read top-to-bottom
instead of nested. See the [`do`-notation idiom](../idioms/do-notation.md).

## Why it is shaped this way

- **Effects as values keep the language pure** ([Correctness](../../PRINCIPLES.md)).
  A function that returns a `Task` still returns the same task for the same
  arguments; only the runtime turns descriptions into actions, so the rest of your
  code stays referentially transparent and testable.
- **A typed error channel makes failure explicit and matchable.** `Task Error a`
  cannot silently swallow an error the way an exception can; the type forces a
  caller to route it (`onError`, `attempt`, or propagate it up to `main`).
- **`parallel` vs. `sequence` puts concurrency in the type.** Independent work is
  visibly independent, so a reader sees at a glance what runs at once.

## References

- `ipe doc Ipe.Task` — the per-symbol reference (every combinator with a snippet).
- [`do`-notation](../idioms/do-notation.md) — the preferred shape for two or more
  chained tasks.
- [The Elm Architecture](../guide/the-elm-architecture.md) — how a task's result
  comes back as a `Msg` via `Cmd` in an interactive program.
- [Pure functions](../guide/pure-functions.md) — why effects must be values.
- Sibling references: [`Ipe.Result`](Ipe.Result.md) (the `fromResult` bridge),
  [`Ipe.List`](Ipe.List.md) (the list `parallel` / `sequence` collect into).
