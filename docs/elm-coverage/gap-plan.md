# Elm coverage gap plan

> Prioritized spec + implementation plan for the **relevant-but-missing** Elm
> API, grouped by package/subsystem. The audit and the full verdict table live
> in [`README.md`](README.md) and
> [`elm-core-coverage.md`](elm-core-coverage.md). This doc only plans the
> **`missing`** rows that are worth closing — `excluded` rows are out of scope by
> justification, not by oversight.
>
> **Anti-drift note.** Every new kernel or stdlib value must update **all**
> anti-drift sites (DEVELOPMENT.md §0b): `ipe_kernels` (enum + `decl()` + `ALL`),
> `ipe_types::constrain` (type-scheme + `FIRST_SCHEMED`), `ipe_lower` (arity table
> + `REGISTRY_ONLY_ALLOWLIST` for alias-only), `ipe_backend_rust/naming.rs`,
> `ipe_ir::pretty`, and `crates/ipe/src/stdlib.rs` (module registration). Values
> implementable as pure Ipê source (folds over existing kernels) need **no** new
> kernel — prefer that path. Fenced signatures below are **illustrative** (not
> compiled).

Priorities: **P1** high-frequency, cheap, pure; **P2** common, moderate;
**P3** heavier or lower-frequency surfaces.

---

## P1 — pure `elm/core` fills (mostly Ipê-source, no new kernels)

The single highest-value cluster: these are everyday functions, most expressible
as pure Ipê folds over existing kernels, so they mostly avoid the kernel
anti-drift dance entirely (add to the module `.ipe` + its `exposing` list).

### List numerics + shape (`src/stdlib/Ipe/List.ipe`)
Missing: `singleton`, `repeat`, `filterMap`, `maximum`, `minimum`, `sum`,
`product`, `intersperse`, `partition`, `unzip`, `map2`–`map5`, `sort`, `sortBy`,
`sortWith`.

```elm
-- illustrative; foldr/foldl based, pure Ipê source
sum : List number -> number
sum = List.foldl (+) 0

maximum : List comparable -> Maybe comparable
maximum xs = case xs of
    []      -> Nothing
    y :: ys -> Just (List.foldl max y ys)
```
`sort`/`sortBy`/`sortWith` need a comparison; `sortWith` already has a kernel
(`List.sortWith` in the runtime) — expose `sort`/`sortBy` on top of it. This is
the most reached-for gap.

### Dict/Set traversal (`Ipe/Dict.ipe`, `Ipe/Set.ipe`)
Dict missing: `singleton`, `update`, `foldr`, `filter`, `partition`, `intersect`,
`diff`, `merge`. Set missing: `singleton`, `isEmpty`, `map`, `foldl`, `foldr`,
`filter`, `partition`. Set has **no** higher-order surface today — landing
`foldl` unlocks the rest as pure source.

### Result/Maybe bridges (`Ipe/Result.ipe`)
Missing: `toMaybe`, `fromMaybe` — two trivial pure functions.

### Char completeness (`Ipe/Char.ipe`)
Missing predicates: `isAlphaNum`, `isHexDigit`, `isOctDigit`. Small kernels or
pure combinations of existing `isDigit`/`isAlpha`.

### String char-level + navigation (`Ipe/String.ipe`)
Missing: `left`, `right`, `cons`, `uncons`, `pad`, `indexes`/`indices`, and the
char-fold family `map`/`filter`/`foldl`/`foldr`/`any`/`all`. `left`/`right` are
thin wrappers over the existing rune-based `slice`; the char-fold family needs
`toList`-then-fold (present) or new kernels for hot paths.

---

## P1 — Cmd.map / Sub.map (`Ipe/Cmd`, `Ipe/Sub`)

`Cmd.map : (a -> msg) -> Cmd a -> Cmd msg` and `Sub.map` are absent. These unlock
composing sub-components with different `Msg` types in the TEA update loop — a
structural TEA need, not a nicety. Requires runtime support in the effect
representation (`runtime/src/ipe_runtime/`) plus the type-scheme in
`ipe_types::constrain`.

---

## P2 — Basics numerics + prelude ergonomics

Two coupled decisions from [`README.md`](README.md) §2:

1. **Re-export `Ipe.Math` numerics through the default prelude** so `round`/
   `floor`/`sqrt`/`abs`/trig are zero-import like Elm's `Basics`. Today they need
   `import Ipe.Math`. This is a canon-prelude (`QUALIFIERS`) change, not new
   kernels.
2. **Give the registered-but-untyped `Basics` qualifiers real kernel types:**
   `compare`, `modBy`, `negate`, plus add `toFloat` (`Int -> Float` widening),
   `remainderBy`, `xor` (Bool), `isNaN`, `isInfinite`, `logBase`,
   `degrees`/`radians`/`turns`/`toPolar`/`fromPolar`. Each needs the full kernel
   anti-drift update.

Operators `(^)`, `(>>)`, `(<<)`: composition + power. `(>>)`/`(<<)` are a
parser/binop-table change (`ipe_parse` + the binop resolver); `(^)` is a numeric
kernel + binop entry. Medium effort, high familiarity payoff.

The **`Order` ADT decision** is a prerequisite for a clean `compare`/`sortWith`
user surface: either expose `LT`/`EQ`/`GT` as a real union or keep `compare`
opaque. Recommend deciding before landing `sortWith` publicly.

---

## P2 — Task combinators (`Ipe/Task.ipe`) — **DONE**

`map2`–`map5` + `attempt` now present. `map2..5` are kernels over new runtime
`task_map2..5` (Elm semantics: argument-ordered effects, first `Err` short-
circuits — a captured `Task` cannot be forwarded through a Rust closure, so pure
`andThen`/`map` composition is unavailable, IPE-L0126). `attempt : (Result Error
a -> msg) -> Task Error a -> Cmd msg` reuses the runtime `cmd_perform` (the
`Cmd.perform` bridge, args swapped).

---

## P2 — Json decoder combinators (`Ipe/Config.ipe`) — **DONE**

`map2`–`map8`, `oneOf`, `maybe`, `index`, `keyValuePairs`, `dict` now present.
`map2..8` and `oneOf` are the load-bearing ones (record decoding, union
decoding). All are kernels sharing the runtime `decode_*` / `config_*` fns
(`decode_map2..8`, `decode_one_of`, `decode_index`, `decode_key_value_pairs`,
`config_maybe`, `config_dict`) — the opaque `Decoder` carrier cannot be forwarded
through a Rust closure, so pure combinator bodies are unavailable (IPE-L0126).
No JSON **encoder** value surface is planned here (serialization stays
per-effect); flag if a typed `Json.Encode` analogue is wanted later.

---

## P2 — `elm/parser` counterpart (new `Ipe.Parser`)

No parser-combinator library exists. A new `Ipe.Parser` module covering the core
`succeed`/`|=`/`|.`/`oneOf`/`chompIf`/`chompWhile`/`getChompedString`/`loop`/`run`
surface would fill a genuine text-parsing gap. Larger design effort (new module,
`Parser`/`Step`/`Problem` types); defer `Parser.Advanced`. Medium priority — pure,
self-contained, no runtime effects.

---

## P2 — Random `Generator` monad (`Ipe/Random.ipe`)

Add the composable `Generator a` surface (`map`/`map2..5`/`andThen`/`constant`/
`uniform`/`list`/`pair`) on top of the existing seeded primitives, so structured
and property-style generation works. `Ipe.Test` (property testing) is the
motivating consumer. The seeded pure surface already exists; this is combinator
layering.

---

## P3 — Url types + builder (new `Ipe.Url`)

Beyond `Http.parseQuery`, add a typed `Url` value (`fromString`/`toString`/
`percentEncode`/`percentDecode`) and `Url.Builder` (`absolute`/`relative`/
`crossOrigin`/`string`/`int`/`toQuery`). Useful for the HTTP client. The
`Url.Parser` routing combinators (`</>`/`<?>`) are lower priority given
server-side routing; scope this to the value + builder first.

---

## P3 — Bytes binary DSL (`Ipe/Bytes.ipe`)

`Ipe.Bytes` has the value + hex/base64/utf-8 codecs. The structured
`Bytes.Decode`/`Bytes.Encode` combinator DSL (`unsignedInt16`/`float64`/
`sequence`/`Endianness`/`loop`) is a heavier, lower-frequency surface — defer
until a concrete binary-format consumer appears.

---

## P3 — Regex options/match record (`Ipe/Regex.ipe`)

The compiled `Regex` handle shipped: `compile : String -> Result Error Regex`
parses a pattern once (an invalid pattern is a typed `Err`), and
`match`/`find`/`findAll`/`replace`/`split` take the compiled handle. Still
additive-and-optional: a `fromStringWith`/`Options` record, a richer `Match`
record (capture groups + indices), and count-limited
`findAtMost`/`replaceAtMost`/`splitAtMost`. Low priority; the current surface
covers the common path.

---

## P3 — Time `Posix`/`Zone` public types (`Ipe/Time.ipe`)

Decide whether to surface a public `Posix`/`Zone`/`Weekday`/`Month` value model
for calendar-arithmetic user code, versus keeping the formatting-first surface.
Only pursue if a consumer needs decomposed date fields; otherwise keep excluded.

---

## Explicitly not planned (see exclusions in `README.md` §4)

`Array`, `Bitwise`, `Tuple` (module), `Debug`, `elm/html`, `elm/svg`,
`elm/virtual-dom`, `elm/browser`, `Platform.worker`/effect-managers,
`Process.spawn`/`kill`, browser `File.Select`/`Download`. `Array`/`Bitwise` have
no pure plan (use `List` / FFI); the additive-surface designs, if that changes,
are tracked as a GitHub issue.
