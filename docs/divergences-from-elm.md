# Divergences from Elm

The full **coverage matrix** (every `elm/*` module → per-value status) lives in
[`elm-coverage/README.md`](elm-coverage/README.md) and the exhaustive
per-value `elm/core` table in
[`elm-coverage/elm-core-coverage.md`](elm-coverage/elm-core-coverage.md).
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
