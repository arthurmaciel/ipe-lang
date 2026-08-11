# release-preflight

A **Program-shape** script (a plain `main` you drive with tasks — no TEA loop)
that shows `do` / `doParallel` notation:

- `=` binds a pure value, `<-` binds a task's result, and a bare line runs a
  task for its effect and discards the result.
- `doParallel` runs its aligned, same-typed tasks concurrently and collects
  their results as a `List`; nested in a `do` via `results <- doParallel …`.

`src/Main.ipe` carries the hand-written `Task.andThen` equivalent in a comment,
so you can see exactly what the block desugars to.

```sh
ipe run src/Main.ipe
```
