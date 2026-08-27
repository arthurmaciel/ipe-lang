# Divergences from Elm

The full **coverage matrix** (every `elm/*` module → per-value status) lives in
[`elm-coverage/README.md`](topics/elm-coverage/README.md) and the exhaustive
per-value `elm/core` table in
[`elm-coverage/elm-core-coverage.md`](topics/elm-coverage/elm-core-coverage.md).
This document is the durable ledger of the deliberate **departures** from Elm's
API shape — the places where Ipê provides the same capability under a different
type or omits an Elm module on purpose. Each entry states only *what differs*
and *why*.

## `Array` — intentionally absent

Elm's `Array` exists because Elm's `List` is a cons-cell linked list: indexing
and length are O(n), so a second sequence type with O(log n) indexed access is
worth carrying. Ipê's `List` lowers to a Rust `Vec`, which already provides O(1)
index and length and amortised O(1) push — exactly the capability `Array` is
there to add. A separate `Array` type would only re-create the "List vs Array,
which one do I reach for?" split without any asymptotic benefit, so it is
deliberately not provided. Sequence code uses `Ipe.List` throughout.

## `Ipe.Bitwise` — 64-bit width

elm/core `Bitwise` is specified on 32-bit integers (JavaScript coerces with
`| 0`). Ipê's `Int` is 64-bit two's-complement (it lowers to Rust `i64`), so
`complement` flips 64 bits and `shiftRightZfBy` zero-fills across the full
64-bit width rather than wrapping at 32. `and` / `or` / `xor` /
`shiftLeftBy` / `shiftRightBy` map 1:1 to the corresponding Rust `i64`
operators. Shift amounts are taken modulo the word width (`& 63`) so every draw
is total (a raw Rust shift by `>= 64` panics in debug).

## `Ipe.Random.Generator` — seed-explicit combinators

elm/random's `Generator` combinators are point-free: `map : (a -> b) ->
Generator a -> Generator b` returns a *deferred* `Generator` (a closure over the
seed). Ipê's combinators are instead **seed-explicit** — `map : (a -> b) ->
Generator a -> Seed -> ( b, Seed )` takes the current seed and returns the value
plus the next seed directly (a fused `map … |> step`). The reason is a Rust
backend limitation: the deferred form requires boxing closures that capture
polymorphic generator values as `Box<dyn Fn + Send + Sync>`, and a captured
`dyn Fn` does not satisfy those bounds (see diagnostic IPE-L0126). The
seed-explicit form threads the seed with the same reproducibility, lowers
cleanly, and keeps the elm/random names (`map` / `map2` / `map3` / `andThen` /
`listOf` / `pair`) and the seeded, deterministic contract.

The polymorphic-value combinators `constant`, `uniform`, and `weighted` are not
provided for the same backend reason: each would either return a closure that
forwards a polymorphic value out of its body, or reuse a generic `Vec<a>` after
a by-value runtime call — neither of which the current generic codegen can
lower. The base generators (`int`, `float`) and the combinators above cover the
composable seeded surface the seed primitives support.

## `Ipe.Url.Parser` — named combinators and pure-data patterns

elm/url's `Url.Parser` uses infix `</>` / `<?>` and threads a continuation
FUNCTION through the parser (`Parser a b`, whose state carries a
partially-applied route builder), with `map` / `oneOf` composing those
function-carrying parsers. Ipê diverges on two points, both to satisfy the Rust
backend's first-class-function limits while keeping identical *matching*
semantics:

- **Named combinators for the operators.** `</>` is `slash` and `<?>` is
  `withQuery`, because Ipê has a fixed operator set and no custom-operator
  declaration. `s` / `int` / `string` / `top` / `query` keep their elm/url
  names.
- **Pure-data patterns; the caller applies the route constructor.** A `Pattern`
  here holds only data — an ordered list of segment matchers plus query keys —
  so it composes, lists, and matches with no stored function value anywhere. It
  has to: the backend stores a function value only in a union payload, never in
  a record field (IPE-L0107), never forwarded through a closure capture
  (IPE-L0126), and — the decisive one for a router — never as a non-`Clone`
  element of a `List` walked by value (cons-destructuring a function-bearing
  list emits a `.clone()` the function type cannot satisfy). A `oneOf` that
  carried per-alternative *builder functions* in a list therefore cannot lower.
  Instead, `parse : Pattern -> Url -> Maybe Captures` yields the ordered
  captures on a total match, and the caller selects the alternative and applies
  its route constructor in ordinary code — a `case` chain over `parse` results,
  where a constructor is only ever CALLED, never stored. `parse` stays total
  (`Maybe Captures`, no match is `Nothing`, no silent wildcard), and the capture
  readers `firstString` / `firstInt` / `firstQuery` name the common single-
  capture reads.

The parser consumes the already-parsed `Url` through the shipped `Ipe.Url`
accessors (`path` / `query`) — it splits path segments and query pairs once,
over the typed value, and never re-parses a raw string.

## Row polymorphism — concrete monomorphisation, opt-in via annotation

Elm compiles one row-polymorphic function to a single body and reads record
fields by dynamic JavaScript property access. Ipê has no dynamic field lookup in
the emitted Rust, so a row-polymorphic annotated function
(`greet : { r | name : String } -> String`) lowers to one rustc-generic function
bounded by a synthesised per-field *witness trait*; rustc then emits one machine
copy per record shape the function is called with. The bare accessor `.name`
diverges similarly: each `.name` *occurrence* is typed and pinned independently,
so `List.map .name people` works, but reusing one unannotated binding at two
different record shapes stays a type error — polymorphic reuse of a single
binding requires the row annotation. Row polymorphism is therefore opt-in via an
annotation; unannotated bindings still pin on first concrete use, preserving the
pinned-records invariant. The one emittable use of a row-typed parameter is as
the direct receiver of a field read (`rec.name`); a row value that flows
anywhere else — re-bound, destructured by a subset pattern, passed as an
argument, stored, returned, or matched — has no witness-getter route and is
gated with `IPE-L0131`. An argument-position open row may carry one or more
required fields (each contributes one witness bound); return-position rows,
rows nested under a container/record/tuple, and a field whose type is itself an
open row remain gated with `IPE-L0131`. These are extensions of the same
witness-trait design, gated until each lands.
