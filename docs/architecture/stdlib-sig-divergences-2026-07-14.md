# Stdlib signature-divergence audit — 2026-07-14

Audit of the 30 embedded `.ipe` modules in `crates/ipe/stdlib/` against the
upstream reference (`../sky/sky-stdlib/`) and the Rust kernel registry
(`scripts/ipe-index parity --gaps`).

## Summary

| Category | Count |
|---|---|
| Confirmed signature divergences (wrong kernel / wrong sig) | 1 |
| Missing functions in existing files (upstream has, local lacks) | 11 |
| Local-only extras (local has, upstream lacks) | 1 |
| Cosmetic / import-style differences (no sig divergence) | 1 |
| Kernel gaps for ports | 1 |
| Blocked ports (upstream source missing) | 1 |

---

## 1. Confirmed signature divergences

### 1.1 `Ipe.Random.choice`

| Field | Value |
|---|---|
| Local sig | `choice : List String -> Task Error String` via kernel `Random_choice` |
| Upstream sig | `choice : List a -> Task Error (Maybe a)` via kernel `Random_choiceMaybe` |
| Impact | Wrong type: not polymorphic over element type; returns bare `String` not `Maybe a`. |
| Fix | Replace kernel name with `Random_choiceMaybe`, update sig and exposing list. |
| Kernel parity | `Random_choiceMaybe` parity=ok |

---

## 2. Missing functions in existing files

All kernels for the missing functions have parity=ok in Rust unless noted.

### 2.1 `Ipe.Time` — 7 missing entries

| Missing function | Kernel | Notes |
|---|---|---|
| `format : String -> Int -> String` | `Time_format` | parity=ok |
| `formatHTTP : Int -> String` | `Time_formatHTTP` | parity=ok |
| `formatISO8601 : Int -> String` | `Time_formatISO8601` | parity=ok |
| `formatRFC3339 : Int -> String` | `Time_formatRFC3339` | parity=ok |
| `addMillis : Int -> Int -> Int` | `Time_addMillis` | parity=ok |
| `diffMillis : Int -> Int -> Int` | `Time_diffMillis` | parity=ok |
| `every : Int -> msg -> Sub msg` | `Time_every` | parity=ok |

### 2.2 `Ipe.Random` — 8 missing entries

| Missing function | Kernel | Notes |
|---|---|---|
| `range : Int -> Int -> Task Error Int` | alias for `int` | pure Ipê alias |
| `shuffle : List a -> Task Error (List a)` | `Random_shuffle` | parity=ok |
| `weighted : List (Float, a) -> Task Error (Maybe a)` | `Random_weighted` | parity=ok |
| `type Seed = Seed Int` | n/a | pure Ipê ADT |
| `seed : Int -> Seed` | n/a | pure Ipê ctor |
| `seededInt : Seed -> Int -> Int -> (Int, Seed)` | `Random_seededInt` | parity=ok |
| `seededFloat : Seed -> (Float, Seed)` | `Random_seededFloat` | parity=ok |
| `seededChoice : Seed -> List a -> (Maybe a, Seed)` | `Random_seededChoice` | parity=ok |

### 2.3 `Ipe.System` — 1 missing entry

| Missing function | Kernel | Notes |
|---|---|---|
| `getcwd : () -> Task Error String` | `System_getcwd` | parity=ok; alias for `cwd` |

---

## 3. Local-only extras (local has, upstream lacks)

### 3.1 `Ipe.File.delete`

Local exposes `delete : String -> Task Error ()` via `File_delete`. Upstream uses
only `remove` for deletion and does not expose `delete`. `File_delete` kernel
exists and has parity=ok. Decision: **remove from local** to match upstream surface.

---

## 4. Cosmetic / import-style differences

### 4.1 `Ipe.Ui.Grid`

Local: `import Ipe.Ui as Ui exposing (Attribute)` then calls `Ui.gridTracksRaw`.
Upstream: `import Ipe.Ui exposing (Attribute, gridTracksRaw)` then calls `gridTracksRaw`.
Semantically identical — same `UiGridTracksRaw` kernel. No action required.

---

## 5. Ports — placement decisions and kernel-gap notes

14 modules were requested for porting. `Ipe.Markdown` does not exist in upstream.

### Registration decisions

| Module | Placement | Reason |
|---|---|---|
| `Ipe.Path` | `MODULES` + qualifier entry | Pure Ffi.kernel, no ADTs |
| `Ipe.Regex` | `MODULES` + qualifier entry | Pure Ffi.kernel, no ADTs |
| `Ipe.Pure` | `COMPILED_STD_MODULES` | Pure Ipê wrappers; imports Task/Time/System/Io |
| `Ipe.WebSocket` | `COMPILED_STD_MODULES` | Has 3 own ADTs plus mixed Ffi.kernel and pure Ipê logic |
| `Ipe.Cache` | `COMPILED_STD_MODULES` | Has `type Cache k v = Cache Int` ADT; not in qualifier table |
| `Ipe.Compression` | `COMPILED_STD_MODULES` | Not in qualifier table |
| `Ipe.Config` | `COMPILED_STD_MODULES` | Has `type alias Decoder`; not in qualifier table |
| `Ipe.Csv` | `COMPILED_STD_MODULES` | Has `type alias Csv` plus pure Ipê builders |
| `Ipe.Email` | `COMPILED_STD_MODULES` | Has `type EmailProvider` plus `type alias EmailMessage` |
| `Ipe.Live.Console` | `COMPILED_STD_MODULES` | Pure Ipê; no Ffi.kernel |
| `Ipe.PubSub` | `COMPILED_STD_MODULES` | Not in qualifier table |
| `Ipe.Trace` | `COMPILED_STD_MODULES` | Not in qualifier table |
| `Ipe.Ui.Events` | `COMPILED_STD_MODULES` | Pure Ipê re-exports |

### Kernel gaps for ports

| Module | Gap | Detail |
|---|---|---|
| `Ipe.WebSocket` | `Sub_subscribeWebSocket` rust=missing | parity=ok in Go; Rust runtime missing. `onOpen`/`onMessage`/`onClose`/`onError` will not function at runtime on the Rust backend. Module ported for parse/canon completeness; gap documented. |

### Blocked

| Module | Reason |
|---|---|
| `Ipe.Markdown` | Source file does not exist in `../sky/sky-stdlib/`. Cannot port. |

---

## 6. Full divergence inventory (all files audited)

| File | Status |
|---|---|
| `Ipê/Core/Basics.ipe` | ok |
| `Ipê/Core/Maybe.ipe` | ok |
| `Ipê/Core/Result.ipe` | ok |
| `Ipê/Core/List.ipe` | ok |
| `Ipê/Core/String.ipe` | ok |
| `Ipê/Core/Char.ipe` | ok |
| `Ipê/Core/Dict.ipe` | ok |
| `Ipê/Core/Set.ipe` | ok |
| `Ipê/Core/Bytes.ipe` | ok |
| `Ipê/Core/Crypto.ipe` | ok |
| `Ipê/Core/Task.ipe` | ok |
| `Ipê/Core/Io.ipe` | ok |
| `Ipê/Core/Time.ipe` | DIVERGES — 7 missing functions (section 2.1) |
| `Ipê/Core/System.ipe` | DIVERGES — 1 missing function (section 2.3) |
| `Ipê/Core/Random.ipe` | DIVERGES — 1 wrong sig + 8 missing (sections 1.1 and 2.2) |
| `Sky/Core/File.ipe` | EXTRA — `delete` not in upstream (section 3.1) |
| `Ipê/Core/Http.ipe` | ok |
| `Ipê/Core/Math.ipe` | ok |
| `Ipê/Core/ToString.ipe` | ok (COMPILED_STD_MODULES) |
| `Ipê/Test.ipe` | ok (COMPILED_STD_MODULES) |
| `Std/Css.ipe` | ok (COMPILED_STD_MODULES) |
| `Std/Palette.ipe` | ok (COMPILED_STD_MODULES) |
| `Std/Live/Head.ipe` | ok (COMPILED_STD_MODULES) |
| `Std/Money.ipe` | ok (COMPILED_STD_MODULES) |
| `Std/Ui/Responsive.ipe` | ok (COMPILED_STD_MODULES) |
| `Std/Ui/Chart.ipe` | ok (COMPILED_STD_MODULES) |
| `Std/Ui/Grid.ipe` | COSMETIC — import style differs (section 4.1) |
| `Std/Ui/Transition.ipe` | ok (COMPILED_STD_MODULES) |
| `Std/Ui/Transform.ipe` | ok (COMPILED_STD_MODULES) |
| `Std/Ui/Animation.ipe` | ok (COMPILED_STD_MODULES) |
