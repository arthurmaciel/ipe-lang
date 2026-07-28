# Elm `elm/core` Coverage Checklist

This document is the authoritative inventory of the entire `elm/core`
package — every module, every exposed type, and every exposed value with
its type signature. It exists to support the roadmap item **"Elm core
coverage"**: auditing how completely the Ipê standard library covers the
Elm core surface.

`elm/core` is the always-available base package in every Elm project. It
provides the primitive types (`Int`, `Float`, `Bool`, `String`, `Char`,
`List`, `Maybe`, `Result`, `Order`), the core data structures (`Array`,
`Dict`, `Set`), tuples, the effect primitives (`Task`, `Process`,
`Platform` with `Cmd`/`Sub`), bitwise operations, and the development
helper `Debug`.

**How to read the tables.** Each module section lists its exposed types
first, then a table of every exposed value/function with its exact Elm
type signature. The `Ipê status` column records the audit verdict for each
row: `✓` present + reachable (module cited), `~` present but under a
different name/signature/module (divergence noted), `✗` absent, `n/a`
Elm-runtime-specific with no Ipê analogue. Signatures are quoted verbatim from the Elm package registry
(`package.elm-lang.org/packages/elm/core/latest/docs.json`); the
`Module.` qualifiers Elm emits in `docs.json` are elided for readability
(e.g. `Basics.Int` → `Int`).

**Source & verification.** Every module below was extracted from the live
`elm/core` `docs.json`. Modules covered: Array, Basics, Bitwise, Char,
Debug, Dict, List, Maybe, Platform, Platform.Cmd, Platform.Sub, Process,
Result, Set, String, Task, Tuple — 17 total, all verified against the
registry.

> **Audit basis.** The authoritative Ipê stdlib is the `ipe_stdlib` crate
> (`src/stdlib/`, the `Ipe/*.ipe` modules embedded via `src/stdlib/src/lib.rs`).
> A name counts as reachable only when it both
> resolves — via the embedded module's `exposing` list or the auto-prelude
> `QUALIFIERS` table in `src/compiler/canon/src/env.rs` — **and** carries a
> concrete type, either from a Ipê-source body/annotation or from a matched
> arm of `kernel_ty` in `src/compiler/types/src/constrain.rs`. `kernel_ty`'s
> catch-all returns an unconstrained type variable, so a name that only hits
> the fallback is treated as *not* usably typed. Notably, Ipê has **no**
> `Array`, `Bitwise`, `Tuple`, `Debug`, `Process`, or `Platform` module, and
> no `Order` or `Never` type; the numeric surface beyond the language
> operators lives in `Ipe.Math` (typed in `kernel_ty` and registered in
> `QUALIFIERS`, though not embedded as source); tuple helpers are
> `Basics.fst`/`snd`; `Cmd`/`Sub` are `Ipe.Cmd`/`Ipe.Sub`.

---

## Array

Fast immutable arrays with O(log n) indexed access.

**Types**

- `type Array a` — opaque. ✗ (no `Array` type/module)

| Function / Type | Signature | Ipê status |
|---|---|---|
| `empty` | `Array a` | ✗ |
| `initialize` | `Int -> (Int -> a) -> Array a` | ✗ |
| `repeat` | `Int -> a -> Array a` | ✗ |
| `fromList` | `List a -> Array a` | ✗ |
| `isEmpty` | `Array a -> Bool` | ✗ |
| `length` | `Array a -> Int` | ✗ |
| `get` | `Int -> Array a -> Maybe a` | ✗ |
| `set` | `Int -> a -> Array a -> Array a` | ✗ |
| `push` | `a -> Array a -> Array a` | ✗ |
| `append` | `Array a -> Array a -> Array a` | ✗ |
| `slice` | `Int -> Int -> Array a -> Array a` | ✗ |
| `toList` | `Array a -> List a` | ✗ |
| `toIndexedList` | `Array a -> List ( Int, a )` | ✗ |
| `map` | `(a -> b) -> Array a -> Array b` | ✗ |
| `indexedMap` | `(Int -> a -> b) -> Array a -> Array b` | ✗ |
| `filter` | `(a -> Bool) -> Array a -> Array a` | ✗ |
| `foldl` | `(a -> b -> b) -> b -> Array a -> b` | ✗ |
| `foldr` | `(a -> b -> b) -> b -> Array a -> b` | ✗ |

---

## Basics

Primitive numeric/boolean/comparison functions and operators, plus the
core types. Everything here is exposed by default in every Elm module.

**Types**

- `type Int` — whole numbers. ✓ (builtin)
- `type Float` — floating-point numbers. ✓ (builtin)
- `type Bool = True | False` ✓ (builtin; `True`/`False` used by `Basics.not`)
- `type Order = LT | EQ | GT` ✗ (no `Order` type; `LT`/`EQ`/`GT` absent)
- `type Never` — a value that can never happen (no constructors). ✗ (absent)

**Operators**

| Function / Type | Signature | Ipê status |
|---|---|---|
| `(+)` | `number -> number -> number` | ✓ (`add`, BinopClass::Arith) |
| `(-)` | `number -> number -> number` | ✓ (`sub`) |
| `(*)` | `number -> number -> number` | ✓ (`mul`) |
| `(/)` | `Float -> Float -> Float` | ✓ (`fdiv`) |
| `(//)` | `Int -> Int -> Int` | ✓ (`idiv`) |
| `(^)` | `number -> number -> number` | ✗ (no power operator; `Math.pow` is Float-only) |
| `(==)` | `a -> a -> Bool` | ✓ (`eq`) |
| `(/=)` | `a -> a -> Bool` | ✓ (`neq`) |
| `(<)` | `comparable -> comparable -> Bool` | ✓ (`lt`, BinopClass::Order) |
| `(>)` | `comparable -> comparable -> Bool` | ✓ (`gt`) |
| `(<=)` | `comparable -> comparable -> Bool` | ✓ (`le`) |
| `(>=)` | `comparable -> comparable -> Bool` | ✓ (`ge`) |
| `(&&)` | `Bool -> Bool -> Bool` | ✓ (`and`) |
| `(\|\|)` | `Bool -> Bool -> Bool` | ✓ (`or`) |
| `(++)` | `appendable -> appendable -> appendable` | ✓ (`append`) |
| `(\|>)` | `a -> (a -> b) -> b` | ✓ (`resolve.rs` pipe path) |
| `(<\|)` | `(a -> b) -> a -> b` | ✓ (`resolve.rs` pipe path) |
| `(>>)` | `(a -> b) -> (b -> c) -> a -> c` | ✗ (no function-composition operator) |
| `(<<)` | `(b -> c) -> (a -> b) -> a -> c` | ✗ (no function-composition operator) |

**Functions**

| Function / Type | Signature | Ipê status |
|---|---|---|
| `toFloat` | `Int -> Float` | ✗ (no `Int -> Float` widening kernel; `String.toFloat` is unrelated) |
| `round` | `Float -> Int` | ✓ (`Ipe.Math.round`, same sig) |
| `floor` | `Float -> Int` | ✓ (`Ipe.Math.floor`) |
| `ceiling` | `Float -> Int` | ~ (`Math.ceil` — name differs: `ceil` vs `ceiling`) |
| `truncate` | `Float -> Int` | ~ (`Math.trunc` — name differs: `trunc` vs `truncate`) |
| `max` | `comparable -> comparable -> comparable` | ✓ (`Math.max`, Ord-bounded `a -> a -> a`) |
| `min` | `comparable -> comparable -> comparable` | ✓ (`Math.min`, Ord-bounded) |
| `compare` | `comparable -> comparable -> Order` | ~ (registered `Basics` qualifier but `kernel_ty` has no arm → unconstrained-var fallback; no `Order` type / `LT`/`EQ`/`GT`) |
| `not` | `Bool -> Bool` | ✓ (`Ipe.Basics.not`, Ipê source) |
| `xor` | `Bool -> Bool -> Bool` | ✗ (no boolean `xor`) |
| `modBy` | `Int -> Int -> Int` | ~ (registered `Basics` qualifier but no typed kernel arm; `Math.mod` is `Float -> Float -> Float`) |
| `remainderBy` | `Int -> Int -> Int` | ✗ (`Math.remainder` is `Float -> Float -> Float`; no Int form) |
| `negate` | `number -> number` | ~ (registered `Basics` qualifier but no typed kernel arm; no `Math.negate`) |
| `abs` | `number -> number` | ~ (`Math.abs` is `Int -> Int` only — no Float/`number` form) |
| `clamp` | `number -> number -> number -> number` | ✓ (`Ipe.Basics.clamp`, Ipê source) |
| `sqrt` | `Float -> Float` | ✓ (`Math.sqrt`) |
| `logBase` | `Float -> Float -> Float` | ✗ (`Math` has `log`/`log2`/`log10`, no `logBase`) |
| `e` | `Float` | ✓ (`Math.e`) |
| `pi` | `Float` | ✓ (`Math.pi`) |
| `cos` | `Float -> Float` | ✓ (`Math.cos`) |
| `sin` | `Float -> Float` | ✓ (`Math.sin`) |
| `tan` | `Float -> Float` | ✓ (`Math.tan`) |
| `acos` | `Float -> Float` | ✓ (`Math.acos`) |
| `asin` | `Float -> Float` | ✓ (`Math.asin`) |
| `atan` | `Float -> Float` | ✓ (`Math.atan`) |
| `atan2` | `Float -> Float -> Float` | ✓ (`Math.atan2`) |
| `degrees` | `Float -> Float` | ✗ |
| `radians` | `Float -> Float` | ✗ |
| `turns` | `Float -> Float` | ✗ |
| `toPolar` | `( Float, Float ) -> ( Float, Float )` | ✗ |
| `fromPolar` | `( Float, Float ) -> ( Float, Float )` | ✗ |
| `isNaN` | `Float -> Bool` | ✗ (`Math.nan` is a constant, not a predicate) |
| `isInfinite` | `Float -> Bool` | ✗ (`Math.inf` is a constant, not a predicate) |
| `identity` | `a -> a` | ✓ (`Ipe.Basics.identity`, Ipê source) |
| `always` | `a -> b -> a` | ✓ (`Ipe.Basics.always`, Ipê source) |
| `never` | `Never -> a` | ✗ (no `Never` type) |

---

## Bitwise

Bitwise operations on `Int`.

| Function / Type | Signature | Ipê status |
|---|---|---|
| `and` | `Int -> Int -> Int` | ✗ |
| `or` | `Int -> Int -> Int` | ✗ |
| `xor` | `Int -> Int -> Int` | ✗ |
| `complement` | `Int -> Int` | ✗ |
| `shiftLeftBy` | `Int -> Int -> Int` | ✗ |
| `shiftRightBy` | `Int -> Int -> Int` | ✗ |
| `shiftRightZfBy` | `Int -> Int -> Int` | ✗ |

---

## Char

Functions over single Unicode characters.

**Types**

- `type Char` — a single Unicode character. ✓ (builtin; runtime rune / `int32`)

| Function / Type | Signature | Ipê status |
|---|---|---|
| `isUpper` | `Char -> Bool` | ✓ (`Ipe.Char.isUpper`) |
| `isLower` | `Char -> Bool` | ✓ (`Ipe.Char.isLower`) |
| `isAlpha` | `Char -> Bool` | ✓ (`Ipe.Char.isAlpha`) |
| `isAlphaNum` | `Char -> Bool` | ✗ |
| `isDigit` | `Char -> Bool` | ✓ (`Ipe.Char.isDigit`) |
| `isOctDigit` | `Char -> Bool` | ✗ |
| `isHexDigit` | `Char -> Bool` | ✗ |
| `toUpper` | `Char -> Char` | ~ (`Char.toUpper : Char -> String` — returns a single-rune String, not `Char`) |
| `toLower` | `Char -> Char` | ~ (`Char.toLower : Char -> String` — returns a single-rune String, not `Char`) |
| `toLocaleUpper` | `Char -> Char` | ✗ |
| `toLocaleLower` | `Char -> Char` | ✗ |
| `toCode` | `Char -> Int` | ✓ (`Ipe.Char.toCode`) |
| `fromCode` | `Int -> Char` | ✓ (`Ipe.Char.fromCode`) |

---

## Debug

Development-only helpers. Cannot be used in packages and must be removed
before `--optimize` builds.

| Function / Type | Signature | Ipê status |
|---|---|---|
| `toString` | `a -> String` | ~ (no `Debug` module; the `String.fromInt`/`fromFloat`/`fromChar` family + a registered-but-untyped `Basics.toString` qualifier are the nearest analogues) |
| `log` | `String -> a -> a` | ✗ (no `Debug.log`; `Ipe.Log` is a `Task`-tier logger, not the value-passthrough dev helper) |
| `todo` | `String -> a` | ✗ (no `Debug.todo`) |

---

## Dict

Immutable dictionary keyed by any `comparable`.

**Types**

- `type Dict k v` — opaque. ✓ (builtin)

| Function / Type | Signature | Ipê status |
|---|---|---|
| `empty` | `Dict k v` | ✓ (`Ipe.Dict.empty`) |
| `singleton` | `comparable -> v -> Dict comparable v` | ✗ |
| `insert` | `comparable -> v -> Dict comparable v -> Dict comparable v` | ✓ |
| `update` | `comparable -> (Maybe v -> Maybe v) -> Dict comparable v -> Dict comparable v` | ✗ |
| `remove` | `comparable -> Dict comparable v -> Dict comparable v` | ✓ |
| `isEmpty` | `Dict k v -> Bool` | ✓ |
| `member` | `comparable -> Dict comparable v -> Bool` | ✓ |
| `get` | `comparable -> Dict comparable v -> Maybe v` | ✓ |
| `size` | `Dict k v -> Int` | ✓ |
| `keys` | `Dict k v -> List k` | ✓ |
| `values` | `Dict k v -> List v` | ✓ |
| `toList` | `Dict k v -> List ( k, v )` | ✓ |
| `fromList` | `List ( comparable, v ) -> Dict comparable v` | ✓ |
| `map` | `(k -> a -> b) -> Dict k a -> Dict k b` | ✓ |
| `foldl` | `(k -> v -> b -> b) -> b -> Dict k v -> b` | ✓ |
| `foldr` | `(k -> v -> b -> b) -> b -> Dict k v -> b` | ✗ (only `foldl` is exposed) |
| `filter` | `(comparable -> v -> Bool) -> Dict comparable v -> Dict comparable v` | ✗ |
| `partition` | `(comparable -> v -> Bool) -> Dict comparable v -> ( Dict comparable v, Dict comparable v )` | ✗ |
| `union` | `Dict comparable v -> Dict comparable v -> Dict comparable v` | ✓ (left-biased) |
| `intersect` | `Dict comparable v -> Dict comparable v -> Dict comparable v` | ✗ |
| `diff` | `Dict comparable a -> Dict comparable b -> Dict comparable a` | ✗ |
| `merge` | `(comparable -> a -> result -> result) -> (comparable -> a -> b -> result -> result) -> (comparable -> b -> result -> result) -> Dict comparable a -> Dict comparable b -> result -> result` | ✗ |

---

## List

Operations on ordered, homogeneous linked lists.

| Function / Type | Signature | Ipê status |
|---|---|---|
| `(::)` | `a -> List a -> List a` | ✓ (cons operator + `List.cons`) |
| `singleton` | `a -> List a` | ✗ |
| `repeat` | `Int -> a -> List a` | ✗ |
| `range` | `Int -> Int -> List Int` | ✓ (`Ipe.List.range`) |
| `map` | `(a -> b) -> List a -> List b` | ✓ (Ipê source) |
| `indexedMap` | `(Int -> a -> b) -> List a -> List b` | ✓ (exposed by `Ipe.List`; not in auto-prelude — needs explicit import) |
| `foldl` | `(a -> b -> b) -> b -> List a -> b` | ✓ |
| `foldr` | `(a -> b -> b) -> b -> List a -> b` | ✓ |
| `filter` | `(a -> Bool) -> List a -> List a` | ✓ |
| `filterMap` | `(a -> Maybe b) -> List a -> List b` | ✗ |
| `length` | `List a -> Int` | ✓ |
| `reverse` | `List a -> List a` | ✓ |
| `member` | `a -> List a -> Bool` | ✓ |
| `all` | `(a -> Bool) -> List a -> Bool` | ✓ |
| `any` | `(a -> Bool) -> List a -> Bool` | ✓ |
| `maximum` | `List comparable -> Maybe comparable` | ✗ |
| `minimum` | `List comparable -> Maybe comparable` | ✗ |
| `sum` | `List number -> number` | ✗ |
| `product` | `List number -> number` | ✗ |
| `append` | `List a -> List a -> List a` | ✓ |
| `concat` | `List (List a) -> List a` | ✓ |
| `concatMap` | `(a -> List b) -> List a -> List b` | ✓ |
| `intersperse` | `a -> List a -> List a` | ✗ |
| `map2` | `(a -> b -> result) -> List a -> List b -> List result` | ✗ |
| `map3` | `(a -> b -> c -> result) -> List a -> List b -> List c -> List result` | ✗ |
| `map4` | `(a -> b -> c -> d -> result) -> List a -> List b -> List c -> List d -> List result` | ✗ |
| `map5` | `(a -> b -> c -> d -> e -> result) -> List a -> List b -> List c -> List d -> List e -> List result` | ✗ |
| `sort` | `List comparable -> List comparable` | ✗ |
| `sortBy` | `(a -> comparable) -> List a -> List a` | ✗ |
| `sortWith` | `(a -> a -> Order) -> List a -> List a` | ✗ |
| `isEmpty` | `List a -> Bool` | ✓ |
| `head` | `List a -> Maybe a` | ✓ |
| `tail` | `List a -> Maybe (List a)` | ✓ |
| `take` | `Int -> List a -> List a` | ✓ |
| `drop` | `Int -> List a -> List a` | ✓ |
| `partition` | `(a -> Bool) -> List a -> ( List a, List a )` | ✗ |
| `unzip` | `List ( a, b ) -> ( List a, List b )` | ✗ (inverse `zip` exists as an Ipê extra) |

---

## Maybe

Optional values.

**Types**

- `type Maybe a = Just a | Nothing` ✓ (builtin; `Just`/`Nothing` in Prelude)

| Function / Type | Signature | Ipê status |
|---|---|---|
| `withDefault` | `a -> Maybe a -> a` | ✓ (`Ipe.Maybe`, Ipê source) |
| `map` | `(a -> b) -> Maybe a -> Maybe b` | ✓ |
| `map2` | `(a -> b -> value) -> Maybe a -> Maybe b -> Maybe value` | ✓ |
| `map3` | `(a -> b -> c -> value) -> Maybe a -> Maybe b -> Maybe c -> Maybe value` | ✓ |
| `map4` | `(a -> b -> c -> d -> value) -> Maybe a -> Maybe b -> Maybe c -> Maybe d -> Maybe value` | ✓ |
| `map5` | `(a -> b -> c -> d -> e -> value) -> Maybe a -> Maybe b -> Maybe c -> Maybe d -> Maybe e -> Maybe value` | ✓ |
| `andThen` | `(a -> Maybe b) -> Maybe a -> Maybe b` | ✓ |

---

## Platform

Program construction and effect-manager plumbing.

**Types**

- `type Program flags model msg` — an Elm program. n/a (no `Program` type)
- `type Task err ok` — asynchronous operation that may fail (the low-level
  primitive re-exposed by `Task`). ✓ (builtin `Task`, but with a fixed `Error`
  channel — see the Task section)
- `type ProcessId` — a lightweight process (re-exposed by `Process` as `Id`). ✗
- `type Router appMsg selfMsg` — routes messages inside an effect manager. n/a

| Function / Type | Signature | Ipê status |
|---|---|---|
| `worker` | `{ init : flags -> ( model, Cmd msg ), update : msg -> model -> ( model, Cmd msg ), subscriptions : model -> Sub msg } -> Program flags model msg` | n/a (Ipê program entry is `Web.app` / `Tui.app` / `WebView.app`, not `Platform.worker`) |
| `sendToApp` | `Router msg a -> msg -> Task x ()` | n/a (effect-manager plumbing; Ipê has no user-defined effect managers / `Router`) |
| `sendToSelf` | `Router a msg -> msg -> Task x ()` | n/a (effect-manager plumbing) |

---

## Platform.Cmd

Commands — effects the runtime performs.

**Types**

- `type Cmd msg` — a batch of effects (re-exposed as `Cmd` in `Platform`). ✓ (builtin)

| Function / Type | Signature | Ipê status |
|---|---|---|
| `none` | `Cmd msg` | ✓ (`Ipe.Cmd.none`) |
| `batch` | `List (Cmd msg) -> Cmd msg` | ✓ (`Ipe.Cmd.batch`) |
| `map` | `(a -> msg) -> Cmd a -> Cmd msg` | ✓ (`Ipe.Cmd.map`) |

---

## Platform.Sub

Subscriptions — external events the runtime listens for.

**Types**

- `type Sub msg` — a batch of subscriptions. ✓ (builtin)

| Function / Type | Signature | Ipê status |
|---|---|---|
| `none` | `Sub msg` | ✓ (`Ipe.Sub.none`) |
| `batch` | `List (Sub msg) -> Sub msg` | ✓ (`Ipe.Sub.batch`) |
| `map` | `(a -> msg) -> Sub a -> Sub msg` | ✓ (`Ipe.Sub.map`) |

---

## Process

Lightweight green-thread primitives.

**Types**

- `type alias Id = Platform.ProcessId` ✗ (no `Process` module / `Id` type)

| Function / Type | Signature | Ipê status |
|---|---|---|
| `spawn` | `Task x a -> Task y Id` | ✗ (no `Process.spawn`; concurrency is `Task.parallel` / `Cmd`) |
| `sleep` | `Float -> Task x ()` | ~ (`Ipe.Time.sleep : Float -> Task Error ()` — same semantics, different module) |
| `kill` | `Id -> Task x ()` | ✗ (no `Process.kill`) |

---

## Result

Computations that may fail with a typed error.

**Types**

- `type Result error value = Ok value | Err error` ✓ (builtin; `Ok`/`Err` in Prelude)

| Function / Type | Signature | Ipê status |
|---|---|---|
| `withDefault` | `a -> Result x a -> a` | ✓ (`Ipe.Result`, Ipê source) |
| `map` | `(a -> value) -> Result x a -> Result x value` | ✓ |
| `map2` | `(a -> b -> value) -> Result x a -> Result x b -> Result x value` | ✓ |
| `map3` | `(a -> b -> c -> value) -> Result x a -> Result x b -> Result x c -> Result x value` | ✓ |
| `map4` | `(a -> b -> c -> d -> value) -> Result x a -> Result x b -> Result x c -> Result x d -> Result x value` | ✓ |
| `map5` | `(a -> b -> c -> d -> e -> value) -> Result x a -> Result x b -> Result x c -> Result x d -> Result x e -> Result x value` | ✓ |
| `andThen` | `(a -> Result x b) -> Result x a -> Result x b` | ✓ |
| `mapError` | `(x -> y) -> Result x a -> Result y a` | ✓ |
| `toMaybe` | `Result x a -> Maybe a` | ✗ |
| `fromMaybe` | `x -> Maybe a -> Result x a` | ✗ |

---

## Set

Immutable set of unique `comparable` values.

**Types**

- `type Set t` — opaque. ✓ (builtin)

| Function / Type | Signature | Ipê status |
|---|---|---|
| `empty` | `Set a` | ✓ (`Ipe.Set.empty`) |
| `singleton` | `comparable -> Set comparable` | ✗ |
| `insert` | `comparable -> Set comparable -> Set comparable` | ✓ |
| `remove` | `comparable -> Set comparable -> Set comparable` | ✓ |
| `isEmpty` | `Set a -> Bool` | ✗ (not exposed; only `size` is) |
| `member` | `comparable -> Set comparable -> Bool` | ✓ |
| `size` | `Set a -> Int` | ✓ |
| `toList` | `Set a -> List a` | ✓ |
| `fromList` | `List comparable -> Set comparable` | ✓ |
| `map` | `(comparable -> comparable2) -> Set comparable -> Set comparable2` | ✗ |
| `foldl` | `(a -> b -> b) -> b -> Set a -> b` | ✗ |
| `foldr` | `(a -> b -> b) -> b -> Set a -> b` | ✗ |
| `filter` | `(comparable -> Bool) -> Set comparable -> Set comparable` | ✗ |
| `partition` | `(comparable -> Bool) -> Set comparable -> ( Set comparable, Set comparable )` | ✗ |
| `union` | `Set comparable -> Set comparable -> Set comparable` | ✓ |
| `intersect` | `Set comparable -> Set comparable -> Set comparable` | ✓ |
| `diff` | `Set comparable -> Set comparable -> Set comparable` | ✓ |

---

## String

Operations on UTF-16 text.

**Types**

- `type String` — a chunk of text. ✓ (builtin; runtime UTF-8)

| Function / Type | Signature | Ipê status |
|---|---|---|
| `isEmpty` | `String -> Bool` | ✓ (`Ipe.String`) |
| `length` | `String -> Int` | ✓ |
| `reverse` | `String -> String` | ✓ |
| `repeat` | `Int -> String -> String` | ✓ |
| `replace` | `String -> String -> String -> String` | ✓ |
| `append` | `String -> String -> String` | ✓ |
| `concat` | `List String -> String` | ✓ |
| `split` | `String -> String -> List String` | ✓ |
| `join` | `String -> List String -> String` | ✓ |
| `words` | `String -> List String` | ✓ |
| `lines` | `String -> List String` | ✓ |
| `slice` | `Int -> Int -> String -> String` | ✓ |
| `left` | `Int -> String -> String` | ✗ (only `dropLeft`/`dropRight` are provided) |
| `right` | `Int -> String -> String` | ✗ |
| `dropLeft` | `Int -> String -> String` | ✓ |
| `dropRight` | `Int -> String -> String` | ✓ |
| `contains` | `String -> String -> Bool` | ✓ (plus haystack-first `containsIn`) |
| `startsWith` | `String -> String -> Bool` | ✓ (plus `startsWithIn`) |
| `endsWith` | `String -> String -> Bool` | ✓ (plus `endsWithIn`) |
| `indexes` | `String -> String -> List Int` | ✗ |
| `indices` | `String -> String -> List Int` | ✗ |
| `toInt` | `String -> Maybe Int` | ✓ |
| `fromInt` | `Int -> String` | ✓ |
| `toFloat` | `String -> Maybe Float` | ✓ |
| `fromFloat` | `Float -> String` | ✓ |
| `fromChar` | `Char -> String` | ✓ |
| `cons` | `Char -> String -> String` | ✗ (no `String.cons`) |
| `uncons` | `String -> Maybe ( Char, String )` | ✗ |
| `toList` | `String -> List Char` | ✓ |
| `fromList` | `List Char -> String` | ✓ |
| `toUpper` | `String -> String` | ✓ |
| `toLower` | `String -> String` | ✓ (plus `casefold` / `equalFold`) |
| `pad` | `Int -> Char -> String -> String` | ✗ (only `padLeft`/`padRight`) |
| `padLeft` | `Int -> Char -> String -> String` | ✓ |
| `padRight` | `Int -> Char -> String -> String` | ✓ |
| `trim` | `String -> String` | ✓ |
| `trimLeft` | `String -> String` | ~ (`String.trimStart` — name differs) |
| `trimRight` | `String -> String` | ~ (`String.trimEnd` — name differs) |
| `map` | `(Char -> Char) -> String -> String` | ✗ (no `String.map`) |
| `filter` | `(Char -> Bool) -> String -> String` | ✗ |
| `foldl` | `(Char -> b -> b) -> b -> String -> b` | ✗ |
| `foldr` | `(Char -> b -> b) -> b -> String -> b` | ✗ |
| `any` | `(Char -> Bool) -> String -> Bool` | ✗ |
| `all` | `(Char -> Bool) -> String -> Bool` | ✗ |

---

## Task

Asynchronous operations that may fail.

**Types**

- `type alias Task x a = Platform.Task x a` ~ (Ipê `Task` fixes the error
  channel to `Error` — every combinator is `Task Error a`, not a polymorphic
  `Task x a`)

| Function / Type | Signature | Ipê status |
|---|---|---|
| `succeed` | `a -> Task x a` | ✓ (`Ipe.Task.succeed : a -> Task Error a`) |
| `fail` | `x -> Task x a` | ✓ (`fail : Error -> Task Error a`) |
| `map` | `(a -> b) -> Task x a -> Task x b` | ✓ |
| `map2` | `(a -> b -> result) -> Task x a -> Task x b -> Task x result` | ✓ |
| `map3` | `(a -> b -> c -> result) -> Task x a -> Task x b -> Task x c -> Task x result` | ✓ |
| `map4` | `(a -> b -> c -> d -> result) -> Task x a -> Task x b -> Task x c -> Task x d -> Task x result` | ✓ |
| `map5` | `(a -> b -> c -> d -> e -> result) -> Task x a -> Task x b -> Task x c -> Task x d -> Task x e -> Task x result` | ✓ |
| `andThen` | `(a -> Task x b) -> Task x a -> Task x b` | ✓ |
| `sequence` | `List (Task x a) -> Task x (List a)` | ✓ (plus `parallel`) |
| `onError` | `(x -> Task y a) -> Task x a -> Task y a` | ✓ (fixed `Error` channel) |
| `mapError` | `(x -> y) -> Task x a -> Task y a` | ✓ (fixed `Error` channel) |
| `perform` | `(a -> msg) -> Task Never a -> Cmd msg` | ~ (exists as `Cmd.perform`, different module + signature) |
| `attempt` | `(Result x a -> msg) -> Task x a -> Cmd msg` | ✓ (`Ipe.Task.attempt`; `x` fixed to `Error`) |

---

## Tuple

Helpers for pairs.

| Function / Type | Signature | Ipê status |
|---|---|---|
| `pair` | `a -> b -> ( a, b )` | ✗ (tuple literals only; no `Tuple` module) |
| `first` | `( a, b ) -> a` | ~ (`Ipe.Basics.fst` — name differs) |
| `second` | `( a, b ) -> b` | ~ (`Ipe.Basics.snd` — name differs) |
| `mapFirst` | `(a -> x) -> ( a, b ) -> ( x, b )` | ✗ |
| `mapSecond` | `(b -> y) -> ( a, b ) -> ( a, y )` | ✗ |
| `mapBoth` | `(a -> x) -> (b -> y) -> ( a, b ) -> ( x, y )` | ✗ |

---

## Summary

Counts are of exposed values/functions (operators counted as values) and
exposed types (union types + type aliases) per module. Coverage is stated as
`✓ present` / `~ divergent` / `✗ absent` (plus `n/a` where noted); `~` is
counted separately, not folded into "present".

| Module | # values/funcs | # types | Coverage (values/funcs) | Types |
|---|---|---|---|---|
| Array | 18 | 1 | 0 ✓ · 0 ~ · 18 ✗ | 0/1 |
| Basics | 54 (35 funcs + 19 operators) | 5 | 34 ✓ · 6 ~ · 15 ✗ | 3/5 (Int, Float, Bool; no Order, Never) |
| Bitwise | 7 | 0 | 0 ✓ · 0 ~ · 7 ✗ | — |
| Char | 13 | 1 | 6 ✓ · 2 ~ · 5 ✗ | 1/1 |
| Debug | 3 | 0 | 0 ✓ · 1 ~ · 2 ✗ | — |
| Dict | 22 | 1 | 14 ✓ · 0 ~ · 8 ✗ | 1/1 |
| List | 38 (37 funcs + `(::)`) | 0 | 20 ✓ · 0 ~ · 17 ✗ | — |
| Maybe | 7 | 1 | 7 ✓ · 0 ~ · 0 ✗ | 1/1 |
| Platform | 3 | 4 | 0 ✓ · 0 ~ · 0 ✗ · 3 n/a | 1/4 (Task only) |
| Platform.Cmd | 3 | 1 | 2 ✓ · 0 ~ · 1 ✗ | 1/1 |
| Platform.Sub | 3 | 1 | 2 ✓ · 0 ~ · 1 ✗ | 1/1 |
| Process | 3 | 1 (`Id` alias) | 0 ✓ · 1 ~ · 2 ✗ | 0/1 |
| Result | 10 | 1 | 8 ✓ · 0 ~ · 2 ✗ | 1/1 |
| Set | 17 | 1 | 10 ✓ · 0 ~ · 7 ✗ | 1/1 |
| String | 44 | 1 | 29 ✓ · 2 ~ · 13 ✗ | 1/1 |
| Task | 13 | 1 (`Task` alias) | 7 ✓ · 1 ~ · 5 ✗ | 1/1 (fixed `Error` channel — divergent) |
| Tuple | 6 | 0 | 0 ✓ · 2 ~ · 4 ✗ | — |
| **Total** | **264** | **20** | **139 ✓ · 15 ~ · 107 ✗ · 3 n/a** | **13/20** |

(Operators break out as 16 ✓ · 3 ✗ within Basics; the 34 ✓ / 6 ~ / 15 ✗ for
Basics is functions + operators combined.)

---

## Audit findings

The concrete gap list, grouped for roadmap item C.4.

### (a) Whole modules absent

Four elm/core modules have **no** Ipê counterpart at all:

- **`Array`** (18 funcs, 1 type) — no immutable array type. `List` is the only
  sequence. This is the single largest missing surface.
- **`Bitwise`** (7 funcs) — no integer bit operations (`and`/`or`/`xor`/
  `complement`/`shiftLeftBy`/`shiftRightBy`/`shiftRightZfBy`).
- **`Tuple`** (6 funcs) — no `Tuple` module. `first`/`second` exist only as
  `Basics.fst`/`snd`; `pair`/`mapFirst`/`mapSecond`/`mapBoth` are absent.
- **`Debug`** (3 funcs) — no dev-helper module. `Debug.log`/`Debug.todo` have no
  analogue; the `toString` role is split across `String.fromInt`/`fromFloat`/…

Additionally, **`Process`** and **`Platform`** are effectively absent as
user-facing surfaces: `Platform.worker`/`sendToApp`/`sendToSelf` are `n/a` (Ipê
programs are `Web.app`/`Tui.app`/`WebView.app`; there are no user effect
managers or `Router`), and `Process.spawn`/`kill` plus the `ProcessId`/`Id` type
are missing (only `Time.sleep` covers `Process.sleep`).

### (b) Individual functions missing from otherwise-present modules

- **List** — `singleton`, `repeat`, `filterMap`, `maximum`, `minimum`, `sum`,
  `product`, `intersperse`, `map2`–`map5`, `sort`, `sortBy`, `sortWith`,
  `partition`, `unzip`. The `sort*` family and the numeric folds
  (`sum`/`product`/`maximum`/`minimum`) are the most commonly reached-for gaps.
- **Dict** — `singleton`, `update`, `foldr`, `filter`, `partition`,
  `intersect`, `diff`, `merge`. (`update` and `merge` are the notable
  ergonomics gaps.)
- **Set** — `singleton`, `isEmpty`, `map`, `foldl`, `foldr`, `filter`,
  `partition`. (Set has no traversal/higher-order surface at all.)
- **String** — `left`, `right`, `indexes`/`indices`, `cons`, `uncons`, `pad`,
  and the whole `Char`-folding family `map`/`filter`/`foldl`/`foldr`/`any`/
  `all`.
- **Result** — `toMaybe`, `fromMaybe` (the `Result` ⇄ `Maybe` bridges).
- **Task** — `map2`–`map5`, `attempt`.
- **Char** — `isAlphaNum`, `isOctDigit`, `isHexDigit`, `toLocaleUpper`,
  `toLocaleLower`.
- **Basics** — `toFloat` (`Int -> Float` widening), `xor`, `remainderBy`,
  `logBase`, `degrees`, `radians`, `turns`, `toPolar`, `fromPolar`, `isNaN`,
  `isInfinite`, `never`; operators `(^)`, `(>>)`, `(<<)`.
- **Platform.Cmd / Platform.Sub** — `Cmd.map` and `Sub.map` (functor mapping
  over commands/subscriptions).

### (c) Signature / behaviour divergences worth a decision

- **`Basics.compare` / the `Order` type.** `Basics.compare`
  (`src/runtime/rust/src/basics.rs`) and `List.sortWith`
  (`src/runtime/rust/src/list.rs`) are implemented as kernels. Whether Ipê exposes
  a first-class `Order` (`LT`/`EQ`/`GT`) ADT to user code, versus keeping compare
  as an opaque three-way kernel result, is the open surface decision.
- **`Basics.modBy` / `negate`.** Both are registered `Basics` qualifiers with no
  `kernel_ty` arm, so they type as an unconstrained variable rather than
  `Int -> Int -> Int` / `number -> number`. The concrete integer-modulo surface
  is `Math.mod`, which is `Float -> Float -> Float`. Decide whether the numeric
  `Basics` names should route to real `Math`/kernel types or be dropped in favour
  of `Math.*`.
- **`Basics.abs`.** `Math.abs` is `Int -> Int` only; Elm's `abs : number ->
  number` also covers `Float`. No Float `abs`.
- **Math namespace vs Elm `Basics`.** `round`/`floor`/`sqrt`/`e`/`pi`/trig live
  under `Ipe.Math`, not auto-exposed `Basics`; `ceiling`→`ceil` and
  `truncate`→`trunc` also rename. Decide whether to re-export the Math numerics
  through the default prelude to match Elm's zero-import ergonomics.
- **`Char.toUpper`/`toLower` return `String`.** Ipê types these as
  `Char -> String` (single-rune String) rather than Elm's `Char -> Char`.
- **`String.trimLeft`/`trimRight`** are named `trimStart`/`trimEnd`.
- **`Task` fixes the error channel.** Every `Task` combinator is `Task Error a`,
  not polymorphic `Task x a`; `Task.perform` is relocated to `Cmd.perform` with a
  different signature, and `Task.attempt` is absent.
- **No function-composition operators.** `(>>)` / `(<<)` are unavailable; only
  `(|>)` / `(<|)` pipes exist. `(^)` (power) is likewise absent — `Math.pow`
  covers it for `Float` only.

## Prioritized implementation backlog

Grouped into build-lane batches ordered by user impact; each is independently
implementable. Regenerate by comparing the kernel registry
(`src/compiler/kernels/src/lib.rs`) and canon qualifiers
(`src/compiler/canon/src/env.rs`) against `elm/core`'s `docs.json`; record
sanctioned gaps in `divergences-from-elm.md`.

- **List numerics (very high, pure):** `maximum`, `minimum`, `sum`, `product`,
  `singleton`, `repeat` — foldl/range based.
- **List sort/parallel (high):** `sort` (needs `comparable`), `map2`, `map3`–`5`,
  `intersperse`, `partition`, `unzip`.
- **Dict traversal (high):** `singleton`, `update`, `filter`, `foldr`,
  `partition`; set-ops `intersect`, `diff`, `merge`.
- **Set HOFs (medium):** `isEmpty`, `singleton`, `foldl` (unlocks the rest),
  `map`, `filter`, `partition` — Set has no HOF surface today.
- **String char-level (medium):** `map`, `filter`, `any`, `all`, `foldl`,
  `foldr`; navigation `left`, `right`, `cons`, `uncons`, `pad`, `indexes`.
- **Result/Maybe bridges (low count, high usability):** `Result.fromMaybe`
  (`toMaybe` already present).
- **Basics numerics (medium):** `toFloat`, `remainderBy`, `xor`, `isInfinite`,
  `logBase`; geometry `degrees`/`radians`/`turns`/`toPolar`/`fromPolar`; `(^)`,
  `(>>)`/`(<<)`.
- **Char completeness (low-medium):** `isAlphaNum`, `isHexDigit`, `isOctDigit`.
- **Task (medium):** `map2`–`map5`, `attempt`.
- **Tuple (low):** `mapFirst`, `mapSecond`, `mapBoth`.
- **Cmd.map / Sub.map (low-medium):** enables composing sub-components with
  different `Msg` types in the TEA update loop.

`Array` and `Bitwise` have no planned pure implementation (use `List` / FFI); see
`additive-stdlib-features.md` for the opaque-type designs if that changes.
