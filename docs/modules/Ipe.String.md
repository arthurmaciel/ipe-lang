# Ipe.String

A `String` is immutable UTF-8 text. Every function here returns a **new** string
rather than changing its argument — the same [immutability](../guide/pure-functions.md)
that governs every value in the language.

## The mental model

- **Nothing mutates.** `String.toUpper s` builds a new string; `s` is untouched.
  Text transformations chain as a `|>` pipeline, each step handing a fresh string
  to the next.
- **Parsing returns a `Maybe`, not a crash.** `toInt "12"` is `Just 12`; `toInt "x"`
  is `Nothing`. The un-parsable case is in the type, so you handle it before using
  the number. This is [parse, don't validate](../idioms/parse-dont-validate.md) at
  the string boundary: turn untyped text into a typed value once, at the edge.
- **The functions cluster into families.** Construction, shape, combining,
  splitting, slicing, case, searching, parsing, and character-wise transforms.
  Reach for the family that matches the step; most real work threads several
  together.

## A worked example

The [`word-frequency`](../../examples/shapes/script/word-frequency) program
(also the [`Ipe.List` guide's](Ipe.List.md) example) leans on `Ipe.String` at both
ends of its pipeline — normalising the input text, then formatting the output.

**Splitting text into words.** `String.toLower` normalises case, `String.words`
breaks on whitespace, and the empty tokens are filtered out:

```ipe
tokenize : String -> List String
tokenize text =
    text
        |> String.toLower
        |> String.words
        |> List.filter (\word -> not (String.isEmpty word))
```

**Formatting each row.** `String.padRight` aligns the word column and
`String.fromInt` renders the count as text:

```ipe
formatRow : ( String, Int ) -> String
formatRow ( word, count ) =
    String.padRight 8 ' ' word ++ String.fromInt count
```

**Joining the rows.** `String.join` stitches the formatted rows into one string
with a newline between each:

```ipe
        |> List.map formatRow
        |> String.join "\n"
```

Together these turn raw text into an aligned three-line report. Every intermediate
is a new string; no buffer is mutated in place.

## Why it is shaped this way

- **Immutable text is safe to share** ([Correctness](../../PRINCIPLES.md)). A
  string passed to two functions cannot be changed by one behind the other's back.
- **Parsers in the type push failure to the edge.** `toInt`/`toFloat`/`isEmail`
  hand back a `Maybe` (or `Bool`), so an ill-formed string is caught once, where it
  enters, rather than surfacing as a crash deep in the program.
- **Locale-aware case is explicit.** `toUpper` is the plain case fold; the `*In`
  and `casefold` variants exist for when locale matters, so the common case stays
  simple and the careful case is available by name.

## References

- `ipe doc Ipe.String` — the per-symbol reference (every function with a snippet).
- [Parse, don't validate](../idioms/parse-dont-validate.md) — the parsing idiom
  the `toInt` family serves.
- [Pure functions and immutability](../guide/pure-functions.md) — why a new
  string, never a mutation.
- Sibling references: [`Ipe.List`](Ipe.List.md) (`words` / `lines` produce lists),
  [`Ipe.Maybe`](Ipe.Maybe.md) (what the parsers return), `ipe doc Ipe.Char`
  (per-character predicates).
