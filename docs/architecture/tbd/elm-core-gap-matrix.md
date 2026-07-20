# Elm `elm/core` Gap Matrix

**Basis:** `elm/core` 1.0.5 — `package.elm-lang.org/packages/elm/core/1.0.5/docs.json`  
**Companion:** `elm-core-coverage.md` — full narrative, audit findings, and design rationale  

---

## How to read this document

Each module table has four columns:

| Column | Values |
|---|---|
| **elm member** | Elm-qualified name (e.g. `List.maximum`) |
| **elm type sig** | Verbatim from the Elm docs (Elm module qualifiers elided for brevity) |
| **status** | One of four labels — see below |
| **notes** | ipê location, name, or divergence reference |

**Status labels:**

| Label | Meaning |
|---|---|
| `same-name` | Present and reachable under the exact same `Qualifier.name` |
| `renamed(X)` | Present as `X` — different qualifier and/or function name |
| `MISSING` | Not implemented; should be added for parity |
| `intentional(REF)` | Absent by design; `REF` cites `divergences-from-elm.md` §section |
| `n/a` | Elm-runtime-specific; no meaningful ipê analogue |

**Scope note.** `Platform.worker`/`sendToApp`/`sendToSelf` and `Process` are n/a or
intentionally-absent because ipê programs use `Live.app`/`Tui.app`/`Cli`/`Webview.app`
and `Task.parallel` replaces `Process.spawn`. `Debug` is n/a in production contexts —
replaced by `Ipe.Log`. `Array` and `Bitwise` are absent by design (divergences §4.4 R5).

---

## Array

**Status: intentionally absent (R5).** No `Array` type or module.  
`List` is the sole sequence; O(log n) random-access arrays are unplanned for the
current release cycle. All 18 functions are blocked by the missing type.

| elm member | elm type sig | status | notes |
|---|---|---|---|
| `Array.empty` | `Array a` | `intentional(R5)` | no Array type |
| `Array.initialize` | `Int -> (Int -> a) -> Array a` | `intentional(R5)` | |
| `Array.repeat` | `Int -> a -> Array a` | `intentional(R5)` | |
| `Array.fromList` | `List a -> Array a` | `intentional(R5)` | |
| `Array.isEmpty` | `Array a -> Bool` | `intentional(R5)` | |
| `Array.length` | `Array a -> Int` | `intentional(R5)` | |
| `Array.get` | `Int -> Array a -> Maybe a` | `intentional(R5)` | |
| `Array.set` | `Int -> a -> Array a -> Array a` | `intentional(R5)` | |
| `Array.push` | `a -> Array a -> Array a` | `intentional(R5)` | |
| `Array.append` | `Array a -> Array a -> Array a` | `intentional(R5)` | |
| `Array.slice` | `Int -> Int -> Array a -> Array a` | `intentional(R5)` | |
| `Array.toList` | `Array a -> List a` | `intentional(R5)` | |
| `Array.toIndexedList` | `Array a -> List (Int, a)` | `intentional(R5)` | |
| `Array.map` | `(a -> b) -> Array a -> Array b` | `intentional(R5)` | |
| `Array.indexedMap` | `(Int -> a -> b) -> Array a -> Array b` | `intentional(R5)` | |
| `Array.filter` | `(a -> Bool) -> Array a -> Array a` | `intentional(R5)` | |
| `Array.foldl` | `(a -> b -> b) -> b -> Array a -> b` | `intentional(R5)` | |
| `Array.foldr` | `(a -> b -> b) -> b -> Array a -> b` | `intentional(R5)` | |

**Summary:** 0 same-name · 0 renamed · 0 MISSING · 18 intentional

---

## Basics

**Types:** `Int` ✓ · `Float` ✓ · `Bool`/`True`/`False` ✓ · `Order(LT/EQ/GT)` intentional(R5) · `Never` intentional(R5)

### Basics — operators

| elm member | elm type sig | status | notes |
|---|---|---|---|
| `(+)` | `number -> number -> number` | `same-name` | BinopClass::Arith |
| `(-)` | `number -> number -> number` | `same-name` | |
| `(*)` | `number -> number -> number` | `same-name` | |
| `(/)` | `Float -> Float -> Float` | `same-name` | float div |
| `(//)` | `Int -> Int -> Int` | `same-name` | integer div |
| `(^)` | `number -> number -> number` | `MISSING` | `Math.pow` is Float-only |
| `(==)` | `a -> a -> Bool` | `same-name` | |
| `(/=)` | `a -> a -> Bool` | `same-name` | |
| `(<)` | `comparable -> comparable -> Bool` | `same-name` | BinopClass::Order |
| `(>)` | `comparable -> comparable -> Bool` | `same-name` | |
| `(<=)` | `comparable -> comparable -> Bool` | `same-name` | |
| `(>=)` | `comparable -> comparable -> Bool` | `same-name` | |
| `(&&)` | `Bool -> Bool -> Bool` | `same-name` | |
| `(\|\|)` | `Bool -> Bool -> Bool` | `same-name` | |
| `(++)` | `appendable -> appendable -> appendable` | `same-name` | string/list append |
| `(\|>)` | `a -> (a -> b) -> b` | `same-name` | |
| `(<\|)` | `(a -> b) -> a -> b` | `same-name` | |
| `(>>)` | `(a -> b) -> (b -> c) -> a -> c` | `MISSING` | no function-composition operator |
| `(<<)` | `(b -> c) -> (a -> b) -> a -> c` | `MISSING` | no function-composition operator |

### Basics — functions

| elm member | elm type sig | status | notes |
|---|---|---|---|
| `toFloat` | `Int -> Float` | `MISSING` | no `Int -> Float` widening kernel |
| `round` | `Float -> Int` | `renamed(Math.round)` | same semantics |
| `floor` | `Float -> Int` | `renamed(Math.floor)` | |
| `ceiling` | `Float -> Int` | `renamed(Math.ceil)` | name differs: `ceil` not `ceiling` |
| `truncate` | `Float -> Int` | `renamed(Math.trunc)` | name differs: `trunc` not `truncate` |
| `max` | `comparable -> comparable -> comparable` | `renamed(Math.max)` | also `Basics.max` (pre-auto) |
| `min` | `comparable -> comparable -> comparable` | `renamed(Math.min)` | also `Basics.min` |
| `compare` | `comparable -> comparable -> Order` | `renamed(Basics.compare)` | returns a simplified form; `Order` type (`LT`/`EQ`/`GT`) absent — see §divergences ER/R5 |
| `not` | `Bool -> Bool` | `same-name` | `Ipe.Basics.not` |
| `xor` | `Bool -> Bool -> Bool` | `MISSING` | boolean XOR absent |
| `modBy` | `Int -> Int -> Int` | `renamed(Basics.modBy)` | registered in canon Basics but typed as `number -> number -> number`; `Math.mod` is `Float -> Float -> Float` — see coverage.md §divergences |
| `remainderBy` | `Int -> Int -> Int` | `MISSING` | `Math.remainder` is `Float -> Float -> Float` only; no Int form |
| `negate` | `number -> number` | `renamed(Basics.negate)` | registered in Basics and auto-prelude; also handles unary `-x` desugar |
| `abs` | `number -> number` | `renamed(Math.abs)` | `Math.abs : Int -> Int` only — no `Float.abs`; semantic gap (Elm's is polymorphic) |
| `clamp` | `number -> number -> number -> number` | `same-name` | `Ipe.Basics.clamp` Ipê source |
| `sqrt` | `Float -> Float` | `same-name` | `Math.sqrt`; also `Basics.sqrt` |
| `logBase` | `Float -> Float -> Float` | `MISSING` | `Math.log`/`log2`/`log10` exist; no `logBase` combinator |
| `e` | `Float` | `renamed(Math.e)` | |
| `pi` | `Float` | `renamed(Math.pi)` | |
| `cos` | `Float -> Float` | `renamed(Math.cos)` | |
| `sin` | `Float -> Float` | `renamed(Math.sin)` | |
| `tan` | `Float -> Float` | `renamed(Math.tan)` | |
| `acos` | `Float -> Float` | `renamed(Math.acos)` | |
| `asin` | `Float -> Float` | `renamed(Math.asin)` | |
| `atan` | `Float -> Float` | `renamed(Math.atan)` | |
| `atan2` | `Float -> Float -> Float` | `renamed(Math.atan2)` | |
| `degrees` | `Float -> Float` | `MISSING` | angle conversion; trivial (`x * pi / 180`) |
| `radians` | `Float -> Float` | `MISSING` | identity wrapper (`x * 1`) |
| `turns` | `Float -> Float` | `MISSING` | `x * 2 * pi` |
| `toPolar` | `(Float, Float) -> (Float, Float)` | `MISSING` | rectangular → polar |
| `fromPolar` | `(Float, Float) -> (Float, Float)` | `MISSING` | polar → rectangular |
| `isNaN` | `Float -> Bool` | `renamed(Math.isNaN)` | added in #132 |
| `isInfinite` | `Float -> Bool` | `MISSING` | `Math.inf` is a constant; no predicate |
| `identity` | `a -> a` | `same-name` | `Ipe.Basics.identity` |
| `always` | `a -> b -> a` | `same-name` | `Ipe.Basics.always` |
| `never` | `Never -> a` | `intentional(R5)` | `Never` type absent by design |

**Summary (operators + functions):** 20 same-name · 14 renamed · 11 MISSING · 1 intentional  
**Types:** 3/5 present (Int, Float, Bool); `Order` and `Never` intentional(R5)

---

## Bitwise

**Status: intentionally absent (R5).** No integer bitwise operations.

| elm member | elm type sig | status | notes |
|---|---|---|---|
| `Bitwise.and` | `Int -> Int -> Int` | `intentional(R5)` | |
| `Bitwise.or` | `Int -> Int -> Int` | `intentional(R5)` | |
| `Bitwise.xor` | `Int -> Int -> Int` | `intentional(R5)` | distinct from `Basics.xor` (Bool) |
| `Bitwise.complement` | `Int -> Int` | `intentional(R5)` | |
| `Bitwise.shiftLeftBy` | `Int -> Int -> Int` | `intentional(R5)` | |
| `Bitwise.shiftRightBy` | `Int -> Int -> Int` | `intentional(R5)` | signed shift |
| `Bitwise.shiftRightZfBy` | `Int -> Int -> Int` | `intentional(R5)` | unsigned shift |

**Summary:** 0 same-name · 0 renamed · 0 MISSING · 7 intentional

---

## Char

**Semantic note:** `Char.toUpper` / `Char.toLower` return `String` (a single-rune String)
in ipê rather than `Char` as in Elm — a sanctioned divergence (STR1: rune-based
semantics; the Go/Rust `unicode.ToLower` result is naturally a `String`).

| elm member | elm type sig | status | notes |
|---|---|---|---|
| `Char.isUpper` | `Char -> Bool` | `same-name` | `Ipe.Char.isUpper` |
| `Char.isLower` | `Char -> Bool` | `same-name` | |
| `Char.isAlpha` | `Char -> Bool` | `same-name` | |
| `Char.isAlphaNum` | `Char -> Bool` | `MISSING` | combine `isAlpha \|\| isDigit` |
| `Char.isDigit` | `Char -> Bool` | `same-name` | |
| `Char.isOctDigit` | `Char -> Bool` | `MISSING` | |
| `Char.isHexDigit` | `Char -> Bool` | `MISSING` | |
| `Char.toUpper` | `Char -> Char` | `renamed(Char.toUpper)` | sig differs: returns `String` not `Char` (STR1) |
| `Char.toLower` | `Char -> Char` | `renamed(Char.toLower)` | same divergence |
| `Char.toLocaleUpper` | `Char -> Char` | `MISSING` | locale-aware; low priority |
| `Char.toLocaleLower` | `Char -> Char` | `MISSING` | locale-aware; low priority |
| `Char.toCode` | `Char -> Int` | `same-name` | `Ipe.Char.toCode` |
| `Char.fromCode` | `Int -> Char` | `same-name` | `Ipe.Char.fromCode` |

**Summary:** 6 same-name · 2 renamed · 5 MISSING · 0 intentional

---

## Debug

**Status: intentionally absent (R5).** Dev-only helpers replaced by `Ipe.Log`.

| elm member | elm type sig | status | notes |
|---|---|---|---|
| `Debug.toString` | `a -> String` | `intentional(R5)` | `String.fromInt`/`fromFloat` etc. are the typed split; `Basics.toString` is registered but unconstrained |
| `Debug.log` | `String -> a -> a` | `intentional(R5)` | `Ipe.Log.*` is the production logger (Task-tier, not pass-through) |
| `Debug.todo` | `String -> a` | `intentional(R5)` | no todo/panic escape hatch in user code |

**Summary:** 0 same-name · 0 renamed · 0 MISSING · 3 intentional

---

## Dict

| elm member | elm type sig | status | notes |
|---|---|---|---|
| `Dict.empty` | `Dict k v` | `same-name` | `Ipe.Dict.empty` |
| `Dict.singleton` | `comparable -> v -> Dict comparable v` | `MISSING` | trivial: `Dict.insert k v Dict.empty` |
| `Dict.insert` | `comparable -> v -> Dict comparable v -> Dict comparable v` | `same-name` | |
| `Dict.update` | `comparable -> (Maybe v -> Maybe v) -> Dict comparable v -> Dict comparable v` | `MISSING` | ergonomic gap; no HOF-dict-update |
| `Dict.remove` | `comparable -> Dict comparable v -> Dict comparable v` | `same-name` | |
| `Dict.isEmpty` | `Dict k v -> Bool` | `same-name` | |
| `Dict.member` | `comparable -> Dict comparable v -> Bool` | `same-name` | |
| `Dict.get` | `comparable -> Dict comparable v -> Maybe v` | `same-name` | |
| `Dict.size` | `Dict k v -> Int` | `same-name` | |
| `Dict.keys` | `Dict k v -> List k` | `same-name` | |
| `Dict.values` | `Dict k v -> List v` | `same-name` | |
| `Dict.toList` | `Dict k v -> List (k, v)` | `same-name` | |
| `Dict.fromList` | `List (comparable, v) -> Dict comparable v` | `same-name` | |
| `Dict.map` | `(k -> a -> b) -> Dict k a -> Dict k b` | `same-name` | |
| `Dict.foldl` | `(k -> v -> b -> b) -> b -> Dict k v -> b` | `same-name` | |
| `Dict.foldr` | `(k -> v -> b -> b) -> b -> Dict k v -> b` | `MISSING` | only left-fold is exposed |
| `Dict.filter` | `(comparable -> v -> Bool) -> Dict comparable v -> Dict comparable v` | `MISSING` | |
| `Dict.partition` | `(comparable -> v -> Bool) -> Dict comparable v -> (Dict comparable v, Dict comparable v)` | `MISSING` | |
| `Dict.union` | `Dict comparable v -> Dict comparable v -> Dict comparable v` | `same-name` | left-biased |
| `Dict.intersect` | `Dict comparable v -> Dict comparable v -> Dict comparable v` | `MISSING` | |
| `Dict.diff` | `Dict comparable a -> Dict comparable b -> Dict comparable a` | `MISSING` | |
| `Dict.merge` | `(comparable -> a -> result -> result) -> (comparable -> a -> b -> result -> result) -> (comparable -> b -> result -> result) -> Dict comparable a -> Dict comparable b -> result -> result` | `MISSING` | complex; blocks some patterns |

**Summary:** 14 same-name · 0 renamed · 8 MISSING · 0 intentional

---

## List

**ipê extras (not in Elm core):** `List.find`, `List.zip` — no parity change needed.

| elm member | elm type sig | status | notes |
|---|---|---|---|
| `(::)` | `a -> List a -> List a` | `same-name` | cons operator + `List.cons` |
| `List.singleton` | `a -> List a` | `MISSING` | trivial: `[x]` literal works; helper improves pipelines |
| `List.repeat` | `Int -> a -> List a` | `MISSING` | common initialisation pattern |
| `List.range` | `Int -> Int -> List Int` | `same-name` | `Ipe.List.range` |
| `List.map` | `(a -> b) -> List a -> List b` | `same-name` | Ipê source; TCO-optimised |
| `List.indexedMap` | `(Int -> a -> b) -> List a -> List b` | `same-name` | |
| `List.foldl` | `(a -> b -> b) -> b -> List a -> b` | `same-name` | TCO |
| `List.foldr` | `(a -> b -> b) -> b -> List a -> b` | `same-name` | O(N) stack |
| `List.filter` | `(a -> Bool) -> List a -> List a` | `same-name` | TCO |
| `List.filterMap` | `(a -> Maybe b) -> List a -> List b` | `same-name` | added #132 |
| `List.length` | `List a -> Int` | `same-name` | |
| `List.reverse` | `List a -> List a` | `same-name` | |
| `List.member` | `a -> List a -> Bool` | `same-name` | TCO |
| `List.all` | `(a -> Bool) -> List a -> Bool` | `same-name` | TCO |
| `List.any` | `(a -> Bool) -> List a -> Bool` | `same-name` | TCO |
| `List.maximum` | `List comparable -> Maybe comparable` | `MISSING` | very common; blocks display patterns |
| `List.minimum` | `List comparable -> Maybe comparable` | `MISSING` | |
| `List.sum` | `List number -> number` | `MISSING` | very common |
| `List.product` | `List number -> number` | `MISSING` | |
| `List.append` | `List a -> List a -> List a` | `same-name` | |
| `List.concat` | `List (List a) -> List a` | `same-name` | |
| `List.concatMap` | `(a -> List b) -> List a -> List b` | `same-name` | |
| `List.intersperse` | `a -> List a -> List a` | `MISSING` | |
| `List.map2` | `(a -> b -> result) -> List a -> List b -> List result` | `MISSING` | parallel list ops |
| `List.map3` | `(a -> b -> c -> result) -> List a -> List b -> List c -> List result` | `MISSING` | |
| `List.map4` | `(a -> b -> c -> d -> result) -> List a -> List b -> List c -> List d -> List result` | `MISSING` | |
| `List.map5` | `(a -> b -> c -> d -> e -> result) -> List a -> List b -> List c -> List d -> List e -> List result` | `MISSING` | |
| `List.sort` | `List comparable -> List comparable` | `MISSING` | unkeyed sort; common |
| `List.sortBy` | `(a -> comparable) -> List a -> List a` | `same-name` | added #132 |
| `List.sortWith` | `(a -> a -> Order) -> List a -> List a` | `MISSING` | UNBLOCKED — `Order` ADT + `Basics.compare` shipped (#123); plain port now |
| `List.isEmpty` | `List a -> Bool` | `same-name` | |
| `List.head` | `List a -> Maybe a` | `same-name` | |
| `List.tail` | `List a -> Maybe (List a)` | `same-name` | |
| `List.take` | `Int -> List a -> List a` | `same-name` | |
| `List.drop` | `Int -> List a -> List a` | `same-name` | TCO |
| `List.partition` | `(a -> Bool) -> List a -> (List a, List a)` | `MISSING` | |
| `List.unzip` | `List (a, b) -> (List a, List b)` | `MISSING` | inverse `zip` exists as ipê extra |

**Summary:** 22 same-name · 0 renamed · 14 MISSING · 0 intentional

---

## Maybe

**Complete Elm coverage.** ipê adds `Maybe.andMap`, `Maybe.combine`, `Maybe.isJust`, `Maybe.isNothing`
as extras.

| elm member | elm type sig | status | notes |
|---|---|---|---|
| `Maybe.withDefault` | `a -> Maybe a -> a` | `same-name` | `Ipe.Maybe.withDefault` |
| `Maybe.map` | `(a -> b) -> Maybe a -> Maybe b` | `same-name` | |
| `Maybe.map2` | `(a -> b -> value) -> Maybe a -> Maybe b -> Maybe value` | `same-name` | |
| `Maybe.map3` | `(a -> b -> c -> value) -> Maybe a -> Maybe b -> Maybe c -> Maybe value` | `same-name` | |
| `Maybe.map4` | `(a -> b -> c -> d -> value) -> Maybe a -> Maybe b -> Maybe c -> Maybe d -> Maybe value` | `same-name` | |
| `Maybe.map5` | `(a -> b -> c -> d -> e -> value) -> Maybe a -> Maybe b -> Maybe c -> Maybe d -> Maybe e -> Maybe value` | `same-name` | |
| `Maybe.andThen` | `(a -> Maybe b) -> Maybe a -> Maybe b` | `same-name` | |

**Summary:** 7 same-name · 0 renamed · 0 MISSING · 0 intentional — **complete**

---

## Platform

**Status: mostly n/a.** ipê has no user-facing `Platform` module;
programs are `Live.app`/`Tui.app`/`Cli`/`Webview.app`.

| elm member | elm type sig | status | notes |
|---|---|---|---|
| `Platform.worker` | `{ init : flags -> (model, Cmd msg), ... } -> Program flags model msg` | `n/a` | ipê program entry is `Live.app` / `Tui.app` / `Webview.app`; `Program` type absent |
| `Platform.sendToApp` | `Router msg a -> msg -> Task x ()` | `n/a` | no effect-manager / `Router` |
| `Platform.sendToSelf` | `Router a msg -> msg -> Task x ()` | `n/a` | no effect-manager |

---

## Platform.Cmd

| elm member | elm type sig | status | notes |
|---|---|---|---|
| `Cmd.none` | `Cmd msg` | `same-name` | `Ipe.Cmd.none` |
| `Cmd.batch` | `List (Cmd msg) -> Cmd msg` | `same-name` | `Ipe.Cmd.batch` |
| `Cmd.map` | `(a -> msg) -> Cmd a -> Cmd msg` | `MISSING` | no functor-map over Cmd; `Cmd.perform` fills a different role |

**Summary:** 2 same-name · 0 renamed · 1 MISSING

---

## Platform.Sub

| elm member | elm type sig | status | notes |
|---|---|---|---|
| `Sub.none` | `Sub msg` | `same-name` | `Ipe.Sub.none` |
| `Sub.batch` | `List (Sub msg) -> Sub msg` | `same-name` | `Ipe.Sub.batch` |
| `Sub.map` | `(a -> msg) -> Sub a -> Sub msg` | `MISSING` | no functor-map over Sub |

**Summary:** 2 same-name · 0 renamed · 1 MISSING

---

## Process

**Status: spawn/kill intentionally absent.** `Task.parallel` is the concurrency model;
there is no lightweight-process handle type.

| elm member | elm type sig | status | notes |
|---|---|---|---|
| `Process.spawn` | `Task x a -> Task y Id` | `intentional(E1)` | concurrency via `Task.parallel`; no `ProcessId` type |
| `Process.sleep` | `Float -> Task x ()` | `renamed(Time.sleep)` | `Ipe.Time.sleep : Float -> Task Error ()` — same semantics, different module |
| `Process.kill` | `Id -> Task x ()` | `intentional(E1)` | no `ProcessId` / task cancellation |

**Summary:** 0 same-name · 1 renamed · 0 MISSING · 2 intentional

---

## Result

| elm member | elm type sig | status | notes |
|---|---|---|---|
| `Result.withDefault` | `a -> Result x a -> a` | `same-name` | `Ipe.Result.withDefault` |
| `Result.map` | `(a -> value) -> Result x a -> Result x value` | `same-name` | |
| `Result.map2` | `(a -> b -> value) -> Result x a -> Result x b -> Result x value` | `same-name` | |
| `Result.map3` | `(a -> b -> c -> value) -> Result x a -> Result x b -> Result x c -> Result x value` | `same-name` | |
| `Result.map4` | `(a -> b -> c -> d -> value) -> Result x a -> Result x b -> Result x c -> Result x d -> Result x value` | `same-name` | |
| `Result.map5` | `(a -> b -> c -> d -> e -> value) -> Result x a -> Result x b -> Result x c -> Result x d -> Result x e -> Result x value` | `same-name` | |
| `Result.andThen` | `(a -> Result x b) -> Result x a -> Result x b` | `same-name` | |
| `Result.mapError` | `(x -> y) -> Result x a -> Result y a` | `same-name` | |
| `Result.toMaybe` | `Result x a -> Maybe a` | `MISSING` | one-liner: `case r of Ok x -> Just x; Err _ -> Nothing` |
| `Result.fromMaybe` | `x -> Maybe a -> Result x a` | `MISSING` | one-liner; bridges Maybe/Result worlds |

**Summary:** 8 same-name · 0 renamed · 2 MISSING · 0 intentional

---

## Set

**Note:** Set has **no higher-order traversal surface** in ipê — `map`, `foldl`, `foldr`,
`filter`, `partition` are all missing. Only set-algebra operations plus conversion are present.

| elm member | elm type sig | status | notes |
|---|---|---|---|
| `Set.empty` | `Set a` | `same-name` | `Ipe.Set.empty` |
| `Set.singleton` | `comparable -> Set comparable` | `MISSING` | |
| `Set.insert` | `comparable -> Set comparable -> Set comparable` | `same-name` | |
| `Set.remove` | `comparable -> Set comparable -> Set comparable` | `same-name` | |
| `Set.isEmpty` | `Set a -> Bool` | `MISSING` | only `size` exposed; `size s == 0` is the workaround |
| `Set.member` | `comparable -> Set comparable -> Bool` | `same-name` | |
| `Set.size` | `Set a -> Int` | `same-name` | |
| `Set.toList` | `Set a -> List a` | `same-name` | |
| `Set.fromList` | `List comparable -> Set comparable` | `same-name` | |
| `Set.map` | `(comparable -> comparable2) -> Set comparable -> Set comparable2` | `MISSING` | no traversal kernels for Set |
| `Set.foldl` | `(a -> b -> b) -> b -> Set a -> b` | `MISSING` | |
| `Set.foldr` | `(a -> b -> b) -> b -> Set a -> b` | `MISSING` | |
| `Set.filter` | `(comparable -> Bool) -> Set comparable -> Set comparable` | `MISSING` | |
| `Set.partition` | `(comparable -> Bool) -> Set comparable -> (Set comparable, Set comparable)` | `MISSING` | |
| `Set.union` | `Set comparable -> Set comparable -> Set comparable` | `same-name` | |
| `Set.intersect` | `Set comparable -> Set comparable -> Set comparable` | `same-name` | |
| `Set.diff` | `Set comparable -> Set comparable -> Set comparable` | `same-name` | |

**Summary:** 10 same-name · 0 renamed · 7 MISSING · 0 intentional

---

## String

**Semantic note (STR1):** All ipê `String` operations count **Unicode code points (runes)**,
not UTF-16 code units. Elm counts UTF-16 code units (JavaScript-backed). This means emoji
and astral-plane characters count as 1 in ipê, 2 in Elm. This is a sanctioned, intentional
divergence that improves correctness.

**ipê extras (not in Elm core):** `casefold`, `equalFold`, `isEmail`, `isUrl`,
`containsIn`, `startsWithIn`, `endsWithIn` — no parity change needed.

| elm member | elm type sig | status | notes |
|---|---|---|---|
| `String.isEmpty` | `String -> Bool` | `same-name` | |
| `String.length` | `String -> Int` | `same-name` | counts code points (STR1) |
| `String.reverse` | `String -> String` | `same-name` | code-point reverse |
| `String.repeat` | `Int -> String -> String` | `same-name` | |
| `String.replace` | `String -> String -> String -> String` | `same-name` | |
| `String.append` | `String -> String -> String` | `same-name` | same as `(++)` |
| `String.concat` | `List String -> String` | `same-name` | |
| `String.split` | `String -> String -> List String` | `same-name` | |
| `String.join` | `String -> List String -> String` | `same-name` | |
| `String.words` | `String -> List String` | `same-name` | |
| `String.lines` | `String -> List String` | `same-name` | |
| `String.slice` | `Int -> Int -> String -> String` | `same-name` | rune-indexed |
| `String.left` | `Int -> String -> String` | `MISSING` | equivalent to `slice 0 n` |
| `String.right` | `Int -> String -> String` | `MISSING` | `dropLeft (length s - n) s` |
| `String.dropLeft` | `Int -> String -> String` | `same-name` | rune-based |
| `String.dropRight` | `Int -> String -> String` | `same-name` | rune-based |
| `String.contains` | `String -> String -> Bool` | `same-name` | needle-first |
| `String.startsWith` | `String -> String -> Bool` | `same-name` | needle-first |
| `String.endsWith` | `String -> String -> Bool` | `same-name` | needle-first |
| `String.indexes` | `String -> String -> List Int` | `MISSING` | find all occurrence positions |
| `String.indices` | `String -> String -> List Int` | `MISSING` | alias of `indexes` in Elm |
| `String.toInt` | `String -> Maybe Int` | `same-name` | |
| `String.fromInt` | `Int -> String` | `same-name` | |
| `String.toFloat` | `String -> Maybe Float` | `same-name` | note: parse, not coerce |
| `String.fromFloat` | `Float -> String` | `same-name` | |
| `String.fromChar` | `Char -> String` | `same-name` | |
| `String.cons` | `Char -> String -> String` | `MISSING` | prepend a Char |
| `String.uncons` | `String -> Maybe (Char, String)` | `MISSING` | split head Char |
| `String.toList` | `String -> List Char` | `same-name` | |
| `String.fromList` | `List Char -> String` | `same-name` | |
| `String.toUpper` | `String -> String` | `same-name` | |
| `String.toLower` | `String -> String` | `same-name` | |
| `String.pad` | `Int -> Char -> String -> String` | `MISSING` | center-pad; only `padLeft`/`padRight` present |
| `String.padLeft` | `Int -> Char -> String -> String` | `same-name` | |
| `String.padRight` | `Int -> Char -> String -> String` | `same-name` | |
| `String.trim` | `String -> String` | `same-name` | |
| `String.trimLeft` | `String -> String` | `renamed(String.trimStart)` | name differs; same semantics |
| `String.trimRight` | `String -> String` | `renamed(String.trimEnd)` | name differs; same semantics |
| `String.map` | `(Char -> Char) -> String -> String` | `MISSING` | char-level transform |
| `String.filter` | `(Char -> Bool) -> String -> String` | `MISSING` | char-level filter |
| `String.foldl` | `(Char -> b -> b) -> b -> String -> b` | `MISSING` | char fold |
| `String.foldr` | `(Char -> b -> b) -> b -> String -> b` | `MISSING` | char fold |
| `String.any` | `(Char -> Bool) -> String -> Bool` | `MISSING` | `String.any f = not . String.all (not . f)` |
| `String.all` | `(Char -> Bool) -> String -> Bool` | `MISSING` | char predicate scan |

**Summary:** 29 same-name · 2 renamed · 13 MISSING · 0 intentional

---

## Task

**Semantic divergence (R4):** ipê's `Task` fixes the error channel to `Error` — every
combinator is `Task Error a`, not the polymorphic `Task x a` Elm exposes. There is no
`Never`-error form. `Task.perform` is relocated to `Cmd.perform` with a different
signature. These are intentional design choices, not gaps.

| elm member | elm type sig | status | notes |
|---|---|---|---|
| `Task.perform` | `(a -> msg) -> Task Never a -> Cmd msg` | `renamed(Cmd.perform)` | ipê: `Cmd.perform : Task Error a -> (Result Error a -> msg) -> Cmd msg` — different sig (E6) |
| `Task.attempt` | `(Result x a -> msg) -> Task x a -> Cmd msg` | `MISSING` | no `Task.attempt`; `Cmd.perform` covers the common case |
| `Task.andThen` | `(a -> Task x b) -> Task x a -> Task x b` | `same-name` | `Ipe.Task.andThen` |
| `Task.succeed` | `a -> Task x a` | `same-name` | `Task.succeed : a -> Task Error a` |
| `Task.fail` | `x -> Task x a` | `same-name` | `Task.fail : Error -> Task Error a` |
| `Task.sequence` | `List (Task x a) -> Task x (List a)` | `same-name` | + `Task.parallel` as ipê extra |
| `Task.map` | `(a -> b) -> Task x a -> Task x b` | `same-name` | |
| `Task.map2` | `(a -> b -> result) -> Task x a -> Task x b -> Task x result` | `MISSING` | use `Task.andThen` + `Task.map` |
| `Task.map3` | `(a -> b -> c -> result) -> Task x a -> Task x b -> Task x c -> Task x result` | `MISSING` | |
| `Task.map4` | `(a -> b -> c -> d -> result) -> Task x a -> Task x b -> Task x c -> Task x d -> Task x result` | `MISSING` | |
| `Task.map5` | `(a -> b -> c -> d -> e -> result) -> Task x a -> Task x b -> Task x c -> Task x d -> Task x e -> Task x result` | `MISSING` | |
| `Task.mapError` | `(x -> y) -> Task x a -> Task y a` | `same-name` | fixed error channel: `Error -> Error` |
| `Task.onError` | `(x -> Task y a) -> Task x a -> Task y a` | `same-name` | |

**Summary:** 7 same-name · 1 renamed · 5 MISSING · 0 intentional

---

## Tuple

**Status: no `Tuple` module (R5).** `first`/`second` live in `Basics` as `fst`/`snd`.
`pair`, `mapFirst`, `mapSecond`, `mapBoth` have no equivalent.

| elm member | elm type sig | status | notes |
|---|---|---|---|
| `Tuple.pair` | `a -> b -> (a, b)` | `intentional(R5)` | tuple syntax `(a, b)` covers this |
| `Tuple.first` | `(a, b) -> a` | `renamed(Basics.fst)` | `fst` in Basics |
| `Tuple.second` | `(a, b) -> b` | `renamed(Basics.snd)` | `snd` in Basics |
| `Tuple.mapFirst` | `(a -> x) -> (a, b) -> (x, b)` | `MISSING` | |
| `Tuple.mapSecond` | `(b -> y) -> (a, b) -> (a, y)` | `MISSING` | |
| `Tuple.mapBoth` | `(a -> x) -> (b -> y) -> (a, b) -> (x, y)` | `MISSING` | |

**Summary:** 0 same-name · 2 renamed · 3 MISSING · 1 intentional

---

## Grand Summary

| Module | Elm values | same-name | renamed | MISSING | intentional | n/a | Coverage % |
|---|---|---|---|---|---|---|---|
| Array | 18 | 0 | 0 | 0 | 18 | 0 | 0% (by design) |
| Basics (ops) | 19 | 16 | 0 | 3 | 0 | 0 | 84% |
| Basics (funcs) | 35 | 11 | 13 | 9 | 2 | 0 | 69% (incl renamed) |
| Bitwise | 7 | 0 | 0 | 0 | 7 | 0 | 0% (by design) |
| Char | 13 | 6 | 2 | 5 | 0 | 0 | 62% (incl renamed) |
| Debug | 3 | 0 | 0 | 0 | 3 | 0 | 0% (by design) |
| Dict | 22 | 14 | 0 | 8 | 0 | 0 | 64% |
| List | 37 | 22 | 0 | 14 | 0 | 0 | 59% |
| Maybe | 7 | 7 | 0 | 0 | 0 | 0 | **100%** |
| Platform | 3 | 0 | 0 | 0 | 0 | 3 | n/a |
| Platform.Cmd | 3 | 2 | 0 | 1 | 0 | 0 | 67% |
| Platform.Sub | 3 | 2 | 0 | 1 | 0 | 0 | 67% |
| Process | 3 | 0 | 1 | 0 | 2 | 0 | 33% |
| Result | 10 | 8 | 0 | 2 | 0 | 0 | 80% |
| Set | 17 | 10 | 0 | 7 | 0 | 0 | 59% |
| String | 44 | 29 | 2 | 13 | 0 | 0 | 70% (incl renamed) |
| Task | 13 | 7 | 1 | 5 | 0 | 0 | 62% (incl renamed) |
| Tuple | 6 | 0 | 2 | 3 | 1 | 0 | 33% (incl renamed) |
| **Total** | **264** | **134** | **21** | **71** | **33** | **3** | **59%** same-name, **67%** incl renamed |

> Coverage % = (same-name + renamed) / (total − intentional − n/a).
> Excluding the five intentionally-absent modules (Array, Bitwise, Debug + Tuple/Process
> partials), effective coverage of the remaining surface is **155/194 ≈ 80%**.

**Types:** 17 of 20 Elm types present; `Order`, `Never`, `Array a` are intentionally absent.

---

## Semantic divergences noted from signatures

1. **`Char.toUpper`/`toLower` return `String` not `Char`.** Every Unicode case-fold
   can produce a multi-codepoint result (e.g. German `ß` → `SS`); returning `String`
   is correct. Elm returns `Char` (JavaScript char — no multi-codepoint issue in UTF-16).
   Migration from Elm: wrap with a `String.toList >> List.head >> Maybe.withDefault '?'`
   if the old `Char` return type is needed.

2. **`String.*` counts code points, not UTF-16 code units (STR1).** `String.length`
   returns 1 for emoji (🎉) vs 2 in Elm. Slicing never splits a codepoint.
   Intentional; see `divergences-from-elm.md §4.5`.

3. **`Basics.compare` has no `Order` type.** `compare` is registered and dispatches,
   but the `LT`/`EQ`/`GT` constructors are absent. `List.sortWith` (which Elm types
   via `Order`) is therefore also blocked. This is the main blocker for porting Elm
   code that pattern-matches on comparison results.

4. **`Basics.abs` is `Int -> Int` only.** Elm's `abs : number -> number` also handles
   `Float`. ipê has no `Float.abs` surface under `Basics`.

5. **`Basics.modBy` typed as `number -> number -> number`.** Elm's is `Int -> Int -> Int`.
   The underlying `Math.mod` is `Float -> Float -> Float`. Integer modulo semantics
   (non-negative result) may diverge for negative inputs.

6. **`Task` error channel fixed to `Error`.** Elm's `Task x a` is polymorphic. This
   means Elm idioms like `Task.perform identity (Task.succeed value)` (which produce
   `Cmd msg`) don't directly translate — use `Cmd.perform`.

7. **`String.trimLeft`/`trimRight` renamed `trimStart`/`trimEnd`.** Mirrors the
   Web/Rust naming convention; the semantics are identical.

---

## Prioritized implementation backlog

Grouped into build-lane batches (~5–10 items) ordered by user impact.
Each batch is independently implementable.

### Batch A — List core numerics (very high impact, pure)
These are the most-reached-for List functions in any application.

| Member | Elm sig | Notes |
|---|---|---|
| `List.maximum` | `List comparable -> Maybe comparable` | foldl-based |
| `List.minimum` | `List comparable -> Maybe comparable` | foldl-based |
| `List.sum` | `List number -> number` | foldl-based |
| `List.product` | `List number -> number` | foldl-based |
| `List.singleton` | `a -> List a` | trivial: `[x]` wrapper |
| `List.repeat` | `Int -> a -> List a` | range + map |

### Batch B — List sort/parallel (high impact)
Enable sorted display and parallel list transforms.

| Member | Elm sig | Notes |
|---|---|---|
| `List.sort` | `List comparable -> List comparable` | needs `comparable` constraint |
| `List.map2` | `(a -> b -> result) -> List a -> List b -> List result` | zip-map |
| `List.intersperse` | `a -> List a -> List a` | fold-based |
| `List.partition` | `(a -> Bool) -> List a -> (List a, List a)` | two-pass or fold |

### Batch C — List structural (medium impact)
Rarely critical but often needed for idiomatic Elm ports.

| Member | Elm sig | Notes |
|---|---|---|
| `List.unzip` | `List (a, b) -> (List a, List b)` | fold-based |
| `List.sortWith` | `(a -> a -> Order) -> List a -> List a` | UNBLOCKED — `Order` shipped (#123) |
| `List.map3`/`map4`/`map5` | (parallel N-list map) | may use `map2` + `andMap` |

### Batch D — Dict traversal (high impact)
`Dict.update` and `Dict.filter` are very common patterns.

| Member | Elm sig | Notes |
|---|---|---|
| `Dict.singleton` | `comparable -> v -> Dict comparable v` | trivial |
| `Dict.update` | `comparable -> (Maybe v -> Maybe v) -> Dict comparable v -> Dict comparable v` | HOF-dict; needs careful typing |
| `Dict.filter` | `(comparable -> v -> Bool) -> Dict comparable v -> Dict comparable v` | foldl-based |
| `Dict.foldr` | `(k -> v -> b -> b) -> b -> Dict k v -> b` | iteration order |
| `Dict.partition` | `(comparable -> v -> Bool) -> Dict comparable v -> (Dict comparable v, Dict comparable v)` | filter-based |

### Batch E — Dict set-ops (medium impact)
Complete the Dict algebra for merge workflows.

| Member | Elm sig | Notes |
|---|---|---|
| `Dict.intersect` | `Dict comparable v -> Dict comparable v -> Dict comparable v` | filter-based |
| `Dict.diff` | `Dict comparable a -> Dict comparable b -> Dict comparable a` | filter-based |
| `Dict.merge` | (3-way fold) | complex; blocks some composition patterns |

### Batch F — Set traversal (medium impact)
Set has **no** HOF surface at all today.

| Member | Elm sig | Notes |
|---|---|---|
| `Set.isEmpty` | `Set a -> Bool` | trivial: `size s == 0` |
| `Set.singleton` | `comparable -> Set comparable` | trivial |
| `Set.foldl` | `(a -> b -> b) -> b -> Set a -> b` | unlocks all others |
| `Set.map` | `(comparable -> comparable2) -> Set comparable -> Set comparable2` | foldl + fromList |
| `Set.filter` | `(comparable -> Bool) -> Set comparable -> Set comparable` | foldl-based |
| `Set.partition` | `(comparable -> Bool) -> Set comparable -> (Set comparable, Set comparable)` | foldl-based |

### Batch G — String char-level (medium impact)
Needed for string validation and character-level transforms.

| Member | Elm sig | Notes |
|---|---|---|
| `String.map` | `(Char -> Char) -> String -> String` | toList + map + fromList |
| `String.filter` | `(Char -> Bool) -> String -> String` | same approach |
| `String.any` | `(Char -> Bool) -> String -> Bool` | short-circuit scan |
| `String.all` | `(Char -> Bool) -> String -> Bool` | short-circuit scan |
| `String.foldl` | `(Char -> b -> b) -> b -> String -> b` | base for G above |
| `String.foldr` | `(Char -> b -> b) -> b -> String -> b` | |

### Batch H — String navigation (medium impact)
Simple helpers that simplify common patterns.

| Member | Elm sig | Notes |
|---|---|---|
| `String.left` | `Int -> String -> String` | `slice 0 n` |
| `String.right` | `Int -> String -> String` | `dropLeft (length - n)` |
| `String.cons` | `Char -> String -> String` | `fromChar c ++ s` |
| `String.uncons` | `String -> Maybe (Char, String)` | head + tail of chars |
| `String.pad` | `Int -> Char -> String -> String` | center-pad |
| `String.indexes` | `String -> String -> List Int` | find all occurrences |

### Batch I — Result/Maybe bridges (low count, high usability impact)

| Member | Elm sig | Notes |
|---|---|---|
| `Result.toMaybe` | `Result x a -> Maybe a` | trivial one-liner |
| `Result.fromMaybe` | `x -> Maybe a -> Result x a` | trivial one-liner |

### Batch J — Basics numerics (medium impact)

| Member | Elm sig | Notes |
|---|---|---|
| `Basics.toFloat` | `Int -> Float` | `Int -> Float` widening; trivial at runtime |
| `Basics.remainderBy` | `Int -> Int -> Int` | signed remainder; distinct from modBy |
| `Basics.xor` | `Bool -> Bool -> Bool` | boolean XOR |
| `Basics.isInfinite` | `Float -> Bool` | `x == inf \|\| x == -inf` |
| `Basics.logBase` | `Float -> Float -> Float` | `log x / log base` |

### Batch K — Basics geometry (low impact)
Only needed for trigonometry-heavy applications.

| Member | Elm sig | Notes |
|---|---|---|
| `Basics.degrees` | `Float -> Float` | `x * pi / 180.0` |
| `Basics.radians` | `Float -> Float` | identity wrapper |
| `Basics.turns` | `Float -> Float` | `x * 2.0 * pi` |
| `Basics.toPolar` | `(Float, Float) -> (Float, Float)` | rectangular → polar |
| `Basics.fromPolar` | `(Float, Float) -> (Float, Float)` | polar → rectangular |
| `(^)` | `number -> number -> number` | power operator (`Math.pow` is Float-only) |
| `(>>)` and `(<<)` | function composition | no composition operators |

### Batch L — Char completeness (low-medium impact)

| Member | Elm sig | Notes |
|---|---|---|
| `Char.isAlphaNum` | `Char -> Bool` | `isAlpha \|\| isDigit` |
| `Char.isHexDigit` | `Char -> Bool` | `0-9 a-f A-F` |
| `Char.isOctDigit` | `Char -> Bool` | `0-7` |

### Batch M — Task map2-5 (medium impact)
Useful for parallel independent tasks without `sequence`.

| Member | Elm sig | Notes |
|---|---|---|
| `Task.map2` | `(a -> b -> result) -> Task x a -> Task x b -> Task x result` | `andThen` + `map` |
| `Task.map3`–`Task.map5` | (N-ary) | same pattern |
| `Task.attempt` | `(Result x a -> msg) -> Task x a -> Cmd msg` | thin wrapper |

### Batch N — Tuple helpers (low impact)
Rarely needed; tuple literals cover the constructor case.

| Member | Elm sig | Notes |
|---|---|---|
| `Tuple.mapFirst` | `(a -> x) -> (a, b) -> (x, b)` | |
| `Tuple.mapSecond` | `(b -> y) -> (a, b) -> (a, y)` | |
| `Tuple.mapBoth` | `(a -> x) -> (b -> y) -> (a, b) -> (x, y)` | |

### Batch O — Cmd.map / Sub.map (low-medium impact)
Enables composing sub-components with different Msg types in the TEA update loop.

| Member | Elm sig | Notes |
|---|---|---|
| `Cmd.map` | `(a -> msg) -> Cmd a -> Cmd msg` | needed for component composition |
| `Sub.map` | `(a -> msg) -> Sub a -> Sub msg` | same |

### Deferred / blocked items
- **`List.sortWith`** — UNBLOCKED: `Order` ADT (LT|EQ|GT) + `Basics.compare` shipped in #123; plain port
- **`List.map3`/`map4`/`map5`** — can wait until `map2` lands (same pattern)
- **`Array`** — no planned implementation; use `List`
- **`Bitwise`** — no planned implementation; use FFI for bit-manipulation needs

---

*Regeneration: compare the kernel registry (`crates/sky_kernels/src/lib.rs`)
+ canon qualifiers (`crates/sky_canon/src/env.rs`) against
`elm/core` 1.0.5 `docs.json`. Cross-reference the full narrative in
`elm-core-coverage.md` and the sanctioned divergences ledger in
`divergences-from-elm.md`.*
