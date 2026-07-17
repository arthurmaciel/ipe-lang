# Reference cross-reference — examples-sweep blockers (2026-07-13)

How the **reference Sky implementation** (`../sky`: `src/` = Haskell compiler,
`runtime-rust/` = Rust backend + runtime) handles each pending Ipê sweep blocker.
The reference has a Haskell→Rust backend at
`../sky/src/Sky/Generate/Rust/Builder/` (`ExprEmitter.hs` ~4334 lines,
`ModuleEmitter.hs`, `TypeRenderer.hs`, `TypeEmitter.hs`, `Types.hs`, `Pattern.hs`,
`Walker.hs`) + runtime at `../sky/runtime-rust/src/sky_runtime/`.
**8 of 9 blockers have a reference template → MIRROR it.** Fix lanes: read the
cited file:line first.

---

## #180 — Live.app `init : {}` vs required `LiveReq` — REFERENCE SOLVES (MIRROR)
Reference supports BOTH `{}`-init and `LiveReq`-init via three mechanisms:
1. **Free type var for init's arg.** `src/Sky/Type/Constrain/Expression.hs:2674-2680`
   — `init : req -> (Model, Cmd Msg)`, `req` a free `TVar` (not pinned). `{}`-init unifies.
2. **Body-reads-req detection.** `src/Sky/Generate/Rust/Builder/Walker.hs:350-374`
   `collectLiveReqInitFns` collects only inits that actually read `req.*`; only those pin to `LiveReq`.
3. **Discard wrapper for non-readers.** `ExprEmitter.hs:1686-1696`:
   `{ let __sky_init = init; move |_r: sky_runtime::LiveReq| __sky_init(()) }`.
   Runtime always calls `Fn(LiveReq)` (`runtime-rust/.../live/mod.rs:1082`); wrapper discards it.
Reference examples: `examples/26-ui-showcase/src/Main.ipe:22` (`{}`-init), `examples/09-live-counter/src/Main.ipe:40`.
**FIX: MIRROR. The `{}`-init example is CORRECT.** Type init arg as free tvar; detect body-reads-req; emit discard-and-call-`()` wrapper for non-readers.

## #181 — polymorphic-kernel turbofish (E0282/E0283) — REFERENCE SOLVES (MIRROR)
Threads solved expected type via `EmitCtx.ecExpectedType`; emits turbofish when concrete, default otherwise.
- `dict_empty`: `ExprEmitter.hs:3081-3090` (`::<K,V>` if concrete else `::<String,i64>`).
- `task_fail`: `ExprEmitter.hs:3102-3114` (`::<_,A>` if concrete; suppressed when `ecInGenericFn`; else `ecReturnElem`/`::<_,i64>`).
- Wiring: `ExprEmitter.hs:1280-1290` (`n ++ taskFailPin ctx`), `950-955` (seed `ecExpectedType` from region).
**FIX: MIRROR.** Build `ecExpectedType` from solved region type; `hasTypeVars` guard before turbofish; add per-kernel pins (`list_head`, `result_map_error`, `decimal_from_string`, `set_empty`); respect the `ecInGenericFn` guard. Reads: `ExprEmitter.hs:950-955,1280-1290,3081-3114`; `ModuleEmitter.hs:770-836`.

## #177 — Db decode `T1: SkyRow` bound (E0277) — REFERENCE SOLVES (MIRROR)
Bound goes on the FUNCTION SIGNATURE, not the struct.
- `runtime-rust/.../db.rs:745-786`: `trait SkyRow { fn sky_get(&self,&str)->String }`, impls for `SkyDict<String>`+`LiveReq`, `pub fn db_get_string<R: SkyRow>(...)`.
- `ModuleEmitter.hs:770-836`: `bodyHasDbGet = "db_get_" isInfixOf tdBody`; `genBound` appends `+ SkyRow` ONLY to the `any` tvar AND only when body calls `db_get_*`.
- Structs emitted WITHOUT bounds: `TypeEmitter.hs:166-177`.
**FIX: MIRROR.** Do NOT bound the record struct. Scan emitted fn body for `db_get_`; conditionally add `+ SkyRow` to the wildcard/`any` generic param. Ensure `SkyRow` in scope.

## #178 — E0308 UI input/filter/SkyMaybe — SOLVER-SIDE (likely local, NOT emitter)
Reference renders `Maybe a`→`SkyMaybe<T>` (`TypeRenderer.hs:157`), `Nothing`→`SkyMaybe::Nothing` (`ExprEmitter.hs:1120,1135`); record-field values via `wrapStoredFn` (`ExprEmitter.hs:2534-2537`) which Arc-wraps only FUNCTION fields — NO `Maybe` coercion inserted. Assumes the SOLVER already gave the field its `Maybe String` region type.
**FIX: MIRROR no-coercion policy; fix region-type SEEDING.** Chase why the record field's region resolves to the inner type instead of `Maybe String`. Do NOT hand-insert coercions.

## #179 — E0308 polymorphic `Vec<Attribute<T1>>` — REFERENCE SOLVES (MIRROR)
`Attribute` registered as generic runtime-opaque alias with a `{M}` marker preserving Sky args:
- `Types.hs:269-271`: `("Std.Ui","Attribute") -> "sky_runtime::ui::Attribute<{M}>"`.
- `TypeRenderer.hs:109-131` `collectRenderedTVars` collects the free `msg`; `253-271` renders `Attribute<Msg>` when args present. Returned `vec![...]` needs NO ascription — Rust unifies from the signature.
**FIX: MIRROR.** Ensure Ipê's runtime-opaque registration for `Attribute` carries the `{M}` marker (else arg dropped → bare `Vec<Attribute>`); ensure `collectRenderedTVars` runs so `msg` lands in the fn's generic params.

## #172 — mixed Arc/Box closure unification in if/case branches — REFERENCE SOLVES (MIRROR: uniformly Arc)
Reference UNIFORMLY Arc-wraps closures; emits each branch independently against a seeded Arc expected type.
- `wrapStoredFn` (`ExprEmitter.hs:2880-2940`): lambda→`Arc::new(move ...)` (capture pre-clones); event/Handler fn-item→`Arc::new`; non-fn→unwrapped.
- `ExprEmitter.hs:3255-3328`: `let __sr: Arc<dyn Fn(&SkyError)->bool + Send + Sync> = if .. { h1 } else { h2 };` — both branches conform to the SAME Arc target (seeded), not emitter-unified.
**FIX: MIRROR — go uniformly Arc.** Our failures (inline `.clone()` on Box; nested-if Arc-vs-Box) come from not committing to Arc. Seed the callback slot's Arc expected type before rendering each branch; wrap EVERY branch closure `Arc::new(...)`; drop the Box path for callback slots. Reads: `ExprEmitter.hs:2880-2940,2598-2606,3255-3328`; `runtime-rust/.../html.rs:45-53`.

---

## Validation notes for already-fixed/in-review

- **#174 (tuple `case`):** reference emits native `match` (`ExprEmitter.hs:2443-2477`); refutable arg patterns use synthetic param + `let <pat> = __pN else { panic!() }`, dropping `else` for irrefutable (avoids `irrefutable_let_patterns`) — `Pattern.hs:113-140`. Confirm ours emits `match` + gates let-else `else` on refutability.
- **#175 (Std.Ui.Animation):** reference HAS `../sky/sky-stdlib/Std/Ui/Animation.sky` (Iterations/FillMode/Spec + `with*` + `attribute : Spec -> Attribute msg`). It uses `Ui.animateRaw : String->String->String->Bool->Attribute msg` (`Std/Ui.sky:1433-1435`) constructing `AttrAnimation`. **RECONCILE:** our #175 added a `UiAnimateRaw` kernel — confirm it matches the reference's `animateRaw` shape (same 4-arg sig → aligned) rather than a divergent dedicated `animate` kernel.
- **#176 (callback Send+Sync):** reference emits `Arc<dyn Fn(..)->M + Send + Sync>` (`html.rs:45-53`, `tea.rs:31-32` `SubSpawn: Arc<dyn Fn(M)+Send+Sync>`), NOT unboxed/monomorphized. **NOTE:** our #176 used unboxed monomorphization — valid IF our runtime slot is a generic `F: Fn+Send+Sync` param (it is, per the fix), but the reference stores Arc<dyn> in a field. Different runtime shape → both can be correct; the #176 review confirms ours builds+runs against our runtime signature.
