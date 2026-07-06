# Backlog — open work items

> Durable replacement for the in-session task board (cleared 2026-07-06 to stop
> per-turn re-injection cost). This is the authoritative list of OPEN work.
> Read on demand; update as items land. Soundness/security/correctness outrank
> everything — never close an item with a workaround (per CLAUDE.md §4
> no-deferral + §7 root-cause-only). Related: `sweep-burndown-map.md`,
> memory `roadmap-tiers` / `security-hardening-before-push` / `endgame-example-sweep`.

## Sweep front — current per-example first blockers (as of master 5b11260, 2026-07-06)
> Landed this session: #154 (misplaced-span kernels wired), batch-U (ex00 Money/ex15 Handler/ex27 Db-scheme), batch-V (ex00 Time kernels, ex15 Request-fields+Handler-alias — ex15 now build-proven end-to-end).
- **ex00** `00-standard-libs`: SKY-T0001 in compiled-source `Std.Money` — `fromMinor` "expected a, found Error" (pre-existing, unrelated to Time).
- **ex12** `12-skyvote`: SKY-T0001 — `Ok _` arm at `src/Lib/Ideas.sky:148` expects `Int`, found `Float`.
- **ex37** `37-composite-live-shop`: SKY-L0108 — `Border.shadow` at `src/View/Home.sky:173`. **Ready-to-wire recipe (fully scoped 2026-07-06, ~6 files, mechanical):** `Border.shadow : { offsetX:Int, offsetY:Int, blur:Int, spread:Int, color:Color } -> Attribute msg`. Reference is pure-Sky (`../sky` Std/Ui/Border.sky `shadow`) → CSS `box-shadow: <ox>px <oy>px <blur>px <spread>px <color>;`. Wire as an **arity-1 record kernel** exactly like `BorderWidthEach` (its template lives at each site below), reusing the generic `Attribute::AttrStyle("box-shadow", …)` variant so **no** new runtime enum variant / renderer arm is needed:
  1. `runtime/src/sky_runtime/ui/helpers.rs` — add `ui_border_shadow_<M>(ox,oy,blur,spread,c:Color) -> Attribute<M>` returning `Attribute::AttrStyle("box-shadow".into(), format!("{ox}px {oy}px {blur}px {spread}px {}", <color→css>))`. Color→css helper is the one `render.rs:205` (`AttrBorderColor`) uses.
  2. `crates/sky_kernels/src/lib.rs` — `BorderShadow` variant (~733), `d("Border","shadow",1,Ui,"ui_border_shadow_")` (~1860), ALL list (~2783), match arm (~3331).
  3. `crates/sky_types/src/constrain.rs` — scheme (~4837, next to `K::BorderWidthEach`): record `{offsetX,offsetY,blur,spread:int(), color:color()}` → `attr(var(0))`; add the 5 field-name symbols to `builtins` (mirror `edge_f_top = intern("top")` at ~480/327); add to FIRST_SCHEMED (~5974).
  4. `crates/sky_lower/src/lower.rs` — dispatch `("Border","shadow") => Kernel(BorderShadow)` (~8517) + the record-destructure match arm (~7391) that spreads the record's 5 fields into positional args (this is the one subtlety — copy BorderWidthEach's 4-field destructure, extend to 5 incl. the Color field).
  5. `crates/sky_backend_rust/src/naming.rs` — `"ui_border_shadow_"` (~824).
  6. `crates/sky_ir/src/pretty.rs` — `"Border.shadow"` (~736).
  Then also wire `Border.glow`/`Border.innerShadow` (same family, reference wraps them onto `borderShadow`/`borderInsetShadow`) if ex37 uses them. Gate: workspace test + clippy on the isolated target; rebuild ex37 to confirm the L0108 is gone + note the next blocker.
- **ex27** `27-multi-session-chat`: SKY-L0102 — **erased-`any` enum-variant payload feature gap** (`MessageReceived any`). NOT a bug to hack: needs type-system + lower + backend + runtime co-design. Proposed fix (from Lane V investigation): represent bare wildcard `any` in monomorphic positions as the existing erased `IrType::Json`, exempt `any` in `lower_enum` Gate 1 (mirror the freeTypeVars/Instantiate `any` filter), and flow the erased carrier end-to-end (`Cmd.publish` concrete→erased; `Db.get*` accept erased carrier via a `SkyRow` impl for the Json carrier). Add a regression: ADT with `any` payload dispatched from `subscribeTopic`, decoded via `Db.get*`. → guardian-design item, not a mechanical lane.

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
- **#66** Well-typed no-panic fuzzer (soundness gate) — adopt from ../sky 9e170314.

## SEAL / correctness follow-ups (exit-0-then-cargo-fail + runtime-correctness)
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
