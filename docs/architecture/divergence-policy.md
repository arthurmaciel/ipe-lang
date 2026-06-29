# Divergence policy — when Sky-Rust output may differ from the Go reference

> Status: IMPLEMENTED (M4b; extended M4c). The oracle crate (`tools/oracle`) +
> the `refresh-oracle` tool encode this policy. M4c added the tagged
> Go-succeeds-but-we-differ marker (`divergence:` / `sanctioned:`); the first
> `divergence:`-tagged divergence is Math.min/max #136 (see registry below).

## Default — byte parity with Go

Sky-Rust's contract (PRINCIPLES.md §2 Correctness) is that for the same
well-typed Sky program and the same input, the Rust output matches the Go
reference's observable behaviour, **ideally byte-for-byte**. The golden parity
suite enforces this: each runnable golden caches the Go reference's clean
program stdout (`expected_go.txt` + `oracle.meta`), and `oracle::check_parity`
diffs skyc's output against it on every run — no live Go in the hot path.

So the DEFAULT for every golden is: skyc must reproduce the Go bytes exactly.
A mismatch is a hard failure.

## The three kinds of recorded divergence

A divergence is **never silent**. PRINCIPLES.md §2 allows a deliberate
divergence only when it is *documented rather than silent*. The oracle
records exactly three kinds, all via `oracle_divergence = true` in `oracle.meta`,
distinguished by the leading TAG on the `divergence_reason`:

### 1. Auto Go-failure divergence (Go FAILS)

The Go oracle panics, exits non-zero, or fails to build on a shape skyc handles
correctly. The `refresh-oracle` tool detects the Go failure, falls back to
skyc's own (correct) output, and records it with a reason such as
`Go oracle failed: …`. This is the original "the Go oracle can fail to produce a
reference on a shape" carve-out. No marker file is needed — the failure itself
triggers the branch.

### 2. `divergence:` kind (Sky's current behaviour differs; we follow a different target)

The Go oracle *succeeds* — it builds and runs — but Sky's current behaviour
differs from what Sky-Rust implements. Because Go does not fail, the auto path
(kind 1) never fires; the golden must opt in explicitly. Sky-Rust records its
own output with a neutral rationale stating what differs and why (e.g.
Elm-conformance, fuller Unicode).

- Drop a `sanctioned.divergence` marker file whose reason begins with the
  `divergence: ` tag (e.g. `divergence: Math.min/max Elm-conformant polymorphic
  comparable. Divergence from Sky, rationale: Elm-conformance`).
- `refresh-oracle` short-circuits straight to skyc's output (it does **not**
  require, or even run, the Go oracle for the expected value) and records it with
  `oracle_divergence = true` and `divergence_reason = divergence: <reason>`.

### 3. `sanctioned:` divergence (Go SUCCEEDS, Sky-Rust is deliberately MORE correct)

The Go oracle succeeds *correctly*, but Sky-Rust is intentionally more correct
still, so the two outputs differ by design (e.g. full-Unicode case mapping).
This is a **deliberate, reviewed** choice — not a bug on either side.

- Drop a `sanctioned.divergence` marker file; its contents are the human-readable
  reason. An untagged reason defaults to the `sanctioned: ` tag (so historical
  markers keep working); you may write `sanctioned: …` explicitly.
- `refresh-oracle` short-circuits straight to skyc's output and records it with
  `oracle_divergence = true` and `divergence_reason = sanctioned: <reason>`.

In all three kinds the staleness gate (`sha256(Main.sky)`) and `check_parity`
apply unchanged — the recorded expectation is still pinned to the source and
diffed exactly. The leading tag (`Go oracle failed:` / `divergence:` /
`sanctioned:`) is what lets the read side, the refresh logs, and a human
reviewer tell the kinds apart without re-running anything. A blank marker is a
hard error: a marker divergence MUST state why.

This keeps the rigour floor intact: a sanctioned divergence is loud, reviewed,
and source-pinned — it can never silently mask a *real* parity bug, because the
expected bytes are Sky-Rust's deliberate output, captured and committed.

## The sanctioned divergences in M4b

M4b ships the `Sky.Core.String` / `Sky.Core.Char` kernels. Char **predicates**
(`Char.isDigit` / `isLower` / `isUpper` / `isAlpha`) are NOT divergences — they
match Go exactly (`unicode.IsDigit`/`Nd`, `IsLower`/`Ll`, `IsUpper`/`Lu`,
`IsLetter`/`L*`); see FIX-3. The only sanctioned divergences are:

1. **Full-Unicode default case mapping** —
   `String.toUpper` / `toLower` / `casefold` and `Char.toUpper` / `toLower`.
   Rust applies the full Unicode `SpecialCasing` rules (e.g. `ß → SS`,
   `İ → i̇`), where Go's `strings`/`unicode` simple per-rune mapping does not.
   Sky-Rust is deliberately more correct here. Reason recorded:
   `sanctioned: full-Unicode default case mapping (Rust SpecialCasing — ß→SS, İ→i̇ — vs Go simple per-rune)`.

2. **`String.toFloat` standard grammar** — a minor sanctioned-stricter
   divergence: Sky-Rust accepts the standard float grammar and *rejects* Go's
   hex-float and underscore-separated literals. Stricter, not looser; recorded as
   sanctioned where it surfaces.

## Recorded `divergence:` entries (Sky's current behaviour differs)

- **Math.min / Math.max — AsInt coercion vs Elm-conformant polymorphic compare
  (Sky PR #136).** Sky's `Math.min`/`Math.max` currently route both arguments
  through `AsInt` before the compare, coercing `Float` to `Int` (`Math.min 0.4
  1.3 → 0`) and yielding a meaningless compare for `String`. Sky-Rust follows
  Elm's `Basics` polymorphic comparable (`min`/`max : a -> a -> a`) preserving
  each argument's type + value. Divergence from Sky, rationale: Elm-conformance.
  Recorded as `divergence: Math.min/max Elm-conformant polymorphic comparable
  (Divergence from Sky, rationale: Elm-conformance)`. (`Math.abs` stays
  `Int -> Int` — the `AsInt` path is correct there and is not a divergence.)

## How to add a marker divergence (`divergence:` or `sanctioned:`)

1. Decide the tag. `sanctioned:` — Sky-Rust is genuinely more correct
   (PRINCIPLES §2). `divergence:` — Sky's current behaviour differs; Sky-Rust
   follows a different target (e.g. Elm-conformance, fuller Unicode). State
   the difference and the rationale neutrally. If in doubt, fix the parity bug
   instead.
2. Add `tests/golden/<name>/sanctioned.divergence` with a one-line reason,
   prefixed with the chosen tag (`divergence: …` / `sanctioned: …`). An untagged
   reason defaults to `sanctioned: `.
3. Run `refresh-oracle <name>` (needs `SKY_RUNTIME_DIR` pointed at the Rust
   runtime). It captures Sky-Rust's output as the expected and writes
   `oracle_divergence = true` + `divergence_reason = <tag> <reason>` — WITHOUT
   requiring Go to fail.
4. Commit `Main.sky`, `expected_go.txt`, `oracle.meta`, and
   `sanctioned.divergence` together. For a `divergence:` entry, also add a
   one-line row to the registry above documenting the difference and rationale.

## Decision: full-Unicode case mapping is intended — deferrals to post-M6

The case-mapping divergence above (`String.toUpper`/`toLower`/`casefold`,
`Char.toUpper`/`toLower`) is a **settled, deliberate decision**, not a bug to
chase down: full-Unicode case mapping is *free* in Rust, strictly more correct
than Go's simple per-rune mapping, and matches the mainstream
(Rust/Python/Swift/`elm-string`/Haskell `Text`). Correctness wins over byte
parity here (PRINCIPLES.md §2). We keep the case kernels full-Unicode and do
**not** "fix them toward Go".

The following are **explicitly DEFERRED to post-M6** (out of scope now — for
velocity, not because they are unimportant):

- **Locale-correct case mapping** (e.g. Turkish dotless-ı / dotted-İ, Greek
  final sigma) — Rust's default `to_uppercase`/`to_lowercase` are locale-
  independent; locale tailoring would need ICU-style data we are not pulling in.
- **Formal oracle-divergence recording for non-ASCII case** — the sanctioned-
  divergence machinery (marker file + recorded reason) exists, but we have NOT
  authored the non-ASCII case goldens (`ß → SS`, `İ → i̇`, …) that would
  exercise it. Doing so is post-M6 work.
- **Non-ASCII case goldens** themselves — only ASCII case parity
  (`toUpper "hi" == "HI"`) is pinned in the M4b suite.

**Unicode lives in the CORE — permanently.** A Roc-style "Unicode as an
upgradable / opt-in package" model is explicitly **rejected** (user directive):
full Unicode is a built-in commitment of the core runtime, and every divergence
and decision is documented *here, in detail*, rather than relocated behind a
separate package. The deferrals above are *velocity* deferrals — more goldens
and locale tailoring to add later, **in core** — never a move of Unicode out of
core.

Predicates (`Char.isDigit`/`isLower`/`isUpper`/`isAlpha`), `String.split`, and
`String.toInt`/`toFloat` are NOT in this carve-out: they are pure Go-parity and
are fixed to match the Go reference's observable behaviour exactly (e.g.
`String.toInt " 42 " == Nothing` — Go's emitted `String_toInt` does not trim,
which also matches Elm).
