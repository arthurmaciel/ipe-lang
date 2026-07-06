# Examples-sweep burndown — integrated blocker map (REMEASURE 2)

> Full 35-example skyc + **cargo-seal** remeasure at master `15dbedb`.
> This is the cargo-verified truth (each example built skyc THEN cargo).

## Verdict tally (cargo-verified)

| verdict | count |
|---|---|
| **SEALED** (skyc-0 ∧ cargo-0) | **19** |
| skyc-FAIL | 10 |
| cargo-FAIL (skyc-0, emit layer) | 5 |
| build-only fixture (26) | 1 |

SEALED: 01 04 09 10 14 17 19 20 21 22 23 30 31 32 33 34 simple-fixtures.
(17/19/31 newly sealed since remeasure-1.)

## Clusters by root mechanism (leverage-ordered)

### R — REGRESSION: poly-tvar quantification (28 ICE, 29 emit) — TOP
- **28** `SKY-I0001`: "generic type variable symbol 49 not in the enclosing
  function's quantification scope; the lowerer must list every structurally-used
  type variable in `Func::type_params`". **28 WAS SEALED** — the batch-any19
  `type_params` filter (drop `any`) over-removed a tvar that IS structurally
  used. Regression introduced by my merge.
- **29** `E0271`: `main_view` returns `Html<()>` but expected `Html<MainMsg>` —
  the #139 Ui-msg→Unit class in a webview view fn #139 didn't cover.
ONE principled fix (reference-faithful to Instantiate/quantification):
`Func::type_params` = every STRUCTURALLY-USED tvar; filter `any` ONLY when it is
genuinely free/unused (not when it names a used position); Ui-msg positions
lower to the enclosing `Generic`. Stop the per-site whack-a-mole.

### S — emit-layer closure/async seal class (02, 06, 18, simple) — cargo-FAIL
- **02, simple** `E0308`: `expected Pin<Box<…>>, found ()` — a Task/async
  discard emitted as `()` where the async block wants a future (TaskSeqSync /
  async-context emit, #140 family).
- **06** `E0308`: `expected FnOnce, found Fn` — a decoder/callback emitted with
  the wrong closure trait (capture-clone / #89 family).
- **18** `E0525`: `Fn` expected, closure is `FnOnce` — db capture; #151 cleared
  skyc but emit still produces FnOnce.

### T — registry singles (25, 00, 37, 12)
- **25** `SKY-I0001`: builtin type `Color` reaches the lowerer with empty home
  and no arm (advanced past Border.widthEach). Add the lowerer arm (or the canon
  fix so `Color` resolves like other UI builtins).
- **00, 37** `SKY-N0004` unknown module (names truncated in log — repro to get
  them; 37 advanced past radioRow, 00 past Jwt.claims).
- **12** `SKY-N0001` (advanced past the Error ctor).

### U — type-inference / feature singles (15, 16, 27, 38, 24) — next rotation
- **15, 27** `T0001` · **16** `N0002` · **38** `L0115` tuple-pattern-not-supported
  (`case … of (a,b) ->` multi-arm — real feature gap) · **24** `L0124`
  `Live.app routes non-empty but Model has no page field` (diagnose: genuine
  user-code requirement = expected-fail, OR our RoutedLive gate mis-fires on a
  Tui example).

## This rotation (3 lanes)
1. **R** — poly-tvar quantification (regression 28 + 29). Type-system core →
   extra gate scrutiny; brief reference-faithful, no per-site patch.
2. **S** — emit closure/async batch (02/06/18/simple).
3. **T** — registry singles (25 Color arm, 00/37 N0004 modules, 12 N0001).

Next rotation: **U** (15/16/27/38/24) + #142 (Access-clone precision).
