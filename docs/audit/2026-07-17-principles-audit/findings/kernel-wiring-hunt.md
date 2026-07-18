# Kernel-wiring hunt — pure-Ipê vs kernel divergence survey

Audit date: 2026-07-17. Read-only survey — no code changed.

## §0 Method

For each of the 43 `src/stdlib/**/*.ipe` files:

1. Classified every top-level binding as (a) `Ffi.kernel "..."` alias or
   (b) pure-Ipê (recursive/case-based body).
2. For every pure-Ipê binding, queried `scripts/ipe-index locate` and
   `parity --gaps` to detect a corresponding runtime kernel
   (`src/runtime/rust/src/*.rs` and/or Go `../sky/runtime-go/rt/`).
3. For confirmed candidates, diff'd the pure-Ipê body against the kernel
   for dropped guards, edge-case divergence, overflow paths.

`ipe-index` was the primary tool; `rg` used only for free-text cross-checks
the index cannot answer.

---

## §1 Confirmed findings

### F-001 — Money: 8 kernel-backed functions replaced by pure-Ipê stubs (CONFIRMED, HIGH)

**File:** `src/stdlib/Ipe/Money.ipe` (entire file — header comment says explicitly "all `Ffi.callPure` calls replaced with pure Sky implementations")

**Kernel counterparts confirmed via `ipe-index locate`:**

| stdlib fn | kernel route | Rust impl |
|---|---|---|
| `minorUnits` | `Money.minorUnits` parity=ok | `src/runtime/rust/src/money.rs:100` |
| `symbol` | `Money.symbol` parity=ok | `money.rs:107` |
| `currencyName` | `Money.currencyName` parity=ok | `money.rs:115` |
| `isKnownCode` | `Money.isKnownCurrency` (no `locate` hit → not registered in ipe compiler) | `money.rs` (kernel exists) |
| `format` | `Money.format` parity=ok | `money.rs:130` |
| `formatWithCode` | `Money.formatWithCode` parity=ok | `money.rs:152` |
| `setRate` / `getRate` / `hasRate` / `clearRates` | parity=ok | `money.rs:175–260` |
| `allocate` | `Money.allocate` parity=ok | `money.rs:271` |

**Current situation:** The compiler has NO `Money_*` kernel string entries (confirmed: `rg '"Money_'` across `src/compiler/` returns empty). The Rust runtime has full kernel implementations but the compiler never routes to them. `Money.ipe` pure-Ipê implementations are what actually runs.

**Divergences (confirmed vs kernel body):**

**F-001a — `allocate`: negative-total residue drops pennies (CO-INCR-001 class)**
- Pure-Ipê `Money.ipe:441–472`: `step = if remainder < 0 then -1 else 1` — correct sign-aware distribution.
- This is actually the FIXED form (the kernel `money.rs:271` comments document a past bug where `.max(0)` dropped negative residue; the pure-Ipê got the fix ported correctly in this specific guard).
- **However**: pure-Ipê performs arithmetic via `Dec.toMinor` then integer `//` (Ipê integer division). If `Dec.toMinor` returns a value that rounds differently than the kernel's `amount.0.checked_mul(scale).trunc()` the per-slot values will diverge. **Severity: MEDIUM** — depends on Decimal.toMinor rounding contract matching the kernel's truncate.

**F-001b — `allocate`: no OOM/amplification guard (CO-INCR-002)**
- Pure-Ipê `Money.ipe:441`: `if parts <= 0 then [] else ...` — only guards the non-positive case.
- Kernel `money.rs:309–311`: `if parts > 100_000 { return Vec::new(); }` — caps at 100k parts to prevent memory-amplification DoS from caller-controlled `parts`.
- **Pure-Ipê has no such cap.** A caller passing `parts = 10_000_000` (a valid `Int` from a request) will call `allocateHelp` recursively 10M times. On the Rust runtime this exhausts the call stack (stack overflow → process abort, since the Ipê recursion is not TCO'd at the Rust level).
- **Severity: HIGH** — reachable from any request-derived `Int` argument. DoS via stack overflow.
- **Reachability: HIGH** — `allocate` is a public API, `parts` is caller-controlled.

**F-001c — `format` / `formatWithCode`: rounding strategy divergence**
- Pure-Ipê `Money.ipe:575–583`: delegates to `Dec.toStringFixed (minorUnits c)` then string-concatenates symbol.
- Kernel `money.rs:130–168`: pre-rounds with `RoundingStrategy::MidpointAwayFromZero` before formatting, with explicit test `"2.545" USD → "$2.55"` (half-away-from-zero, Go shopspring parity). `rust_decimal`'s `{:.*}` TRUNCATES when precision < scale without the pre-round.
- **Pure-Ipê path relies on `Dec.toStringFixed`'s rounding behaviour.** If `Ipe.Decimal.toStringFixed` truncates rather than rounds-half-away (matching Go's `StringFixed`), formatted outputs will diverge: `"$2.54"` instead of `"$2.55"`.
- **Severity: MEDIUM** — correctness divergence for amounts with sub-cent precision (tax, crypto). Reachable whenever `Dec.mul` or an imported decimal has more decimal places than the currency's `minorUnits`.

**F-001d — `minorUnits`: BTC hardcoded as 8 dp vs kernel's 8 dp — MATCHES**
- Pure-Ipê `Money.ipe:167–178` only enumerates JPY/KRW/VND/CLP/IDR (0 dp) and BHD/KWD/OMR/JOD (3 dp), wildcard `_ -> 2`. BTC/ETH/USDT/USDC all fall through to 2.
- Kernel `money.rs:100–105` also falls through to `2` for unknown codes (including BTC/ETH/USDT/USDC — only "USD"→"RUB"/"UAH"/"BTC" listed, BTC=8 there).
- **CONFIRMED DIVERGENCE**: kernel `money.rs` has `"BTC" => (8, ...)` (8 decimal places). Pure-Ipê wildcard gives BTC 2 decimal places.
- Go kernel also has `"BTC": {8, ...}`, `"ETH": {18, ...}`, `"USDT": {6, ...}`, `"USDC": {6, ...}`.
- Pure-Ipê treats BTC/ETH/USDT/USDC as 2 dp instead of 8/18/6/6. This means `fromMinor BTC 1` gives `0.01` BTC instead of `0.00000001` BTC — a **1 million-fold magnitude error**.
- **Severity: CRITICAL** — crypto amounts are silently mis-scaled. Any BTC/ETH/USDT/USDC `fromMinor` call returns the wrong value.
- **Reachability: HIGH** — any user importing `Ipe.Money` with BTC/ETH/USDT/USDC.

**F-001e — `symbol` / `currencyName`: CHF symbol divergence**
- Pure-Ipê `Money.ipe:189`: CHF → `"Fr"`. Kernel `money.rs:107` and Go kernel: CHF → `"Fr."` (with period).
- **Severity: LOW** — cosmetic display difference.

**F-001f — `setRate` / `getRate` / `hasRate` / `clearRates`: always-Err stubs**
- Pure-Ipê `Money.ipe:594–611`: all four return `Err (Error.unexpected "FX registry not available")` / `False`.
- Kernel `money.rs:175–260`: full process-global FX registry with mutex, MAX_RATES=4096 guard, auto-inverse, MAX_CODE_LEN=16 guard.
- **These are NOT divergences — they are stubs.** The FX registry is intentionally disabled in the pure-Ipê port. The risk is: any user calling `Money.setRate` / `getRate` / `convert` will get `Err` silently, with no compile-time warning. `Money.convert` (pure-Ipê `Money.ipe:614–626`) delegates to `getRate` so always returns `Err` for cross-currency conversions.
- **Severity: MEDIUM** — silent behavioral stub masking a missing feature. Not a correctness bug in isolation, but `convert` is documented as working and silently fails.

---

### F-002 — List: pure-Ipê reimplementations shadow kernel-routed variants (CONFIRMED, MEDIUM)

**File:** `src/stdlib/Ipe/List.ipe`

**Situation:** `List.ipe` contains pure-Ipê implementations of `isEmpty`, `length`, `head`, `tail`, `reverse`, `take`, `drop`, `append`, `concat`, `member`, `range`, `zip`, `map`, `filter`, `any`, `all`, `find`, `foldl`, `foldr`, `concatMap`, `indexedMap`. Every one of these has a confirmed Rust kernel (`list.rs`) with `parity=ok`.

**Key question — which path does the compiler actually use?**
The `ipe-index parity=ok` verdict for all List kernels means the compiler registers and routes calls to the kernel, NOT the pure-Ipê body. `List.ipe` bodies are stdlib source declarations that give the functions HM type signatures; the emitter routes to `list_*` kernels. This is the intended architecture (the `List.ipe` header comment says HOFs "stay kernel-anchored" but the non-HOF surface was migrated to Sky source).

**However, two divergences exist between the pure-Ipê bodies and kernel bodies that matter if the compiler ever routes to the pure-Ipê path (e.g. in-lined expansion, fallback, or future pure-Ipê mode):**

**F-002a — `List.range`: no allocation cap in pure-Ipê**
- Pure-Ipê `List.ipe:481–495`: `rangeHelp lo hi []` — no bound on span.
- Kernel `list.rs:131–156`: `const CAP: usize = 10_000_000` — emits first 10M elements + warning on overflow.
- **If the pure-Ipê body is ever used** (e.g. in a `--target wasm` build that uses stdlib source directly, or a future pure-Ipê eval mode), `List.range 0 1_000_000_000` will attempt to build a 1-billion-element list, OOMing the process.
- **Severity: LOW (kernel-routed today)** — currently the kernel is used; becomes HIGH if pure-Ipê path is ever activated (WASM target, pure evaluation, etc.)

**F-002b — `List.foldr` semantics match confirmed**
- Pure-Ipê delegates: `foldr fn acc list = foldl (\x a -> fn x a) acc (reverseHelp list [])`.
- Kernel `list.rs:121–129`: `for item in list.into_iter().rev() { acc = f(item, acc); }`.
- Both traverse right-to-left. **No divergence.**

---

### F-003 — Basics.ipe: pure-Ipê implementations of kernel-backed functions (CONFIRMED, LOW)

**File:** `src/stdlib/Ipe/Basics.ipe`

Pure-Ipê implements: `identity`, `always`, `not`, `fst`, `snd`, `clamp`.
All five have confirmed Rust kernels (`basics.rs:60–95`) with `parity=ok`.

**Analysis:** These are structurally correct pure-Ipê implementations of trivial functions (identity, not, fst/snd, clamp). The kernels and pure-Ipê agree on semantics. The compiler routes to the kernel, so no runtime divergence today.

**F-003a — `clamp` behavior on `lo > hi`:**
- Pure-Ipê `Basics.ipe:24–30`: `if x < lo then lo else if x > hi then hi else x`. When `lo > hi`, returns `lo` for any `x < lo`, `hi` for any `x > hi`, and `x` in the impossible overlap.
- Kernel `basics.rs:95–104`: identical logic — `if x < lo { lo } else if x > hi { hi } else { x }`. Same behavior when `lo > hi`.
- **No divergence.** Both match Elm's semantics.

**Severity: NONE** — pure-Ipê bodies are correct and kernel-routed anyway.

---

### F-004 — Maybe.ipe / Result.ipe: pure-Ipê, legitimately kernel-free (CONFIRMED SAFE)

**Files:** `src/stdlib/Ipe/Maybe.ipe`, `src/stdlib/Ipe/Result.ipe`

`ipe-index` shows `Maybe.map`, `Maybe.andThen`, `Result.map`, `Result.andThen`, `Result.mapError` as `parity=go-kernel-opt` with `rust=<missing>`. This means: the Go backend inlines these as codegen optimizations (no named runtime function). No Rust kernel exists or is expected.

The pure-Ipê implementations (`withDefault`, `map`, `andThen`, `mapError`, `map2`–`map5`, `andMap`, `combine`, `isJust`, `isNothing`) are the intended implementation on both backends. **No kernel to diverge from.**

**Severity: NONE** — legitimately kernel-free.

---

### F-005 — ToString.ipe: pure-Ipê wrappers, thin aliases — low risk (CONFIRMED, LOW)

**File:** `src/stdlib/Ipe/ToString.ipe`

`fromInt` / `fromFloat` delegate to `String.fromInt` / `String.fromFloat` (kernel-routed). `fromBool` is a pure-Ipê case expression (`True -> "True"`, `False -> "False"`).

**F-005a — `fromBool` capitalisation vs Go:**
- Pure-Ipê: `True -> "True"`, `False -> "False"` (capitalised).
- Go's `fmt.Sprintf("%v", true)` yields `"true"` (lowercase). Elm's `Debug.toString True` yields `"True"`.
- No Rust kernel for `fromBool`; this is the intended form (Ipê/Elm style, not Go/%v style).
- **Severity: LOW** — cosmetic; documented divergence from Go's `%v` convention is intentional per Elm alignment.

---

## §2 Coverage — modules swept

All 43 `src/stdlib/**/*.ipe` files surveyed:

| Module | Classification | Finding |
|---|---|---|
| `Ipe.Basics` | Mixed: pure-Ipê for `identity/always/not/fst/snd/clamp`; kernel for `modBy` | F-003 (no divergence, kernel-routed) |
| `Ipe.Bytes` | All `Ffi.kernel` | None |
| `Ipe.Cache` | Mixed: type/builder pure-Ipê, operations `Ffi.kernel` | None |
| `Ipe.Char` | All `Ffi.kernel` | None |
| `Ipe.Compression` | All `Ffi.kernel` | None |
| `Ipe.Config` | All `Ffi.kernel` | None |
| `Ipe.Crypto` | All `Ffi.kernel` | None |
| `Ipe.Css` | All pure-Ipê (typed DSL) | No kernel counterpart — legitimate |
| `Ipe.Csv` | All `Ffi.kernel` | None |
| `Ipe.Dict` | All `Ffi.kernel` | None |
| `Ipe.Email` | Mixed: builder pure-Ipê, `send` kernel | None |
| `Ipe.File` | All `Ffi.kernel` | None |
| `Ipe.Http` | Mixed: builder pure-Ipê, effect kernels | None |
| `Ipe.Io` | All `Ffi.kernel` | None |
| `Ipe.List` | All pure-Ipê BUT all have kernel counterparts; compiler routes to kernel | F-002 (range cap divergence, low risk today) |
| `Ipe.Live.Console` | Type alias | None |
| `Ipe.Live.Head` | Pure-Ipê builder DSL | No kernel counterpart — legitimate |
| `Ipe.Math` | All `Ffi.kernel` | None |
| `Ipe.Maybe` | All pure-Ipê; `go-kernel-opt` — no Rust kernel exists | F-004 (safe) |
| `Ipe.Money` | All pure-Ipê; ALL have Rust+Go kernels | **F-001 (CRITICAL + HIGH + MEDIUM)** |
| `Ipe.Palette` | Pure-Ipê spike module | No kernel — legitimate |
| `Ipe.Path` | All `Ffi.kernel` | None |
| `Ipe.PubSub` | All `Ffi.kernel` | None |
| `Ipe.Pure` | Pure-Ipê wrappers delegating to kernels | None |
| `Ipe.Random` | Mixed: effect kernels + thin Seed wrappers pure-Ipê | None — wrappers correct |
| `Ipe.Regex` | All `Ffi.kernel` | None |
| `Ipe.Result` | All pure-Ipê; `go-kernel-opt` — no Rust kernel exists | F-004 (safe) |
| `Ipe.Set` | All `Ffi.kernel` | None |
| `Ipe.String` | All `Ffi.kernel` | None |
| `Ipe.System` | All `Ffi.kernel` | None |
| `Ipe.Task` | All `Ffi.kernel` | None |
| `Ipe.Test` | Mixed: 1 kernel + pure-Ipê helpers | No divergence found |
| `Ipe.Time` | All `Ffi.kernel` | None |
| `Ipe.ToString` | Pure-Ipê wrappers | F-005 (low, intentional) |
| `Ipe.Trace` | All `Ffi.kernel` | None |
| `Ipe.Ui.Animation` | All pure-Ipê (typed DSL) | No kernel — legitimate |
| `Ipe.Ui.Chart` | All pure-Ipê (typed DSL) | No kernel — legitimate |
| `Ipe.Ui.Events` | All pure-Ipê (typed DSL) | No kernel — legitimate |
| `Ipe.Ui.Grid` | All pure-Ipê (typed DSL) | No kernel — legitimate |
| `Ipe.Ui.Responsive` | All pure-Ipê (typed DSL) | No kernel — legitimate |
| `Ipe.Ui.Transform` | All pure-Ipê (typed DSL) | No kernel — legitimate |
| `Ipe.Ui.Transition` | All pure-Ipê (typed DSL) | No kernel — legitimate |
| `Ipe.WebSocket` | Mixed: type/builder pure-Ipê, ops kernel | None |

---

## §3 Summary — ranked findings

| ID | Severity | Module | Specific divergence | Reachability |
|---|---|---|---|---|
| F-001d | **CRITICAL** | `Ipe.Money` | BTC=2dp instead of 8dp; ETH=2dp instead of 18dp; USDT/USDC=2dp instead of 6dp — `fromMinor` produces wrong values | Any `BTC`/`ETH`/`USDT`/`USDC` `fromMinor` call |
| F-001b | **HIGH** | `Ipe.Money` | `allocate`: no 100k-parts cap → stack overflow DoS on large `parts` | Any request-controlled `parts` arg |
| F-001c | **MEDIUM** | `Ipe.Money` | `format`/`formatWithCode`: rounding strategy depends on `Dec.toStringFixed` — may truncate instead of round-half-away | Any amount with sub-minor-unit decimal places |
| F-001a | **MEDIUM** | `Ipe.Money` | `allocate`: pure-Ipê integer path vs kernel's `checked_mul/div` — overflow behavior on very large amounts | Large amounts + large `parts` |
| F-001f | **MEDIUM** | `Ipe.Money` | `setRate`/`getRate`/`convert`: always-Err stubs — FX registry silently disabled | Any FX conversion call |
| F-001e | **LOW** | `Ipe.Money` | CHF symbol: `"Fr"` vs `"Fr."` | CHF format calls |
| F-002a | **LOW** (HIGH if WASM/pure-Ipê path activated) | `Ipe.List` | `range`: no allocation cap in pure-Ipê body | If pure-Ipê path used (WASM target, pure eval) |
| F-003 | **NONE** | `Ipe.Basics` | `identity/always/not/fst/snd/clamp`: pure-Ipê bodies correct + kernel-routed | N/A |
| F-004 | **NONE** | `Ipe.Maybe`, `Ipe.Result` | `go-kernel-opt` — no Rust kernel exists; pure-Ipê is intended | N/A |
| F-005 | **LOW** | `Ipe.ToString` | `fromBool`: capitalised `"True"`/`"False"` vs Go `%v` lowercase — intentional Elm alignment | Display only |

**Total candidates found: 5 modules with pure-Ipê bodies shadowing kernels.**
**True divergences: F-001d (CRITICAL), F-001b (HIGH), F-001c (MEDIUM), F-001f (MEDIUM).**
**Legitimately kernel-free (no divergence risk): 38 modules.**

---

## §4 Fix directions

**F-001 (Money):** The root fix is to register `Money_*` kernel names in the compiler's kernel table (`src/compiler/kernels/src/lib.rs`) and route the pure-Ipê stdlib bindings to `Ffi.kernel "Money_minorUnits"` etc. — exactly the pattern the upstream `../sky/sky-stdlib/Std/Money.sky` uses. The Rust runtime kernels are already fully implemented in `src/runtime/rust/src/money.rs`. This is a compiler wiring task, not a runtime task.

Short-term mitigations for the pure-Ipê path (in case kernel wiring is deferred):
- **F-001d**: Add BTC→8, ETH→18, USDT→6, USDC→6 to `minorUnits` case in `Money.ipe:167`.
- **F-001b**: Add `if parts > 100000 then []` guard before `allocateHelp` call in `Money.ipe:443`.
- **F-001e**: Fix CHF symbol to `"Fr."`.

**F-002a (List.range):** Add `if (hi - lo) > 10_000_000 then []` (or emit a structured warning) at the top of the pure-Ipê `range` body for defensive parity with the kernel, so the WASM / pure-eval path can't OOM.
