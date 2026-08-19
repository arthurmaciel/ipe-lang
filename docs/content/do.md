# do

A syntactic shorthand for chaining `Task.andThen` calls. Each `<-` line binds
the result of a `Task` to a name; the final expression is the overall `Task`
result. The desugaring is mechanical: every `name <- task` becomes a
`Task.andThen (\name -> …)` wrapping the rest of the block.

## Syntax

    do
        <name> <- <task-expr>
        <name> <- <task-expr>
        <expr>

## Example

    greet : Task Error ()
    greet =
        do
            name <- Io.readLine
            Io.println ("Hello, " ++ name ++ "!")

## Desugaring

The `do` block above is equivalent to:

    greet : Task Error ()
    greet =
        Io.readLine
            |> Task.andThen (\name -> Io.println ("Hello, " ++ name ++ "!"))

## Notes

- Only `Task`-typed expressions may appear on the right-hand side of `<-`.
- The final line must be a `Task` expression (not a plain value).
- `do` is notation — it generates no runtime overhead beyond the `andThen` chain.

## See also

- `Ipe.Task` — the full `Task` combinator surface.
- `Ipe.Io` — standard-I/O tasks.
