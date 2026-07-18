# Cross-Design Coherence Review — banked specs for future work

> **Date:** 2026-07-04 (review pass ~09:30 -03).
> **Reviewer:** Fable Design-Review Lane. READ-ONLY on crates; this doc is the
> only file written. Every claim below was verified against the actual tree
> (working tree includes the uncommitted #108 T1–T7 implementation in
> `sky_types`/`sky_lower`/`sky_backend_rust`), not taken from the designs'
> own text.
> **Tree state at review:** HEAD `49ab842` (task #73); #108 implementation
> present but uncommitted (`constrain.rs`, `unify.rs`, `ty.rs`, `lower.rs`,
> `emit_live.rs`, `ipe/lib.rs`, `live_e2e.rs` modified).

---

## 0. One-screen verdict table

| Design | Verdict | Blocking corrections before execution |
|---|---|---|
| `seal-noncopy-move-design.md` (#104/#99/#104b) | **NEEDS-FIX** | Clone-availability invariant is false for `Fun`/`Task`/`Cmd`/`Sub`/`Decoder`/`Db` (§1.1); stale line cites |
| `seal-gates-msg-lambda-view-design.md` (#94/#95) | **READY** (with 3 required amendments) | L0121 collision (C1); must also thread `fn_param_ty` through `routed_page_field` (C4); ordering vs #90 (C2) |
| `seal-jsondecp-design.md` (#89) | **NOT PRESENT** | Cannot review — still being written. Executor must NOT start #89 until it exists and C3 is honoured |
| `ctor-payload-function-design.md` (#90) | **READY** (with 2 required amendments) | Reuse-gate code L0121 → **L0122** (C1); hard ordering: #94 lands first (C2) |
| `effect-modules-kernel-plan.md` (#111) | **NOT PRESENT** | The single largest sweep unblock (5 examples) has no design artifact — highest-priority doc gap |
| `kernel-registration-backlog.md` | **NEEDS-FIX** | `Task.perform` mis-signatured + mis-tiered; `Ipe.Live.Head` already landed; `List.filterMap` missing from the backlog entirely |
| `routed-live-app-design.md` (#108) | **NEEDS-FIX** (doc drift; landed code is sound) | Part B `RoutedLiveCheck` undocumented; §6's IPE-L0119 is taken → **L0123**; unify step-4 re-binding omission unrecorded (§7.3); routed-lambda silent-unrouted hole |
| `oracle-and-tiered-verification.md` (#51/#110) | **NEEDS-FIX** (staleness only) | §4 item 1 / §8 item 1 (HTML normalizer) landed via `63f57b2` after the doc was written — retier `body` mode |
| `parity-gap-snapshot.md` | **READY** (as a dated snapshot) | 2 rows already overtaken (ex-33 parser fix `e586668`; #108 implemented); Fix-4 cost estimate collapses per the `Task.perform` correction |
| `parallel-lane-plan.md` | **STALE — supersede** | Pre-#108 ground truth; keep only the cluster-ownership protocol + the §6 rejection rationale |
| `docs/ideas/idea-7-effect-do-block-design.md` | **READY** | No conflicts; correctly deferred post-parity |
| `docs/divergences-from-sky.md` | **NEEDS-FIX** (2 edits) | B-route-param's "upgrade to IPE-L0121" → **L0123** (C1); B16 must be revised when #104's Clone-predicate fix lands |

---

## 1. Per-design review

### 1.1 `seal-noncopy-move-design.md` (#104 last-use clone + #99 alias clone-split + #104b) — NEEDS-FIX

**What holds up (verified in-repo):**
- `Expr::Var(sym) => ctx.emit_ident(*sym)` at `emit_expr.rs:2792` — exact.
- `BinOp::Append` → `format!` at `emit_expr.rs:2805` — exact; the borrow-position
  classification (§1.3) is sound.
- Runtime kernels are by-value (`string.rs:283 pub fn string_starts_with(prefix: String, s: String)`).
  Note the design quotes them as `starts_with(…)`; the real names carry the
  `string_` prefix — cosmetic, but fixtures asserting emitted text must use the
  real names.
- `emit_arm_head` IS the single shared choke point with an existing prelude slot
  (`str_binder_rebinds`/`list_binder_rebinds` composed at `emit_expr.rs:3227-3229`) —
  the #99 landing-site analysis is correct.
- The two-passes verdict (§8) is correct: #99 is a local pattern-renderer fix,
  #104 is a body liveness pass; they compose without ordering constraints.
- §4.2's reference-bug claim (alias name dropped, `ExprEmitter.hs:4206`) is
  already recorded as ledger B17 — consistent.

**Required fixes:**

1. **The load-bearing invariant in §0/§1.6 is FALSE.** The design states
   "*every non-`Copy` value is `Clone`* (guaranteed upstream by the #87/#93
   derive-seal), so a clone is always available as the escape hatch", and §1.6
   explicitly lists `Task`, `Fun`/closures among the "non-`Copy` but **is
   `Clone`**" set. Verified false:
   - `SkyTask<E, A> = Pin<Box<dyn Future<…> + Send>>` (`runtime/src/sky_runtime/core.rs:17`) — **not `Clone`**.
   - `IrType::Fun` renders `Box<dyn Fn(..) + Send + 'static>` (`emit_types.rs` Fun arm) — **not `Clone`**.
   - `Cmd`/`Sub`/`Decoder`/`Db` handles are in the same class (all `false` in
     `ir_type_is_derivable`, `ir.rs:699+`).
   Emitting `x.clone()` for an earlier owned read of any of these produces
   E0599 — a new exit-0-then-cargo-fail, i.e. the pass would *create* the
   seal-hole class it exists to close. `ctor-payload-function-design.md` §2
   (row 5) and §3 step 4 independently identified this and filed it as a
   requirement on #104 (its T7). **The #104 design must be amended before
   implementation:** partition non-`Copy` into *Clone-renderable* (String, Vec,
   records/enums passing the #87 fixpoint, tuples thereof → clone-all-but-last
   per §2.2) and *non-Clone-renderable* (`Fun`-embedding, `Task`, `Cmd`, `Sub`,
   `Decoder`, `Db` → **more than one consuming use is a diagnostic**, the
   semantics of #90's L0122 reuse gate, never a `.clone()`). The `is_copy`
   predicate of task A1 becomes a three-way classifier; add red fixtures for a
   twice-consumed `Task` binding and a twice-consumed fn-carrying `Maybe`.
2. **Generic type-vars need the same care.** §1.6 says a type-var is
   "conservative: clone is always sound since the bound carries `Clone`" —
   only true if the emitted generic bound actually is `Clone`; where a
   type-param can instantiate at a fn-carrying payload post-#90, cloning at a
   generic position is not universally available. The Opus review item in §7
   ("Copy predicate reads the binding's type") must extend to: the *emitted
   Rust generic bound* set for the enclosing function must include `Clone`
   for any type-var the pass clones.
3. **Line-cite drift (non-blocking, but fix before dispatch to a Sonnet lane):**
   the pattern-side anchors have shifted ≈380 lines since the doc was written
   (working-tree #108 + other edits): `emit_arm_head` 2834→**3217**,
   `render_pat` 3131→**3480**, `pat_contains_alias` 3195→**3578**,
   `emit_binding_stmts` 3237→**3620**, `emit_lambda` 4006→**3986**. Function
   names all still resolve — instruct the lane to navigate by symbol, not line.
4. **#104b (closure capture)** is correctly scoped out and filed; keep it out
   of #104's diff. Note it inherits fix 1 verbatim (a non-Clone capture cannot
   take the clone-prelude → diagnostic).

**Execution-readiness:** high once fix 1 is written in. Fixtures are concrete
and red→green; the §5.3 negative minimality assertion is a good guard.
Seal-tiering marking (Opus adversarial review before commit) is correct.

### 1.2 `seal-gates-msg-lambda-view-design.md` (#94 Msg gate + #95 lambda-view) — READY with amendments

**What holds up (verified):**
- `emit_model_gate.rs:38` `model_ty_of_view` / `:62` `check_admissible_model` — exact.
- Runtime bounds: Live `Msg: Clone + Send + Sync + Debug` (`live/mod.rs:1083`,
  and `live_app_routed` at `:1132`), Tui/Webview `Clone + Send` — the §2.1
  "Msg needs `derivable`, NOT serde, for all three shapes" finding is confirmed
  and is the design's most valuable correctness catch (preserves the
  Html-in-Live-Msg asymmetry).
- `Expr::Lambda { params: Vec<(Symbol, IrType)>, … }` at `ir.rs:982`;
  `lower_lambda` at `lower.rs:2227` (cited 2224 — trivial drift). The
  "lambda params carry solved concrete IrTypes" premise of #95 is real.
- Invocation sites: `emit_tui.rs:143` and `emit_webview.rs:126` exact;
  `emit_live.rs` drifted 229→**243** (model gate) and 220→**234** (`update_e`)
  due to the #108 edits. Cosmetic.
- Ledger A15 already records #94/#95 as designed-pending-impl with L0121 —
  consistent.

**Required amendments:**

1. **L0121 is contested three ways — see CONFLICTS C1.** This design keeps
   L0121 (it has the strongest documented claim); the other two claimants
   renumber.
2. **`fn_param_ty` must also serve #108's routed detection.** Verified:
   `emit_live.rs:374` `routed_page_field` delegates to `model_ty_of_view`.
   Because `model_ty_of_view` is FuncValue-only today, a **routed app whose
   `view` is a lambda silently emits the NON-routed `live_app`** (routes and
   `notFound` dropped) — not a cargo-fail but silently wrong runtime behavior,
   a *worse* failure class than the one #95 documents. Meanwhile the
   type-tier `RoutedLiveCheck` (see §1.7) reads the *solver's* Model and
   correctly detects the same app as routed — the two tiers disagree exactly
   on the lambda-view shape. When A2 re-expresses `model_ty_of_view` as
   `fn_param_ty(view_e, 0)`, `routed_page_field` inherits the fix for free —
   but the design must say so and add the fixture:
   `LIVE_LAMBDA_VIEW_ROUTED` (routed cfg + lambda view + plain Model) must
   emit `live_app_routed`, and a cross-tier consistency test must assert
   RoutedLiveCheck-routed ⇔ emit-routed on the same corpus.
3. **Ordering vs #90:** this design must land **before or together with** #90's
   Stage 1 — see C2. Add that constraint to §5.
4. §3.3's residual (cfg field is neither FuncValue nor Lambda) is narrower than
   the doc implies: per landed A16 (`IPE-L0119`, `code.rs:303`) the cfg itself
   must be an inline record literal, so only the *field* expressions can be
   odd shapes. The fail-closed option in §3.3 (reject non-FuncValue/non-Lambda
   `view`/`update` fields) is therefore cheap and consistent with the A16
   precedent — recommend adopting it at Opus review rather than leaving the
   documented fail-open residual.

**Execution-readiness:** high — file:line anchors verified, fixtures concrete,
tiering (Opus, seal-touching) correct.

### 1.3 `seal-jsondecp-design.md` (#89) — NOT PRESENT

Not on disk at review time (another agent may still be writing it). Two
obligations recorded for whenever it lands:
- Its E0382-moved class **must be checked against #104's general read pass
  first** — if #104's liveness pass covers the same emit sites, #89 should
  reuse it, not add a second pass (one liveness engine, two consumers). See C3.
- If it mints a diagnostic code, the next free slot after this review's
  assignments is **IPE-L0124** (see C1).

### 1.4 `ctor-payload-function-design.md` (#90) — READY with amendments

The strongest doc in the set: fresh line cites (written 09:24 today, verified —
`lower.rs:1581` decl gate, `lower.rs:2856` region gate, bounded derives at
`core.rs:224`/`396`, `curryN` at `json.rs:799-822`), a correct
hazard-by-hazard table, and it independently caught #104's false invariant
(§2 row 5, T7). The over-restriction verdict is sound: construction was never
the unsound step; the two residuals (curried-`andMap`, fn-carrier reuse) are
gated at their actual unsound sites.

**Required amendments:**

1. **Step 4's new code is NOT L0121** — renumber `Feature::FunctionValueReuse`
   to **IPE-L0122** (C1).
2. **Hard ordering constraint (add to §6):** T2 deletes `lower_enum`'s
   `ir_contains_fun` decl gate, which is what makes an fn-carrying **Msg**
   type (`type Msg = WithK (Int -> Int)`) representable at all. After T2,
   the only thing standing between that Msg and a cargo trait-bound failure
   is the #94 Msg gate. **#94 must land before (or in the same PR series as)
   #90 Stage 1**, otherwise T2 opens a seal hole on the app-entry surface.
   The design's §4 checklist covers Model (L0120 row) but is silent on Msg —
   add the row: "Live/Tui/Webview **Msg** holding `Maybe (a->b)` / declared fn
   payload → #94 gate → IPE-L0121".
3. Minor: §3 step 4's consuming-use counter is a deliberate interim that #104
   supersedes — state explicitly that #104's landing must *keep the L0122
   diagnostic semantics* for non-Clone carriers (same fixture set), so the
   gate's removal is a refactor, not a behavior change.

**Execution-readiness:** high. Seal-tiering (Opus review, YES) correct.
The `golden_m3a_function_payload_gate.rs` either-branch test being pre-written
for the lift is verified (`ipe/tests/golden_m3a_function_payload_gate.rs`).

### 1.5 `effect-modules-kernel-plan.md` (#111) — NOT PRESENT

Not on disk. This is the **most urgent missing design**: the parity snapshot's
critical path ranks #111 as Fix 1 (5 examples: 12, 20, 28, 30, 32), and the
kernel backlog defers four heavy modules to it (`Ipe.Auth`, `Ipe.Cli`,
`Ipe.Http.Stream`, `Ipe.Http.Server.Stream`). Until it exists, Lane A has
no implementable spec for the largest single unblock. Recorded obligations:
- It must state the Cmd/Task entry contract it assumes. Note: **"#116
  entry-contract" has NO in-repo artifact** — `rg '#116'` over `docs/` +
  `crates/` finds nothing. Whoever owns #116 must write it down before #111
  bakes in assumptions about the Cmd/Task boundary.
- `Task.perform` is NOT in its scope (it is a `Task.run` alias — §1.6 below);
  `Cmd.perform`-style dispatch kernels are.

### 1.6 `kernel-registration-backlog.md` — NEEDS-FIX

**What holds up (verified):** `CmdPublish`/`CmdPublishNoEcho`/`SubSubscribeTopic`
exist in `StdlibKernel` (`sky_kernels/src/lib.rs:529/531/533`) and are excluded
from `ALL` — ALIAS-GAP classification correct. `FontLineThrough`,
`TimeTimeString`, `TaskPerform` absent from the enum — correct. The
tier structure and the Lane A/Lane B split are sound.

**Required fixes:**

1. **`Task.perform` is mis-signatured and mis-tiered.** The backlog's row says
   `perform : Task e a -> (Result e a -> msg) -> Cmd msg` (KERNEL, Tier-3,
   ~1 day). Verified upstream (`../sky/sky-stdlib/Sky/Core/Task.sky:85-95`):
   `perform : Task e a -> Result e a` — the **legacy 1-arg alias of
   `Task.run`** ("same as `run` — kept as the legacy name"). `TaskRun` already
   exists in the enum (`lib.rs:427`). So `Task.perform` is a **Tier-1
   alias-thin registration (~30 min)**, not a Tier-3 medium kernel; the
   Cmd-dispatch signature belongs to `Cmd.perform` only. The backlog's own
   note half-spots this but the "alias under both qualifiers pointing to the
   same variant" suggestion is wrong — alias `Task.perform → TaskRun`, do NOT
   involve `Cmd.perform`. This re-orders the critical path: `simple` and
   00-standard-libs' first blocker clear for near-zero cost.
2. **`Ipe.Live.Head` already landed** (commit `3904173`,
   `crates/ipe/stdlib/Std/Live/Head.ipe` + `stdlib.rs:220` verified). Remove
   from Tier 2 (item 9), Lane B table, and Top-5 (item 5). Example 38's next
   blocker is `Ipe.Ui.Region` — which the parity snapshot (later the same day)
   already shows.
3. **`List.filterMap` is missing from the backlog entirely.** The parity
   snapshot records it as 16-ipehess's *first* blocker (N0005); the backlog
   lists 16-ipehess only under `Error.ErrorKind` (task #85). Both are real —
   filterMap fires first. Add `List.filterMap` as a Tier-1 thin kernel
   (upstream `Ipe.List`); the deduped count becomes 28.
4. Typo: `Stime.isLeapYear` → `Ipe.Time.isLeapYear` (§1 table, row
   00-standard-libs).

### 1.7 `routed-live-app-design.md` (#108) — NEEDS-FIX (doc drift; the landed code itself is sound where checked)

**Landed-state verification.** T1–T7 are genuinely in the working tree:
- **T1/T2:** `Ty::Record(map, RowTail)` + `FlatType::EmptyRecord` (`ty.rs:64/73/300`);
  the open-record `unifyRecords` port at `unify.rs:259-340` (shared-field
  unify → closed-absorb guard → step 3 tail-unify → step 4 fresh-tail merge).
- **T3:** `K::LiveApp` open six-field scheme at `constrain.rs:3034` with
  `routes : List (LiveRoute page)` — note the landed `LiveRoute` is
  **phantom-parametric** (`live_route(page)` at `constrain.rs:2311-2324`,
  `Live.route : String -> page -> LiveRoute page` at `:3075`), a refinement
  over the design's §1.4/§3 which describe a **nominal, non-parametric**
  `LiveRoute`. The parametrisation is what lets the scheme thread
  `page = var(2)` from routes into `notFound`.
- **T4:** `LiveAppRouted` aliased to the same `lower_app_entry_cfg` path
  (`lower.rs:3323`) — design option (a); IPE-L0118 kept as tombstone.
- **T5:** emit branch + `set_page` + `live_app_routed` (`emit_live.rs:260-290`,
  `routed_page_field:374`).
- **T6:** `route_param_get` (`emit_live.rs:333`) with String/Int/Float/Bool
  conversions and a **`Diagnostic::CompilerBug`** for other payloads — NOT the
  design's proposed IPE-L0119.

**Required fixes:**

1. **Document Part B.** The doc's status header claims T1–T7 complete but the
   body never describes the landed **`RoutedLiveCheck` post-solve hook**
   (`constrain.rs:689-716` struct + push at `:1628-1645`;
   `resolve_routed_live_checks` at `sky_types/src/lib.rs:448`, run from
   `lib.rs:120`): one check per `Live.app` call site, deferred until the solver
   settles; if the settled Model has a `page` field, `notFound` is unified with
   `Model.page` (IPE-T0001 on mismatch, pre-empting the emitted `set_page`
   closure's E0308/E0631). This is a genuine design addition — the
   conditional `Model.page ≡ notFound` constraint is inexpressible as a plain
   build-time HM constraint. Add a §5-bis describing it, plus the
   phantom-parametric `LiveRoute` refinement of item T3 above.
2. **§6's IPE-L0119 proposal is dead — the code is taken.** Verified
   `code.rs:196/303`: `IPE-L0119` = "app entry cfg must be an inline record
   literal" (ledger A16). The landed interim is `CompilerBug` in
   `route_param_get`; the ledger's B-route-param says "to be upgraded to …
   IPE-L0121" which collides with #94. Per C1 the route-payload diagnostic is
   **IPE-L0123**. Update §6 and the ledger entry.
3. **The unify step-4 tail re-binding omission is real and recorded NOWHERE
   actionable.** The design's own §4 pseudocode carries the comment "Re-point
   `ext1` and `ext2` at records that carry the *other* side's extras + the
   fresh tail … (see Unify.hs:496 — the reference re-binds each side)", but
   the landed `unify.rs:319-337` step 4 only unions `ra`/`rb` under the merged
   map + fresh tail — **`ext1` and `ext2` are left as unconstrained flex
   vars**. Consequence: a third record type sharing `ext1` never learns about
   the other side's fields — an under-constraint that can accept programs the
   reference rejects (or lose field info downstream). Practical exposure today
   is low (only the three app-cfg kernel schemes are open, each instantiation
   is fresh, and A16 forbids let-bound cfgs), but it becomes live the moment
   task #56 generalises row polymorphism to user records. **File it now** as
   an explicit #56 sub-item with an adversarial fixture (two open records
   unified pairwise through a shared tail; assert the field propagates), and
   add a code comment at `unify.rs` step 4 naming the omission. Neither the
   code, this design, nor any other doc currently records it.
4. **Routed-lambda silent-unrouted hole** — see §1.2 amendment 2 and C4. The
   `lib.rs` doc-comment's claim that RoutedLiveCheck and `routed_page_field`
   "agree on what routed means" is false for lambda `view`s until #95 lands.
5. **T2 was marked seal-touching / Opus-review-required and the code sits
   uncommitted.** Confirm the Opus adversarial pass (closed⊋open reject;
   fresh-tail no-leak; recursive-record occurs-check) actually ran before the
   commit lands; if not, run it now — do not fold it into the T8 sweep gate.
6. T8 (E2E sweep gate) is still open — see EXECUTION-ORDER.

### 1.8 `oracle-and-tiered-verification.md` (#51/#110) — NEEDS-FIX (staleness only)

The tiering rule (§6) is coherent, and the other banked designs use it
correctly (routed #108 §9 classifies T1–T3 seal/Opus vs T4–T8 oracle; both
seal designs self-mark Opus-required — consistent with §6.2's "security-seal
properties are absolute, not Go-relative").

**Staleness:** §4 item 1 and §8 item 1 (HTML normalizer not wired; `body` =
raw byte floor) were **overtaken hours after writing** by commit `63f57b2`
("wire HTML normalizer into body-mode oracle comparison (#110)") — verified:
`scripts/lib/checks.sh:283-299` pipes bodies through
`equivalence_normalize_html.py`, and `examples-sweep.sh:93` documents the scenario
fallback to normalized-body diff. Update the doc: `body` mode is promoted to
real-oracle status **provided** the false-green audit obligations
(`go-oracle-fixture-corpus-plan.md` §3 — SVG-coord mask, charref
canonicalisation, CRLF folding) were honoured in the wiring — verify that,
then re-tier: server/live faithful-port slices with green `body` EQUIVALENCE are
Opus-retired. §8's remaining items: pty grid normalizer (open), playwright
stack (open), release-ipe rebuild (open), CI phase-2 flip (open), 65-fixture
corpus (open).

### 1.9 `parity-gap-snapshot.md` — READY as a dated snapshot

Honest, measured, correctly caveated (ipe-0 only; cargo-tier tracked
separately). Already-overtaken rows to note when refreshing:
- **ex-33** triple-quoted-string parser bug: fixed by `e586668` (post-snapshot).
- **#108 caveat rows (09, 25, 34):** implementation now in the working tree;
  they should flip on the next sweep (pending T8).
- **Fix 4 cost:** collapses per §1.6 fix 1 — `Task.perform` is an alias-thin.
  The retry family stays medium.
- **ex-15 (`Handler` head-alias unfold T0004)** has no owning task in any
  banked doc — upstream closed this class via `unfoldHeadAlias`
  (AGENTS.md "Closed in v0.15" — contributor PR #123). File it.

### 1.10 `parallel-lane-plan.md` — STALE, supersede

Written against HEAD `940dd15` with #106/#107 "in flight" — several rounds
ago. Specifics now wrong:
- The sweep-frontier table (§0) disagrees with the authoritative snapshot
  (e.g. it lists 26/29/37 as IPE-P0001 parse failures; the snapshot measures
  26→N0004 `Input`, 29→T0001, 37→N0004 `Region`).
- `error-module-design.md` correction, #98/#47 compiled-source subsystem,
  `Ipe.Live.Head` (round-1 Lane-B class work) — landed.
- Round-2/3 sketch items partially done: #51 oracle activation (done, §1.8),
  `-cents` prefix-neg (done, #114 `1806aa2`).

**Carry forward (still valid, extract into the next plan):**
- The cluster-ownership protocol ("exactly one lane holds
  `constrain.rs`/`lower.rs`/`sky_kernels`/backend at a time") and the
  crate-level disjointness proof method.
- §6's rejection rationale: **#104/#99/#94/#95 may never run parallel to other
  cluster work** — all edit `sky_backend_rust`/`sky_lower`. With #90 added,
  the entire seal queue (§EXECUTION-ORDER) is serialized in Lane A by
  construction.
- The df-abort-guard + cold-target-2 recipe for Lane B.

### 1.11 `docs/ideas/idea-7-effect-do-block-design.md` — READY

Internally consistent, correctly deferred post-parity, correctly classified as
a syntactic divergence (not oracle-verifiable at source level → guardian tier
when built). No conflicts with the banked designs. Two consistency notes:
- Its §4 "`perform` collides with `Cmd.perform` / `Task.perform`" is
  compatible with §1.6's correction (`Task.perform` = `Task.run` alias): the
  collision argument survives — even stronger, since `Task.perform` *runs*
  a task synchronously.
- Its §8 auto-force retirement will eventually interact with #111's effect
  modules (CLI/`Task.run` boundary); note it in #111's design when written.

### 1.12 `docs/divergences-from-sky.md` — NEEDS-FIX (2 edits)

The ledger is in good shape overall (B15 resolution, A15–A17 additions, and
the pending-impl honesty on L0121 are exemplary). Required edits:
1. **B-route-param:** "to be upgraded to a proper diagnostic code `IPE-L0121`
   in a follow-up task" → **`IPE-L0123`** (C1). Also move the entry from its
   current position (appended after the "Could not confirm / verify" section)
   up into §2 with a proper `B18` number, and bump the §Counts.
2. **B16 (#104 last-use):** the entry describes clone-all-but-last as if
   universally applicable. When #104's Clone-predicate amendment (§1.1 fix 1)
   lands, extend B16: non-Clone-renderable carriers (fn-embedding, Task/Cmd/
   Sub/Decoder/Db) are *diagnosed* on multi-consuming-use (IPE-L0122), never
   cloned — still strictly better than the reference, which has no gate at all.
3. (No edit needed, recorded for the executor) A15's L0121 claim is the
   canonical one and wins C1.

---

## 2. CONFLICTS — decisions the executor must follow

### C1 — IPE-L0121 is claimed by THREE designs. Assignment is final as follows.

| Code | Owner | Claimants displaced |
|---|---|---|
| **IPE-L0121** | `InadmissibleAppMsg` (#94 Msg gate) | — (strongest claim: ledger A15 + README-liftable table + seal-gates design all already cross-reference it) |
| **IPE-L0122** | `Feature::FunctionValueReuse` (#90 Stage 1 step 4) | was "L0121, next free slot after L0120" in `ctor-payload-function-design.md` |
| **IPE-L0123** | Route `:param` payload rejection (#108 follow-up, upgrades `route_param_get`'s `CompilerBug`) | was "IPE-L0119" in `routed-live-app-design.md` §6 (L0119 is taken by the cfg-literal diagnostic, `code.rs:303`) and "IPE-L0121" in the ledger's B-route-param |
| IPE-L0124 | next free — reserved for #89 if it needs one | |

Rationale: minimize renumbering of already-cross-referenced docs; L0121-as-Msg
appears in two committed documents. Each landing PR must grep
`docs/ + crates/sky_diagnostics` for its code before minting (make this a
standing rule — this collision happened because three designs each took
"next free after L0120" in parallel).

### C2 — #90's gate-lift vs #94's Msg gate: WHO gates `Msg = … | Wrap (Ok fn)`, and in what order?

**Decision:** #94's `check_admissible_msg` (predicate `ir_type_is_derivable`,
which recurses `Fun → false` through `Result`/`Maybe`/`List`/`Dict`/user-enum
carriers) is the sole gate for fn-carrying **Msg** payloads at app entry. #90's
construction lift does not weaken it — construction admissibility and
app-entry admissibility are different judgments and both designs agree on the
split. **But the ordering is load-bearing:** today `type Msg = WithK (Int ->
Int)` is unrepresentable (killed at declaration by `lower_enum`'s
`ir_contains_fun`, `lower.rs:1581`). #90 T2 deletes that gate. If #90 lands
first, an fn-carrying Msg compiles through `ipe` and dies at cargo on the
`Msg: Clone` bound — a seal regression introduced by #90.
**Execution rule: #94 (+#95) lands strictly before #90 Stage 1.** #90's §4
checklist gains a Msg row; #94's fixture 2 (`LIVE_FUNC_MSG`) becomes the
post-#90 regression proving the ordering held.

### C3 — #104's last-use clone pass vs #89's E0382 class vs #90's reuse gate: one liveness engine.

**Decision:** there is exactly **one** body-liveness analysis, owned by #104,
with two consumers:
- Clone-renderable non-`Copy` bindings → clone-all-but-last (the #104 rule).
- Non-Clone-renderable bindings (fn-carrying, Task/Cmd/Sub/Decoder/Db) →
  >1 consuming use = **IPE-L0122 diagnostic** (the #90 interim gate's
  semantics, absorbed into the general pass when #104 lands; #90's standalone
  counter is an explicitly temporary bridge).
#99 stays a separate local pattern fix (the design's §8 two-pass verdict is
ratified). #89, when its design lands, must first check whether its moved-value
sites are `Expr::Var` reads (covered by #104's pass — reuse it) or a distinct
emission seam (justify a separate mechanism explicitly). No third clone pass.

### C4 — #94/#95's `fn_param_ty` vs #108's routed detection: SHARED machinery, and a live hole until then.

**Decision:** they already share machinery — `emit_live.rs:374
routed_page_field` delegates to `emit_model_gate::model_ty_of_view` —
so #95's single edit (re-express as Lambda-aware `fn_param_ty`) fixes **three**
consumers at once: the Model gate (#95 proper), the Msg gate recovery (#94),
and #108's routed/non-routed emit branch. Until #95 lands, a routed app with a
lambda `view` **type-checks as routed (RoutedLiveCheck reads the solver's
Model) but emits the non-routed `live_app`** — routes silently dead at
runtime. This cross-tier disagreement is the sharpest finding of this review:
it is not a cargo-fail, so no seal gate catches it; only a runtime/scenario
check would. Required: the `LIVE_LAMBDA_VIEW_ROUTED` fixture (§1.2) + a
consistency test asserting the type-tier and emit-tier routed predicates agree
on every app fixture. This also raises #94/#95's priority (see
EXECUTION-ORDER): #95 is now a functional-correctness fix for shipped #108
behavior, not just a gate hardening.

### C5 — Kernel backlog's `Task.perform` vs the "#116 entry-contract".

**Decision:** `Task.perform` is a legacy alias of `Task.run`
(`Task e a -> Result e a`, upstream `Task.ipe:85-95`) — a synchronous
Result-returning runner with **no** Cmd/dispatch semantics. Registering it as
an alias of the existing `TaskRun` kernel touches no entry contract. The
Cmd-side dispatch (`Cmd.perform : Task e a -> (Result e a -> msg) -> Cmd msg`)
remains #111/M6 scope. **#116 has no in-repo artifact** — before #111's design
is written, the #116 owner must document the entry contract or cede the
number; #111 must not silently define it.

### C6 — Seal-tiering consistency across the set.

All banked designs classify correctly per `oracle-and-tiered-verification.md`
§6: #90/#94/#95/#99/#104 and #108-T1–T3 are marked seal-touching/Opus;
#108-T4–T8 oracle-verifiable. One caveat: #108 §9 calls T5's emit
"byte-comparable to the reference Rust backend" — that reference is **not
buildable on this host** (oracle doc §1), so T5's practical oracle is
run-vs-Go via the sweep. With #110's normalizer landed, `body`-mode live
examples are now genuinely oracle-coverable; the routed T8 gate should use it.
And per §1.7 fix 5: confirm the T2 Opus review actually happened before the
working-tree #108 code is committed.

---

## 3. EXECUTION-ORDER recommendation (Lane A queue)

All items below touch the cluster (`sky_types`/`sky_lower`/`sky_backend_rust`/
`sky_kernels`) and are **serialized in one lane** (parallel-lane-plan §6
rationale, still valid). Doc lane items run concurrently.

1. **#108 close-out** — commit the working tree behind: (a) Opus sign-off on
   T1–T3 if not yet done (C6); (b) the routed-lambda consistency fixture
   *even if red* (documents C4's hole); (c) file the #56 step-4 re-binding
   sub-item + code comment (§1.7 fix 3); (d) run T8 sweep gate. Cheapest path
   to 3 more ipe-0 examples (09, 25, 34).
2. **Kernel Tier-1 batch** (cheap, high example yield, keeps ipe-0 count
   moving while seal reviews queue): `Time.timeString`, `Cmd.publish` +
   `publishNoEcho` + `Sub.subscribeTopic` (ALL-wiring), `Font.lineThrough`,
   `Time.isLeapYear`/`daysInMonth`, **`Task.perform` → `TaskRun` alias**
   (§1.6 fix 1), **`List.filterMap`** (§1.6 fix 3). Unblocks 02, 23, 24, 27,
   `simple`, moves 00 and 16 forward. ~1 day total.
3. **#94 + #95** (one PR series; A2 `fn_param_ty` first — closes #95, hardens
   #94's recovery, and fixes #108's silent-unrouted hole per C4). Mints
   IPE-L0121.
4. **#99** (self-contained pattern-renderer fix; smaller than #104; unblocks
   the as-pattern corpus).
5. **#90 Stage 1** (after #94 per C2). Mints IPE-L0122.
6. **#104** (after the §1.1 Clone-predicate amendment is written into the
   design; absorbs #90's reuse gate per C3). Then file/execute **#104b**.
7. **Region + Input registration** (parity Fix 2, 4 examples) — can interleave
   anywhere in 2–6 as a cluster round; Region is Tier-2-thin, Input is medium.

**Doc lane, concurrent:** (a) write `effect-modules-kernel-plan.md` (#111) —
the largest unblock has no spec (§1.5); (b) finish `seal-jsondecp-design.md`
(#89) honoring C3; (c) apply the ledger edits (§1.12) and the design-doc
amendments (§1.1 fix 1, §1.2 items, §1.4 items, §1.7 items); (d) refresh
`oracle-and-tiered-verification.md` §4/§8 post-#110; (e) supersede
`parallel-lane-plan.md`.

**Priority sanity vs the parity snapshot:** the snapshot's Fix-1 (#111,
5 examples) cannot execute without its design — hence it heads the *doc* lane,
not the build lane. With the `Task.perform` correction, the recomputed
cheapest-first build order is: #108 close-out (3) → Tier-1 batch (5+) →
Region/Input (4) → #111 (5, once designed) — while the seal queue
(#94/#95 → #99 → #90 → #104) interleaves as the review-gated track protecting
the exit-0 ⇒ cargo-0 invariant that all the ipe-0 wins ultimately rest on.
Nothing in the banked set is mis-ordered beyond the corrections above; the one
genuine inversion found is that #95 was tiered as "gate hardening" but is in
fact a **functional fix for already-landed #108 behavior** (C4) and moves up
accordingly.
