# Ipe.List

A `List a` is an ordered sequence of values that all share the type `a`. Lists are
[immutable](../guide/pure-functions.md): every function here returns a **new** list
rather than changing its argument, so the same list can be shared freely without
one caller's transformation affecting another's.

## The mental model

- **Nothing mutates; everything is a new list.** `List.map f xs` does not touch
  `xs` — it builds a fresh list. A "loop that updates a running total" becomes a
  **fold**: `List.foldl` walks the list once, threading an accumulator you rebuild
  at each step. That accumulator is the immutable stand-in for a mutable variable.
- **Absence is a `Maybe`, not a crash.** `head`, `maximum`, `find` might have no
  answer, so they return `Ipe.Maybe`: `head [] == Nothing`. The type forces you to
  handle the empty case before touching a first element — there is no
  out-of-bounds.
- **The functions cluster into families.** Shape (`length`, `reverse`), taking
  apart (`take`, `drop`), building up (`cons`, `append`, `concat`), element-wise
  (`map`, `filter`, `filterMap`), searching (`member`, `find`, `any`, `all`),
  folding (`foldl`, `sum`, `maximum`), and ordering (`sortBy`, `sortWith`,
  `unique`). Most real work is a `|>` pipeline through several families.

## A worked example

[`examples/shapes/program/word-frequency`](../../examples/shapes/program/word-frequency)
turns a paragraph into its three most common words. The whole computation is one
`|>` pipeline, read left to right — a tour of the families in order:

```ipe
report : String -> String
report text =
    text
        |> tokenize
        |> tally
        |> ranked
        |> List.take 3
        |> List.map formatRow
        |> String.join "\n"
```

**Building the list, then filtering it.** `tokenize` splits the text into words
and drops the empties — `String.words` produces the list, `List.filter` keeps only
the non-empty ones:

```ipe
tokenize : String -> List String
tokenize text =
    text
        |> String.toLower
        |> String.words
        |> List.filter (\word -> not (String.isEmpty word))
```

**Folding a list into a tally.** `tally` counts occurrences with `List.foldl`,
threading a growing `Dict` as the accumulator — the immutable equivalent of a
mutable counter map:

```ipe
tally : List String -> List ( String, Int )
tally words =
    List.foldl bump Dict.empty words
        |> Dict.toList
```

**Ordering by a custom comparison.** `ranked` sorts by descending count, breaking
ties alphabetically, with `List.sortWith` — which takes a comparison returning
`Order` and sorts stably:

```ipe
ranked : List ( String, Int ) -> List ( String, Int )
ranked counts =
    List.sortWith byCountThenWord counts
```

Then `List.take 3` keeps the top three and `List.map formatRow` renders each. Run,
the program prints `the` (4), `dog` (2), `fox` (2) — the tie broken alphabetically
by the stable `sortWith`.

## Why it is shaped this way

- **Immutability makes sharing safe and reasoning local**
  ([Correctness](../../PRINCIPLES.md)). Because no function can mutate its
  argument, a list passed to two places cannot be changed by one behind the
  other's back — no defensive copies, no aliasing bugs.
- **`Maybe` on the partial functions removes a whole class of crashes.** There is
  no `head` that throws on `[]`; the empty case is in the type, so a well-typed
  program cannot fall over reading a first element.
- **`foldl` is the one general reducer.** `sum`, `maximum`, and the `tally` above
  are all folds; learning `foldl` gives you every list-to-value reduction.

## References

- `ipe doc Ipe.List` — the per-symbol reference (every function with a snippet).
- [Pure functions and immutability](../guide/pure-functions.md) — why a new list,
  never a mutation.
- [The pipe idiom](../idioms/pipe.md) — reading a `|>` pipeline like this one.
- Sibling references: [`Ipe.Maybe`](Ipe.Maybe.md) (what the partial functions
  return), `ipe doc Ipe.Dict` (the key-value tally), [`Ipe.String`](Ipe.String.md)
  (`words`, `join`).
