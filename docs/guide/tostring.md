# ToString

`Ipe.ToString` gathers the "render this value to a `String`" functions for the
primitive types under one predictable prefix, so you can write
`ToString.fromInt n` without remembering which module owns each renderer.

## The mental model

Two ideas.

- **One prefix for every primitive's String form.** `ToString.fromInt`,
  `ToString.fromFloat`, and `ToString.fromBool` are thin aliases to the canonical
  renderers in their home modules — the same functions, gathered under one name so
  the editor and `ipe doc` surface them together. When you have a value and want
  its text, you reach for `ToString.` and the completion lists the options.
- **Rendering is total.** Every `Int`, `Float`, and `Bool` has a String form, so
  these functions never fail — no `Maybe` to unwrap, no `Result` to handle. The
  direction that *can* fail is the other one, *parsing* a String back into a
  number, which lives in `Ipe.String` (`toInt` / `toFloat`) and returns a `Maybe`.

## A worked example: a summary row

The example under
[`examples/shapes/script/tostring-render`](../../examples/shapes/script/tostring-render/src/Main.ipe)
renders one row of mixed-type fields, each through the matching `ToString`.

```ipe
row : String -> Int -> Float -> Bool -> String
row label count ratio enabled =
    String.concat
        [ label
        , ": count="
        , ToString.fromInt count
        , " ratio="
        , ToString.fromFloat ratio
        , " enabled="
        , ToString.fromBool enabled
        ]
```

Running it (`ipe run`):

```
alpha: count=3 ratio=0.75 enabled=True
beta: count=128 ratio=1.5 enabled=False
```

## The why

Rendering a primitive to text is total by nature — a number is always some
sequence of digits — so `ToString` returns a bare `String`, not a `Maybe`. This is
the asymmetry [parse-don't-validate][parse] names: going *to* a String throws away
structure and cannot fail, while going *from* one recovers structure and can, so
only the parse direction carries a failure type. Keeping the two directions in
different shapes (a total `fromInt`, a fallible `String.toInt`) makes that
asymmetry visible in the types.

[parse]: ../idioms/parse-dont-validate.md

## References

- **Per-symbol reference:** `ipe doc Ipe.ToString` — `fromInt`, `fromFloat`,
  `fromBool`, each with its signature.
- **Sibling guides:** [Strings](string.md) — the home of the parse direction
  (`String.toInt` / `String.toFloat`) and richer text building. [Basics](basics.md)
  — the auto-imported `toString`, which renders many types generically.
  [Characters](char.md) — code points and classification.
- **Concepts:** [The parse-don't-validate idiom](../idioms/parse-dont-validate.md)
  — why rendering is total but parsing is fallible.
