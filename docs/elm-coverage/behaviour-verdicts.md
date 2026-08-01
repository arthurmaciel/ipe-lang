# Stdlib behaviour verdicts vs Elm

A behaviour audit of the JSON/String stdlib against Elm's documented `elm/json`
and `elm/core` semantics. Each row states Ipê's actual runtime behaviour, Elm's,
the most-correct behaviour, and the verdict. Verdicts obey `PRINCIPLES.md`:
Elm-parity never overrides a higher principle — where Ipê is stronger (Security,
Correctness, Soundness, parse-don't-validate) we keep ours and record the
divergence; where Elm is better or equal and weakens nothing, we match it.

Every verdict is pinned by a test:

- integer / object-key / `oneOf` / float-encode verdicts →
  `json::elm_behaviour_verdicts` in `src/runtime/rust/src/json.rs`.
- `nullable` verdicts → `json::elm_behaviour_verdicts` (config-gated).
- `String.fromFloat` verdicts → `string::tests` (`verdict_*`) in
  `src/runtime/rust/src/string.rs`.

## Verdict table

| # | Behaviour | Ipê (before) | Elm | Most-correct | Verdict |
|---|-----------|--------------|-----|--------------|---------|
| 1 | `Decode.int` on `1.5` | truncated to `1` | rejects | reject (parse-don't-validate) | **fix-to-most-correct** — now a typed `Err` |
| 1 | `Decode.int` on `1.0` | `1` | `1` (integral) | `1` | matches — integral float still decodes |
| 1 | `Decode.int` on `1e21` | truncated / saturated | rejects (out of `Int`) | reject | **fix-to-most-correct** — typed `Err` |
| 2 | `Encode.object` key order | sorted (lexicographic) | insertion order | either (both deterministic) | **keep-ours** — sorted matches the Go oracle |
| 3 | `String.fromFloat 1.0` | `"1"` | `"1"` | `"1"` | matches |
| 3 | `String.fromFloat (0.1+0.2)` | `"0.30000000000000004"` | `"0.30000000000000004"` | shortest round-trip | matches |
| 3 | `String.fromFloat 1e-7` | `"1e-07"` | `"1e-7"` | Go 'g' shape | **keep-ours** — Go-oracle parity |
| 3 | `String.fromFloat -0.0` | `"-0"` | `"0"` | preserve sign | **keep-ours** — Go-oracle parity |
| 3 | JSON number `Encode.float 1.0` | `1` | `1` | `1` | matches |
| 4 | `nullable` on `null` | `Nothing` | `Nothing` | `Nothing` | matches |
| 4 | `nullable` on non-null failing inner | `Err` | `Err` | `Err` | matches |
| 4 | `oneOf` first success | first `Ok` | first `Ok` | first `Ok` | matches |
| 4 | `oneOf` empty / all-fail | `Err` (no panic) | `Err` | typed `Err`, never panic | matches |

## Details

### 1. Integer-decoder strictness — fixed

Elm's `Json.Decode.int` rejects any JSON number that is not an integer, so `1.5`
fails and `1.0` (integral) succeeds. Ipê previously unmarshalled every number to
`f64` and truncated toward zero (`1.5` → `1`, `1e21` → a saturated `i64`), a
silent lossy coercion.

That truncation violates **parse, don't validate**: an `Int` decoder must yield
an integer or a typed rejection, never a quietly mangled value. The fix rejects
non-integral and out-of-`i64`-range numbers with a typed `Err`, while still
accepting an integral value written in float form (`1.0`). This simultaneously
matches Elm and satisfies the fundamental rule, so it is unconditional. The
`json_dec_primitives` golden feeds `"3.5"` to `int`; its fixture and expected
output show the rejection explicitly (`"3.5" -> Err _`, printed as `reject`)
rather than a truncated value. The rejection is a typed `Err`, never a panic
(Soundness).

### 2. JSON object key order — keep-ours (divergence)

`Encode.object` emits keys in sorted (lexicographic) order because the runtime
`JsonVal` is `serde_json::Value`, whose `Map` is a `BTreeMap` (the crate is built
without `preserve_order`). Elm preserves the insertion order of the list passed
to `Json.Encode.object`.

Both orders are deterministic. Sorted output additionally matches Go's
`encoding/json` (which sorts map keys) — the oracle the example sweep diffs
byte-for-byte. Switching to insertion order would break Go parity (Correctness)
and require an ordered-map dependency. The higher principle (Correctness against
the oracle) is kept; the divergence is documented. Recorded here as an extension
option, not a bug.

### 3. Float formatting — keep-ours (divergence)

`String.fromFloat` and JSON number rendering follow Go's
`strconv.FormatFloat(f, 'g', -1, 64)` / `encoding/json` floatEncoder shape — the
correctness anchor the sweep diffs against. This agrees with Elm on the common
cases (integral floats drop the fraction; the shortest round-tripping digits are
used) and diverges on two shape details:

- **Exponent padding.** Go pads the exponent to two digits (`1e-07`); Elm's JS
  `String(1e-7)` yields `1e-7`.
- **Negative zero.** Go keeps the sign (`-0`); Elm's JS `String(-0)` collapses to
  `0`.

Go-oracle parity (Correctness) outranks Elm-parity, so both are kept and
documented.

### 4. `null` / `oneOf` / `nullable` — matches Elm

- `nullable` returns `Nothing` on JSON `null`, `Just x` on a value the inner
  decoder accepts, and propagates the inner error on a non-null value that fails
  — Elm's `Json.Decode.nullable` contract exactly.
- `oneOf` returns the first branch that succeeds; an empty branch list or a
  total failure is a typed `Err`, never a panic (Soundness).
- Ipê exposes no public `Decode.null : a -> Decoder a`; the internal
  `json_decode_null` primitive matches only JSON `null`. Surfacing a public
  `null` decoder with Elm's value-returning shape is a completeness item, not a
  behaviour bug — recorded here against the `elm/json` coverage gap.
