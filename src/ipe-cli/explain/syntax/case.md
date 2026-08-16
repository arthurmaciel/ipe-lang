---
kind: syntax
title: "case: pattern matching"
summary: "Inspect a value and branch on its shape. Every constructor of a union must be covered."
aliases: ["case-of", "pattern-match", "match"]
see_also: ["let", "type", "or-pattern"]
---

# `case` — pattern matching

The code examples in this page are illustrative Ipê source snippets, not shell commands.

Imagine asking a question about a value: "Is this a `Just` or a `Nothing`? Is
this `Red`, `Green`, or `Blue`?" The `case` expression is how you ask that
question in Ipê. It looks at a value, matches it against a list of shapes
(called *patterns*), and runs the branch whose shape fits.

## Basic form

```ipe
case myValue of
    Pattern1 ->
        result1

    Pattern2 ->
        result2
```

Each `->` separates the pattern from the expression that runs when it matches.
Indentation is significant: every branch must align.

## Matching a union

Define a union first, then match on each constructor:

```ipe
type Colour
    = Red
    | Green
    | Blue

describeColour : Colour -> String
describeColour colour =
    case colour of
        Red ->
            "warm red"

        Green ->
            "fresh green"

        Blue ->
            "cool blue"
```

The compiler requires you to cover **every** constructor. Omit `Blue` and you
get IPE-T0010 (non-exhaustive case). This is a feature — the compiler catches
the gap for you so a future new constructor cannot be silently skipped.

## Matching with payload

A constructor can carry data. The pattern names the payload variables:

```ipe
type Shape
    = Circle Float
    | Rectangle Float Float

area : Shape -> Float
area shape =
    case shape of
        Circle radius ->
            3.14159 * radius * radius

        Rectangle width height ->
            width * height
```

## Matching a `Maybe`

`Maybe` is a built-in union with two constructors: `Just a` (holds a value) and
`Nothing` (holds nothing). It is the canonical way to model optional data.

```ipe
greet : Maybe String -> String
greet maybeName =
    case maybeName of
        Just name ->
            "Hello, " ++ name ++ "!"

        Nothing ->
            "Hello, stranger!"
```

## Wildcard `_`

Use `_` to match anything you do not need to name. For a *closed* union
(one defined with `type`), prefer naming every constructor — that way the
compiler warns you when you add a new constructor later (IPE-T0018). Reserve
`_` for open or infinite domains like `Int` or `String`.

```ipe ipe:error
-- Closed union: avoid wildcard — IPE-T0018 flags this
isRed : Colour -> Bool
isRed colour =
    case colour of
        Red ->
            True

        _ ->
            False
```

```ipe
-- Name every constructor instead
isRed : Colour -> Bool
isRed colour =
    case colour of
        Red ->
            True

        Green ->
            False

        Blue ->
            False
```

## Nested patterns

Patterns can nest inside other patterns:

```ipe
firstElement : List Int -> Maybe Int
firstElement xs =
    case xs of
        [] ->
            Nothing

        x :: _ ->
            Just x
```

## Or-patterns

Two patterns that yield the same result can share a branch with `|`:

```ipe
isWarm : Colour -> Bool
isWarm colour =
    case colour of
        Red | Green ->
            True

        Blue ->
            False
```

See [`or-pattern`](or-pattern) for variable-binding rules in or-patterns.

## Glossary

- **pattern** — a description of a value's shape. Constructors, variables,
  wildcards, and literals are all patterns.
- **constructor** — one named form of a union type (`Red`, `Just`, `Nothing`).
- **branch** — one `Pattern -> expression` arm of a `case`.
- **exhaustive** — covering every constructor. The compiler enforces this.
- **wildcard** — `_`, matches anything without binding a name.
