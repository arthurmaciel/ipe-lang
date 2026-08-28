# `do`-notation

**The idiom:** when a computation is two or more effects run in order — each
possibly using the previous result — write a `do` block instead of a chain of
`Task.andThen`.

## The shape

A `do` block is a sequence of steps, read top to bottom. Two forms of step:

- **`name <- task`** — run `task`, bind its success value to `name`, continue.
- **`task`** — run `task` for its effect and discard the result (a `()`).

The block's value is its last step. A failure anywhere short-circuits the rest —
you never thread the error by hand.

## Why prefer it

Written with `Task.andThen`, a three-step chain drifts to the right in a pyramid:

```ipe
main =
    let version = "1.4.0" in
    Io.println ("Preflight for v" ++ version)
        |> Task.andThen (\_ ->
            Task.parallel [ checkBuild, checkChangelog, checkGitClean ]
                |> Task.andThen (\results ->
                    Io.println (report version results)))
```

The same logic as a `do` block reads straight down —
from [`examples/shapes/program/release-preflight`](../../examples/shapes/program/release-preflight):

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

The two are the *same* program — `do` desugars to that `andThen` chain. The block
form makes the order of effects, and each binding, plain.

## When not to reach for it

A single effect needs no `do` — just return the task. One `andThen` is fine
inline; the payoff starts at two chained steps. For *independent* values combined
at the end (not a dependent sequence), `Task.map2`..`map5` or `Task.parallel` say
that better than a `do` block.

## References

- [`Ipe.Task`](../modules/Ipe.Task.md) — the combinators `do` sequences.
- `ipe doc do` — the construct reference.
- [The Elm Architecture](../guide/the-elm-architecture.md) — where a task's result
  returns as a `Msg` in an interactive program.
