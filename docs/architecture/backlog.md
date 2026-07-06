# Backlog — open work items

> Durable replacement for the in-session task board (cleared 2026-07-06 to stop
> per-turn re-injection cost). This is the authoritative list of OPEN work.
> Read on demand; update as items land. Soundness/security/correctness outrank
> everything — never close an item with a workaround (per CLAUDE.md §4
> no-deferral + §7 root-cause-only). Related: `sweep-burndown-map.md`,
> memory `roadmap-tiers` / `security-hardening-before-push` / `endgame-example-sweep`.

## Sweep front — current per-example first blockers (as of master 5b11260, 2026-07-06)
> Landed this session: #154 (misplaced-span kernels wired), batch-U (ex00 Money/ex15 Handler/ex27 Db-scheme), batch-V (ex00 Time kernels, ex15 Request-fields+Handler-alias — ex15 now build-proven end-to-end).
- **ex37 `Border.shadow`** ✅ **LANDED** (commit `416fd4f`) — `BorderShadow` kernel wired across all 8 sites via `AttrStyle` box-shadow; autonomously produced by a `progressive-development --once` run, gate-green, cleared the L0108. (ex37 now hits the inference cluster below.)
- **inference cluster (ex00/ex12/ex37) — proved to be TWO root causes, not one** (Opus guardian, 2026-07-06):
  - **#1 integer-literal monomorphization — ✅ FIXED & gate-verified** (merged). Integer literals were pinned to concrete `Int`; now `Super{Number}`-polymorphic (pin to Int/Float from context, default Int; Float literals stay concrete per Elm). `unify.rs` rejects `Super{flex}` vs `Super{rigid}` so the parametric-generic gate holds (`f : a -> a; f x = x + 1` stays a type error; regression `literal_added_to_parametric_skolem_is_rejected`). Cleared ex12's `SKY-T0001`, which was a **span mis-attribution** (real site `Styles.sky:188` `width (pct 100)`).
  - **#2 untyped top-level binding shares ONE monomorphic var across the linked program — 🔴 OPEN, guardian (multi-session).** `constrain.rs` `untyped` map returns the *same* `VarId` for each `VarTopLevel` ref instead of a re-instantiated scheme → an obligation from module B (`.message` from `AuthHandlers.sky`; `Dict String String` form-data from a `View/*` module) leaks into a use in module A. Surfaces now as: ex12 `SKY-T0012` `Roadmap.sky:50` "type `a` has no field `message`"; ex00 `found Error, expected Var(a)`; ex37 `cart = []` → `Dict String String` vs `List CartItem`. **Proper fix = rank-based let-generalization for untyped bindings** (M2a defers it; large, real risk to 400+ tests) → guardian design, do NOT hack. **Verify a fix with #66 no-panic fuzzer + adversarial review, not just the gate.**
  - **cheap independent win — ✅ LANDED** (merge, `9c44642`): gave `sky_types::solve::Constraint` a **home-module discriminant** so diagnostics point at the real file (byte-offset spans currently resolve against whichever merged file they land in — the cause of every mis-attribution above). Related: #35, #56, #66.
- **ex27** `27-multi-session-chat`: SKY-L0102 — **erased-`any` enum-variant payload feature gap** (`MessageReceived any`). NOT a bug to hack: needs type-system + lower + backend + runtime co-design. Proposed fix (from Lane V investigation): represent bare wildcard `any` in monomorphic positions as the existing erased `IrType::Json`, exempt `any` in `lower_enum` Gate 1 (mirror the freeTypeVars/Instantiate `any` filter), and flow the erased carrier end-to-end (`Cmd.publish` concrete→erased; `Db.get*` accept erased carrier via a `SkyRow` impl for the Json carrier). Add a regression: ADT with `any` payload dispatched from `subscribeTopic`, decoded via `Db.get*`. → guardian-design item, not a mechanical lane.

> **Note (2026-07-06):** with `Border.shadow` landed, ex27 excluded, and ex00/ex12/ex37 escalated as the one inference bug above, the sweep front currently has **no mechanical progdev-eligible item** — a progressive-development run would now return DRY. Next mechanical work must come from unblocking the inference bug (guardian) or from other backlog sections.

### [progdev-safe] — mechanical sibling kernels (template = landed `Border.shadow`, `416fd4f`)
- **`Border.glow : Int -> Color -> Attribute msg`** ✅ **LANDED** (orchestrated lane `lane-orch-143601-0`) — `BorderGlow` kernel wired across all sites (enum variant + decl arity-2 + ALL + is_ui + constrain curried scheme `Int→Color→Attr` + FIRST_SCHEMED + lower callee_arity Ok(2) + dispatch + pretty + naming + emit_expr 2-positional arm + runtime `ui_border_glow_(blur, c)` → `AttrStyle("box-shadow", "0px 0px <blur>px 0px <colour>")`). Regression test `border_glow_renders_box_shadow` added. Colour routed through the same `color_to_css` boundary as `Border.color`/`Border.shadow`.
- **`Border.innerShadow : { offsetX, offsetY, blur, spread : Int, color : Color } -> Attribute msg`** ✅ **LANDED** (orchestrated lane `lane-orch-143601-1`) — `BorderInnerShadow` kernel wired across all 8 sites, reusing `Border.shadow`'s 5 field symbols + record-destructure and the already-present `AttrBorderInsetShadow` runtime variant (renders `box-shadow:inset …`). Regression test `border_inner_shadow_renders_inset_box_shadow`.
  These two share the registry files (sky_kernels/constrain/lower/naming/pretty/emit_expr) → a parallel `orchestrate.sh` run WILL conflict on merge → exercises the Opus reconcile path.

Wrinkle for all lanes: the shared `~/.cache/sky-rust-target` suffers cross-worktree stale-rlib thrash under concurrent builds (spurious "variant not found" for variants that exist in source). Use a per-lane `CARGO_TARGET_DIR`, or rebuild the dep chain; the merge gate is unaffected (isolated `~/.cache/master-gate-target`).

## Tier-1 — finish the parity sweep + push (ORDER: sweep-green → seal → #110 → #37 → #59 → push)
- **#35** Port examples-sweep to skyc + run the full sweep (the source-of-truth gate).
- **#110** Oracle full-activation: wire HTML/tui/scenario normalizers + rebuild release skyc + flip CI phase-2 + 65-fixture divergence corpus.
- **#37** Fix CI (port ../sky examples-sweep.yml + ci.yml) + push to `git@github.com:arthurmaciel/ipe-lang`. Includes the CI example-patch-queue idea (in-repo patches CI applies to upstream examples before build — drop `Task.run`, margin stripping, rename — + adapt Go-only examples). See `docs/ideas/departures-from-sky.md`.
- **#56** Prove row-poly subset/superset record resolution (A7 watch) + gate on sweep.
- **#45** Make constrain kernel-scheme table exhaustive over canon lists (close exit-0-then-cargo-fail class).
- **#70** Fix kernel arity-table drift (`decl().arity` vs `callee_arity`) — latent exit-0-then-cargo-fail.
- **#71** Fix `explain_lookup`: UnknownCode for 8 real page-backed diagnostic codes.

## Security tier — BEFORE the compiler push (per memory `security-hardening-before-push`; all Tier-1 except FFI-sandbox which gates FFI)
- **#44** Opaque `Secret` stdlib type (gates WASM hydration island + CLAUDE.md §8 secrets-are-typed).
- **#61** `SqlFragment` param-query newtype — SQL injection = type error.
- **#63** Port `Sky.Http.Middleware.withCsrf` (constant-time, `__Host-`, parse-once).
- **#66** ✅ **LANDED** — Well-typed no-panic fuzzer `scripts/fuzz-well-typed.sh` (HM-valid templates, LCG seeding, skyc→cargo→run, panic detection incl. the Ipê `[error] <Class> (ref …)` runtime format). TRUE-POSITIVE proven (`42 // 0` → DivisionByZero flagged); clean iters. Wired into autopilot as the guardian soundness oracle (dual-role: measure-phase bug-finder + guardian-gate verifier). Template set being extended to ~18 construct-family shapes (case/ADT-match, let-poly, HOF, record-update, tuples, recursion, Dict/Set, andMap, pipelines, interp).
- **#66-T2 — Type-directed well-typed AST generator (fuzzer Tier-A)** — 🔵 guardian-typesystem, multi-session PROJECT (design item, do NOT rush). Tier-B (#66) is fixed templates with seed-filled holes — broad but NOT combinatorial. Tier-A generates ARBITRARY well-typed programs *by construction*: given a goal type τ + context Γ, recursively emit `e` with `Γ ⊢ e : τ` (the typing rules run "in reverse" as a generator) → checks the pipeline in-process (fast, combinatorial — constructs nested arbitrarily, where the subtle soundness bugs hide). **Adopt the reference design:** `../sky` `test/Sky/Build/WellTypedFuzzerGen.hs` + `WellTypedFuzzerSpec.hs` (Haskell QuickCheck). Port to Rust via `proptest`/`arbitrary`: a type-directed term generator + shrinker; assert `skyc` accepts + the emitted program is no-panic. **Load-bearing invariant:** "well-typed by construction under generation" — get it wrong and you fuzz the parser, not the type system. **Scope:** pure type-relevant constructs (expressions/types/patterns/ADTs/records/let-poly/HOF); effects/apps/FFI stay OUT (the example sweep covers those). Wire as a second soundness oracle alongside Tier-B in the autopilot guardian gate. Related: #66, #35, #66-fuzzer.
- **#66-N — Negative / rejection fuzzer (the inverse of #66)** — the other half of "if it compiles, it works": #66 checks *good programs are accepted + run clean*; #66-N checks **broken programs are REJECTED** (false-acceptance is where soundness dies — an accepted ill-typed program → exit-0-cargo-fail or miscompile).
  - **First half — guaranteed-breaking mutation tier (✅ DONE, `scripts/fuzz-ill-typed.sh`, e2f7b98):** 7-entry catalogue of ill-typed-by-construction mutations → assert `skyc` REJECTS with the expected code: cat1 undef-field `.value<hex>`→`SKY-T0012`, cat2 undef-var→`SKY-N0001`, cat3 unknown member `String.<typo>`→`SKY-N0005`, cat4a `String.length <int>`→`SKY-T0001`, cat4b `if <int> then`→`SKY-T0001`, cat5 ctor-arity `Nothing <int>`→`SKY-T0001`, cat6 non-exhaustive case→`SKY-T0010`. Self-validated on master: 6/6 bases compile clean, 7/7 cat-demo rejected with correct code, 28/28 sweep rejected (0 false-acceptances, all 7 cats covered). Modes: `--base-sanity` / `--cat-demo` / `--iters N --seed S` / `SKY_FUZZ_NEG_FULL=1`. Reuses `lib/env.sh` + LCG + timeout-bound + forensics from #66. A mutant skyc ACCEPTS aborts the run (exit 1, forensics to `/tmp/sky-fuzz-neg/FAILURES/`) — a real false-acceptance finding. Honest scope: the loop counts any rejection as success (the soundness property); `--cat-demo` is the stricter right-code check. NOTE (not a bug): multi-clause function pattern-dispatch (`f _ 0 = …`) trips `SKY-P0030` in the Rust backend — correct, Sky is single-clause + case like Elm; the cat5 base was written single-clause.
  - **Second half — differential vs `../sky` (🔵 FUTURE, with a CAVEAT):** mutate freely, run both compilers, compare accept/reject. **Do NOT treat the reference as ground truth:** `../sky` is NOT verified sound for versions >v0.16.29 and is KNOWN to have missed subtle errors at v0.16.29. A divergence is a **REVIEW candidate, never an auto-verdict.** Specifically **"Ipê rejects, Sky accepts" is most likely Ipê being MORE correct** (Sky false-accepted) — a sanctioned-divergence candidate, NOT an Ipê bug. Only "Ipê accepts, Sky rejects" weakly hints Ipê might be over-lax, and even that needs human review (Sky could be over-strict). Use `../sky` as a corpus/inspiration for interesting mutations, not as a correctness oracle. Related: #51 (equiv harness), sanctioned-divergence policy.

## SEAL / correctness follow-ups (exit-0-then-cargo-fail + runtime-correctness)
- **🐛 FUZZER FINDING — `SKY-I0001` on `{{interp}}` of a qualified call with a literal arg** (guardian-typesystem/lowering; surfaced by fuzzer template 18, 2026-07-06). `msg = """count={{String.fromInt 54}}"""` → "unbound local `<N>`" ICE. The interpolation lowering mishandles an interpolated expression that is a qualified call (`String.fromInt`) applied to a **literal** (not a bound var) — it emits a reference to a local that was never bound. Workaround in the wild: bind the value first (`let n = 54 in ...{{String.fromInt n}}...`). Root-cause the interpolation lowering path (a bound-var vs inline-expr distinction). Repro preserved at `/tmp/sky-fuzz/FAILURES/seed-1016-*/`. (This is the kind of item the autopilot's measure-phase fuzzer would auto-file as a guardian item.)
- **#99** Refutable match-arm alias over non-Copy payload double-moves — `case m of Just ((a,b) as w) -> use a,b,w` → E0382 (pre-existing, out of #96 scope).
- **#90** SKY-L0114 ctor-payload-function blocks `Ok`/`Just` holding a function → `Result.andMap` / `Maybe.andMap` unusable.
- **#125** Decoder thunk coverage: tuple-destructure + record-field binders (loud E0382, pre-existing).
- **#142** Precision follow-up: #139 made `Expr::Access` clone unconditionally + blanket `Clone` bounds on fn generics — restore borrow fast-path.
- **#102** F1 (low, fail-closed): local `type X` shadowing a dep-imported `X` → downstream SKY-T0001 instead of clean SKY-N0012 at the decl.
- **#109** OnRaw stale comment + `onSubmit` runtime no-op (correctness).
- **#113** Pseudo-class attrs render to nothing in static `htmlRender` sink (AttrPseudoRule no-op).
- **#31** Phase 4 — make-invalid-states-unrepresentable hardening (non-blocking).

## Rename (pre-push, per memory `pre-push-rename-sky-to-ipe`)
- **#59** PRE-PUSH: full codebase rename Sky → Ipê/Ipe/ipe (case-preserving; watch the naive-sed trap — upstream-Sky refs stay Sky).
- **#75** Rename `type Color` → `Swatch` in 5 fixtures + 2 canon tests, then add `Color` to `RESERVED_BUILTIN_TYPES`.

## Non-blocking hardening / follow-ups
- **#32** M5a follow-ups (fail-closed): Task arity-3 ICE + Task-in-ADT-ctor gate.
- **#33** M5b-http follow-ups: header-case parity, extra Http builders (|> already done).
- **#34** M5b-db follow-ups: SqlValue variant completeness, exhaustive `emit_db_call`, self-oracle, db-without-live build.
- **#105** Std.Css hardening (defence-in-depth): optional @import/expression gating on `raw`/`keyframes` bodies + reject CSS-hex-escaped values in `safeValue`.
- **#129** Runtime audit: `spawn_blocking` for CPU-heavy/blocking kernels (bcrypt/zstd/file) — reactor-starvation guard.
- **#122** Cli.program view printer: missing separator (`lines: 0lines: 1`) — cosmetic runtime nit.
- **#53** Backlog: emit backend via typed token AST instead of String concatenation.

## Tier-3 — macro roadmap (post-parity; ORDER: Elm-core coverage → salsa → LSP → watch → departures)
- **#155** TIER-3 DX: route URL changes to a Msg (Elm `Browser.application` parity), demote magic `page` field to sugar. First Elm-core-coverage item.
- **#116** Entry contract (Option C): auto-run Task/backend-app `main` + drop trailing `|> Task.run` (v0.17.3 pipeCollapsesTask parity).
- **#128** POST-PARITY DEPARTURE: drop `Task.run` + `Task.perform` from Ipê surface (#116 companion).
- **#131** POST-PARITY: `Task.map2..5` + `Task.parallel2..5` (expression forms) + `parallelDo` block.
- **#133** POST-PARITY DEPARTURE: multiline-string margin stripping (anchor = first string character's column).
- **#85** Error rich-ADT (PARKED, non-blocking): kernel-path impl + atomic 69-golden SkyError flip — NOT source-module.

## Tier-2 — FFI (LAST, per memory `roadmap-tiers`)
- **#40** FFI Phase 0 — inspector hardening (disjoint `tools/` crate).
- **#41** FFI sandbox — blocking security gate before `ipe add` ships.
- **#42** FFI consumer port — generator (Haskell → Rust `ipe_ffi` crate).
