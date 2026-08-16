---
kind: syntax
title: "do: sequential effect blocks"
summary: "Chain Tasks in reading order without explicit Task.andThen nesting."
aliases: ["do-notation", "do-block", "do-syntax"]
see_also: ["let", "effects", "main"]
---

# `do` — sequential effect blocks

The code examples in this page are illustrative Ipê source snippets, not shell commands.

A `do` block lets you write a sequence of `Task` steps in top-to-bottom
reading order, without manually nesting `Task.andThen` calls. Each step runs
after the previous one completes.

## Basic form

```ipe
do
    step1
    step2
    step3
```

Each line is a `Task`. The `do` block threads them with `Task.andThen`
automatically. The whole block is itself a `Task`.

## Binding a result

Use `<-` to bind the result of a `Task` to a name for use in later steps:

```ipe
main : Task Error ()
main =
    do
        line <- Io.readLine
        Io.println ("You typed: " ++ line)
```

`line` is the `String` produced by `Io.readLine`. It is in scope for the rest
of the `do` block.

## Intermediate let bindings

Use `let` inside a `do` block to bind a pure (non-Task) value:

```ipe
main : Task Error ()
main =
    do
        line <- Io.readLine
        let upper = String.toUpper line
        Io.println upper
```

## Discarding a result

Use `_` on the left of `<-` when you run a `Task` for its effect but do not
need its return value:

```ipe
main : Task Error ()
main =
    do
        _ <- Io.println "starting…"
        line <- Io.readLine
        Io.println ("Done: " ++ line)
```

## The equivalent without `do`

A `do` block is syntactic sugar. The three-step block above is exactly:

```ipe
main : Task Error ()
main =
    Io.println "starting…"
        |> Task.andThen (\_ -> Io.readLine)
        |> Task.andThen (\line -> Io.println ("Done: " ++ line))
```

Both forms are equivalent; pick whichever reads better for your use case.

## Glossary

- **Task** — a description of a side-effectful computation.
- **`<-`** — binds the result of a `Task` step to a name.
- **`Task.andThen`** — sequences two Tasks, passing the first's result to the second.
