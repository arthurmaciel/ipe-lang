# Rendering values to String

Every primitive has a total "render this value to a `String`" path: the typed
`String.fromInt` / `String.fromFloat` for their own type, the generic
`toString` for any `Stringify` value, and `{{expr}}` interpolation inside a
`"""…"""` string for stitching several rendered values into one line.

## The mental model

Two ideas.

- **A typed renderer per type, plus a generic fallback.** `String.fromInt` and
  `String.fromFloat` are the precise renderers for their own type. `toString`
  (auto-imported from `Ipe.Basics`) works over any `Stringify` value —
  `Int`, `Float`, `Bool`, `String` included — so it is the one to reach for
  when a value's exact type does not matter, or when it varies.
- **Rendering is total.** Every `Int`, `Float`, and `Bool` has a String form,
  so these functions never fail — no `Maybe` to unwrap, no `Result` to
  handle. The direction that *can* fail is the other one, *parsing* a String
  back into a number, which lives in `Ipe.String` (`toInt` / `toFloat`) and
  returns a `Maybe`.

## A worked example: a summary row

The example under
[`examples/shapes/script/tostring-render`](../../examples/shapes/script/tostring-render/src/Main.ipe)
renders one row of mixed-type fields by interpolating each straight into a
triple-quoted string:

```ipe
row : String -> Int -> Float -> Bool -> String
row label count ratio enabled =
    """{{label}}: count={{count}} ratio={{ratio}} enabled={{enabled}}"""
```

Running it (`ipe run`):

```
alpha: count=3 ratio=0.75 enabled=true
beta: count=128 ratio=1.5 enabled=false
```

Each `{{expr}}` body is auto-stringified through `toString`, so `count` (an
`Int`) and `enabled` (a `Bool`) need no explicit `String.fromInt` or manual
conversion — only `label`, already a `String`, passes through unchanged.

## The why

Rendering a primitive to text is total by nature — a number is always some
sequence of digits — so both `toString` and `String.fromInt`/`fromFloat`
return a bare `String`, not a `Maybe`. This is the asymmetry
[parse-don't-validate][parse] names: going *to* a String throws away structure
and cannot fail, while going *from* one recovers structure and can, so only
the parse direction carries a failure type. Keeping the two directions in
different shapes (a total renderer, a fallible `String.toInt`) makes that
asymmetry visible in the types.

[parse]: ../idioms/parse-dont-validate.md

## References

- **Per-symbol reference:** `ipe doc Basics.toString`, `ipe doc String.fromInt`,
  `ipe doc String.fromFloat`.
- **Sibling guides:** [Strings](string.md) — the home of the parse direction
  (`String.toInt` / `String.toFloat`), richer text building, and the full
  `{{expr}}` interpolation syntax. [Basics](basics.md) — the auto-imported
  `toString`. [Characters](char.md) — code points and classification.
- **Concepts:** [The parse-don't-validate idiom](../idioms/parse-dont-validate.md)
  — why rendering is total but parsing is fallible.
  [String interpolation](../constructs/string-interpolation.md) — the full
  `{{expr}}` grammar.
