# Examples-sweep burndown — integrated blocker map

> Generated from a full 35-example first-blocker skyc sweep at master
> (post-#148, snapshot 19/35 cargo-sealed). Replaces the ad-hoc
> peel-the-onion loop: blockers are clustered by ROOT MECHANISM and
> ordered by leverage (examples-unblocked-per-fix), so lanes fix whole
> classes and the critical path is visible.
>
> **skyc-OK ≠ cargo-sealed.** This map is skyc-front only; a skyc-OK
> example may still have an emit-layer (cargo) blocker — those are the
> #139/#140/#142 seal classes, tracked separately.

## Raw sweep (skyc first-blocker)

| verdict | count | examples |
|---|---|---|
| skyc-OK | 20 | 01 04 06 09 10 14 17 20 21 22 23 28 30 32 33 34 + simple/spikes/test_pkg |
| SKY-L0108 | 4 | 02 19 24 25 |
| SKY-T0001 | 3 | 29 31 38 |
| SKY-N0005 | 3 | 00 27 37 |
| SKY-N0002 | 2 | 15 16 |
| SKY-N0003 | 1 | 12 |
| SKY-L0126 | 1 | 18 |
| build-only fixture | 1 | 26 (NO_TOML — expected per CLAUDE.md) |

## Clusters by mechanism (leverage-ordered)

### A — `Ui.layout` Html-vs-Element T0001 (leverage 3) — LANE RUNNING (#150)
`expected a, found Html <Msg>` at every `Ui.layout [] (viewX model)`.
ONE root cause blocks **29, 31, 38**. #150 targets 38; the fix is a
scheme/member-sig correction that cascades to 29 + 31 (identical error).
→ Verify #150's fix clears all three; do not re-lane 29/31 separately.

### B — kernel-implicit Prelude types as annotations, N0002 (leverage 2 + REGRESSION) — TOP PRIORITY, un-lane'd
- **15** `handleHome : Handler` (Main.sky:27)
- **16** `viewHome : Model -> (Html Msg)` (Page/Home.sky:15)
`Handler` / `Html` are kernel-implicit Prelude types (the #576 list of
15). #138's total type-name resolution (`canonicalise_type` →
`TypeNotFound`) rejects them because its builtin allowlist
(`RESERVED_BUILTIN_TYPES` ∪ `EXTRA_BUILTIN_TYPE_NAMES`) does NOT include
the kernel-implicit set. **This is a #138 completeness gap** — likely a
regression (16 passed in #138's own before/after matrix, but on a
different file). Fix: add the 15 kernel-implicit types to the
`canonicalise_type` builtin allowlist. One fix, +2, closes the
regression. **Slot first.**

### C — L0108 registry-gap members (leverage up to 4) — un-lane'd
Four DISTINCT members, one wiring mechanism (5-layer registry path each):
- **02** `errorToString` (Prelude, unqualified) — Main.sky:34
- **19** `Font.color` — View/Common.sky:47
- **24** `Ui.link` — Main.sky:432
- **25** `Border.widthEach` — View.sky:81
Batch as ONE lane; each advances past L0108 (may reveal a next blocker).

### D — N0005 registry members (leverage 1 each)
- **27** `Cmd.publish` — LANE RUNNING (pub/sub trio)
- **37** `Input.radioRow` — LANE RUNNING (#150)
- **00** `Jwt.claims` — un-lane'd, single

### E — N0003 ctor registration (leverage 1) — un-lane'd
- **12-skyvote** — `Err`/`Errored` ctor not registered. Single.

### F — L0126 Db lambda-capture (leverage 1) — LANE RUNNING (#149)
- **18-job-queue** — `\ts -> insertRow db ts`. #149 in flight.

## Scheduled burndown (not discovered — planned)

**Running now** clears: A (29/31/38), D-27, D-37, F-18 → +6 skyc-fronts.

**Next rotation (3 lanes), leverage-ordered:**
1. **Cluster B** — kernel-implicit-type allowlist (#138 completeness) → 15, 16. TOP (regression).
2. **Cluster C** — L0108 four-member batch → 02, 19, 24, 25.
3. **Singles lane** — D-00 (Jwt.claims) + E-12 (skyvote ctor).

After both rotations, every skyc-front is cleared; remaining work is the
**emit/cargo-seal layer** (skyc-OK-but-cargo-fail) — the #139/#140/#142
Access-clone family + any per-example residuals surfaced by cargo. Those
get a dedicated seal sweep (cargo-build every skyc-OK example, bucket the
E-class errors) once the skyc front is green.

## Invariants for the burndown
- Every lane fixes a CLASS; before merging, grep the corpus for sibling
  instances of the same mechanism (the serde-derive class hit ChunkEvent
  then SkyStreamId one-at-a-time — avoid that).
- `symbol_resolution` drift test is the safety net for "emitted a call to
  a runtime fn that doesn't exist" (caught the slider + paragraph misses).
- `decl_equiv_legacy_match` catches the legacy-dispatch-arm miss (caught
  slider). New kernel = wire BOTH the id-path and the legacy arm.
