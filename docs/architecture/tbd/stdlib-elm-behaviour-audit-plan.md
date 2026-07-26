# Standard-library behaviour audit against Elm semantics — methodology (D.2)

> Backlog item D.2 (Longer-horizon): "Standard-library behaviour audit
> against Elm semantics (JSON key order, integer-decoder strictness,
> float formatting, null/oneOf/nullable)." Plan written 2026-07-10.
> This is an **audit methodology + scope + triage contract**, not a
> pre-baked findings list — the findings are the audit's own output,
> produced by whoever executes this plan. Design-only; no code changed.

## Problem statement

Ipê's stdlib is built to **Go-reference parity** (PRINCIPLES.md §"Match
the reference"); Elm parity has only ever been audited at the
**API-surface** level (`docs/architecture/elm-core-coverage.md` records
*which functions exist*, explicitly not *what they do*). But the project's
stated ambition (README framing,
divergences-from-elm ledger) is Elm-family **semantics**, and several
behaviours are known to sit at three-way corners where Go, Elm, and
"most correct" disagree. `docs/architecture/divergence-policy.md:264-267`
already flags JSON semantics as UNAUDITED. D.2 is the systematic pass
that turns "we think we match Go and haven't checked Elm" into a
per-behaviour, test-pinned verdict ledger.

### Seed corners (verified 2026-07-10 — these calibrate the method, they do not bound the scope)

| Behaviour | Ipê (file:line) | Go reference | Elm | Status |
|---|---|---|---|---|
| JSON object key order | alphabetical — serde_json `BTreeMap` (`src/runtime/rust/src/json.rs:369-371`; no `preserve_order` feature, `runtime/Cargo.toml:9`) | alphabetical (`json.Marshal`, `upstream:runtime-go/rt/stdlib_extra.go:353-368`) | insertion order (JS object kernel) | Ipê≡Go ≠ Elm |
| `Json.Decode.int` strictness | truncates any number: `3.5`→`3`, `1e2`→`100` (`json.rs:383-397`, documented Go-parity) | same truncation (`stdlib_extra.go:529-536`) | rejects non-integral numbers ("Expecting an Int") | Ipê≡Go ≠ Elm; arguably a **correctness** bug in both backends (silent data loss) |
| Float formatting | Go `%v` pinned, exp ≥ 6 threshold, probed vs Go 1.26.2 (#52; `json.rs:90-128` `go_format_f64`) | same by construction | JS shortest-repr (Ryū) | Ipê≡Go ≠ Elm; resolved+recorded (`divergences-from-sky.md` B15) |
| `oneOf` failure reporting | last branch's error only (`json.rs:748-768`) | last error | accumulates ALL branch failures | Ipê≡Go ≠ Elm; Elm's is better DX |
| `nullable` gating | all-inner-fields NULL/absent → `Nothing` (`db.rs:469-500` for Db; JSON variant in `json.rs`) | mirrored (`db_decoder.go:272`) | not yet compared | UNAUDITED |

## Scope

**In scope — the behavioural surface of every module with an Elm
counterpart**, audited *behaviour-by-behaviour*, not function-by-
function. Concretely, per module family:

1. **Json.Encode / Json.Decode / Pipeline** (the four named areas plus:
   `float` encode of NaN/Infinity, `field` vs `at` error paths, `index`
   out-of-range wording, `keyValuePairs` order, `dict` decode order,
   `maybe` vs `nullable` distinction, decoder laziness/short-circuit
   order).
2. **String** — Unicode segmentation corners (`length`/`slice`/
   `reverse`/`toList` on astral + combining chars), `toInt`/`toFloat`
   accepted grammars, `trim` character class, padding with wide chars.
3. **Basics/Math** — integer division/modBy/remainderBy signs at
   negatives, `round` half-behaviour (Elm: half-away? — pin it),
   `^` on negatives, Int overflow behaviour (Elm: JS doubles; Go/Ipê:
   i64 wrap/abort — soundness-relevant), NaN propagation through
   `min`/`max`/`clamp`/`compare`.
4. **List/Dict/Set** — `sortWith` stability, `Dict` iteration order
   (Elm: sorted by key — Ipê Dict is HashMap + sort-on-read,
   `src/runtime/rust/src/dict.rs:15,40-60`; equivalence must be
   *observable-order* equivalence), `==` structural-equality corners
   (functions inside data: Elm throws — Ipê/Go?).
5. **Time/Random** — only *contract* shapes (Elm's `Random` is a pure
   generator; Ipê's is entropy-backed Task — already a filed E-series
   divergence; audit records it as covered, doesn't relitigate).

**Out of scope:** API presence/naming (C.4's job, already matrixed);
effect-model differences already ledgered in
`docs/divergences-from-elm.md` (E/ER/P/U/R/S series); performance.

## Methodology

The audit is **fixture-driven, three-column, and verdict-producing**.
For each behaviour probed:

1. **Write the probe once, as a Ipê program** (plus a runtime-level
   Rust unit test where the behaviour isn't reachable from Ipê syntax).
   Probes live as goldens under `tests/golden/d2_<module>_<behaviour>/`
   following the existing `oracle.meta` format, and as
   `runtime/tests/<module>_elm_audit.rs` following the
   `decimal_parity.rs` hand-verified-oracle pattern
   (`runtime/tests/decimal_parity.rs:1-14`).
2. **Record three answers per probe:**
   - **Ipê** — run the golden (`IPE_E2E=1`).
   - **Go** — the existing cached oracle (`refresh-oracle` tooling).
   - **Elm** — an Elm 0.19.1 oracle. Method: a throwaway
     `elm/json`+`elm/core` project executed under node, one probe
     expression per line printed via `Debug.toString`/encoded output;
     results are **hand-transcribed into the fixture as comments with
     the elm/compiler + package versions pinned** (same discipline as
     decimal_parity's hand-verified Go answers). No Elm toolchain in
     CI — the transcription is the artifact; re-verification is manual
     and rare. Where running Elm is impractical, the kernel JS source
     (`elm/json` `Json.js`, `elm/core` `Basics.js`) is the cited
     authority, with the exact kernel lines quoted in the fixture
     comment.
3. **Classify** each three-way outcome into a verdict, in principle
   order:

   | Verdict | Meaning | Action |
   |---|---|---|
   | `elm-match` | Ipê ≡ Elm (≡ or ≠ Go) | pin with test; if ≠ Go, ensure a `divergences-from-sky.md` entry exists |
   | `go-match` | Ipê ≡ Go ≠ Elm, Go behaviour defensible | pin with test; add entry to `divergences-from-elm.md` with rationale |
   | `defect` | Ipê ≠ both, or Ipê ≡ Go but the shared behaviour loses to a higher principle (e.g. `Decode.int` silent truncation = correctness) | file a BACKLOG.md row (phase: Post-completion or Hardening follow-ups per severity), with the probe as the ready-made regression test |
   | `elm-wart` | Elm's behaviour is itself the broken one (e.g. NaN-keyed Dict) | pin Ipê's better behaviour; entry in `divergences-from-elm.md` tagged as deliberate |

   The default stance is unchanged from PRINCIPLES.md: **Go parity
   wins ties**; Elm wins only where its behaviour is strictly better
   under the principle order AND the change is recorded in
   `divergences-from-sky.md` — an audit verdict is a *filed row*, never
   an in-audit behaviour change.
4. **No silent findings.** Every probe lands one of: a pinning test
   (verdict recorded in the fixture header), a divergence-ledger entry,
   or a BACKLOG row. An audit finding without one of the three
   artifacts does not exist.

## Deliverables (definition of done for D.2)

1. `docs/architecture/stdlib-elm-behaviour-audit-<date>.md` — the
   findings ledger: one row per probe (module, behaviour, three-column
   result, verdict, artifact link). Structure mirrors
   `principles-audit-2026-07-09.md`'s findings table.
2. The probe corpus: `tests/golden/d2_*` goldens +
   `runtime/tests/*_elm_audit.rs` unit fixtures, all green and wired
   into the normal test run (no special CI lane).
3. `docs/divergences-from-elm.md` extended with a new numbered series
   (suggest `B1..Bn`, "behavioural") for every `go-match`/`elm-wart`
   verdict — the existing E/ER/S/P/U/R/STR series stay
   architecture-level.
4. New BACKLOG.md rows for every `defect` verdict, each carrying its
   probe fixture name in Notes.
5. `elm-core-coverage.md` gains a `behaviour-audited` marker (or per-row
   note) for entries the audit covered, so C.4 and D.2 don't double-track.

## Execution notes for the lane that picks this up

- **Batching:** one module family per session (Json first — it has the
  four named areas and the richest seed table). Each batch is
  independently landable; do not hold findings hostage to audit
  completion.
- **The seed table above is calibration**, not conclusions: re-verify
  each seed with a real probe before recording a verdict (the
  `nullable` row in particular is unfinished — Elm's `nullable` vs
  `maybe` distinction needs its own probes).
- **`Decode.int` deserves priority**: silent `3.5`→`3` truncation is a
  correctness-tier candidate defect; if confirmed, its BACKLOG row
  should propose Elm's reject-non-integral gate as a
  `divergences-from-sky.md` candidate (Go parity loses to correctness
  per the principle order — but that decision belongs to the filed
  row's own review, not to this audit).
- **Dict iteration order:** compare *observable* order (Ipê sorts on
  `keys`/`toList` reads) — backing-structure differences that never
  surface are not findings.
- Estimated shape: ~40–60 probes total; Json ≈ 20, String ≈ 12,
  Basics/Math ≈ 12, List/Dict/Set ≈ 10.
