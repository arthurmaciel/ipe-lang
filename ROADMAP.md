# ipê — Project Roadmap

This document enumerates the remaining and future work for the ipê
compiler, backend, and runtime, in priority order, together with the
**done ledger** (dated landed milestones and major items). It is the
durable plan of record: finish the compiler + backend + runtime first,
then the parked FFI subsystem, then the post-completion program, then
the longer-horizon standing work.

**Pending work is mirrored in [`BACKLOG.md`](BACKLOG.md)** — the flat,
pending-only SSOT table the progressive-development loop consumes. The
eight canonical road-map phase names used in both files are defined
here: **Sweep to green** · **Security hardening** · **CI, oracle &
publish** · **Hardening follow-ups** · **FFI** · **Post-completion** ·
**Longer-horizon** · **Designed targets**. Sections A (phases 1–4),
B (FFI), C (Post-completion), D (Longer-horizon), and E (Designed
targets) below carry the same rows, with a `Done at` column recording
what has already landed.

**Principles order.** security > correctness > soundness > efficiency
> completeness > readability. Every decision below is resolved in
favour of the earlier principle when two conflict.

**Two fundamental design rules** govern all work:

- **Parse, don't validate** — turn unstructured input into precise
  types at the boundary; never re-check the same invariant downstream.
- **Make invalid states unrepresentable** — encode invariants in the
  type system so illegal configurations cannot be constructed.

**How work runs (velocity model, condensed).** Rigour and speed
conflict only if you serialize the wrong thing. Each round lands a
short sequential CORE step (shared-contract additions: IR variants,
diagnostic codes, kernel-registry rows), then fans out parallel agents
with **disjoint file sets only** (hard rule — no shared file is edited
by two agents in a round), each behind the full non-negotiable gate
(guardian review, behavioural parity vs the Go reference, clippy-hardest
+ tests + fmt), then merges serially through the same gate. The gates
are never skipped to save time; speed comes from partitioning, not from
relaxing rigour. Mechanical, reference-backed items are additionally
eligible for the autonomous progressive-development loop (rows tagged
`[progdev-safe]` in `BACKLOG.md`).

---

## Done ledger — milestone ladder (M0–M6)

| Done at | Milestone |
|---|---|
| 2026-06-26 | **M0 — spine**: ADT + `case` + a kernel + `println`, end-to-end, runs |
| 2026-06-27 | **M1 — core language**: `let…in`, lambdas + first-class functions, `if`/multi-way `if`, tuples, full binop set, records + access + update, type aliases |
| 2026-06-28 → 06-29 | **M2 — polymorphism**: type variables end-to-end, generic functions, same-module re-instantiation, wildcard-`any` soundness gate, parametric type aliases, float literals + exponent parity |
| 2026-06-29 → 06-30 | **M3 — full ADTs & patterns**: non-nullary constructors, nested/cons/tuple/record/literal/alias/wildcard patterns, exhaustiveness (Maranget) |
| 2026-06-30 → 07-06 | **M4 — stdlib breadth via the kernel registry**: List/Maybe/Result/Dict/Set/String/Math/Char/Decimal/… as registry rows + Rust runtime mirrors + parity tests |
| 2026-07-03 → 07-05 | **M5 — effects & runtime**: Task everywhere (incl. `Task.retryWith`), Cmd/Sub, Http, File, System, Process, Db, Crypto, Time, Random — mirroring `runtime-go` module-for-module |
| 2026-07-04 → ongoing | **M6 — app shapes (partial)**: Sky.Http.Server, Sky.Live (routed apps, SSE, forms), Sky.Tui, Cli — driven by the example-sweep front; Webview pending |

## Done ledger — major landed items

| Done at | Item |
|---|---|
| 2026-07-02 | #50 crate-spec SSOT; #52 float sci-notation pinned to Go `%v` (exp ≥ 6, probed vs Go 1.26.2); #48 let-bound-cfg diagnostic |
| 2026-07-03 | F7 CSS/attribute emission injection-safe by construction; #55a Encoding text codecs → UTF-8 (Go parity); #96 lambda/function parameter patterns (SKY-L0105 retired); auto-TCO (typed `TailRecur`/`TailLoop` IR → Rust `loop`) |
| 2026-07-04 | #89 JsonDecP seal (curry-wrap succeed, DbDecSucceed naming, Decoder thunk-rewrite); #111 effect modules (Auth + ServerStream + HttpStream); #95 lambda-view seal gate design landed with RoutedLiveApp (#108, closes #56 core) |
| 2026-07-05 | #121 curried FuncValue arity-exact invariant (T1–T6 + SKY-L0125); #94 Msg-admissibility gate (SKY-L0125); #135 WS Ping heartbeat (B20 closed); #143 Appendable `++` super-type — `++` accepts both `String` and `List` operands (closes the former "`++` is String-only" parity gap) |
| 2026-07-06 | #66 well-typed no-panic fuzzer + #66-N first half (ill-typed rejection fuzzer, 0 false-acceptances); #154 misplaced-span kernels wired; Border.shadow/glow/innerShadow kernels; SKY-I0001 interp-literal ICE fixed (`crates/skyc/tests/interp_literal.rs`) |
| 2026-07-09 | #85 Error rich-ADT core (`Error ErrorKind ErrorInfo`, 69-golden `SkyError` flip) + #160 Error ctor-scheme; #71 explain_lookup (closed by AUD-15); AUD-01..08 + AUD-10..15 hardening (14 confirmed audit findings, 13 landed — see `docs/architecture/principles-audit-2026-07-09.md`); class-1 inference bug #1 (integer-literal monomorphization) fixed + gate-verified; #82 record type-alias auto-constructor (SKY-N0001); ex27/ex37 erased-`any` ctor payload pinned to `Dict String String` (`pin_any_in_ty`, divergence B-AnyCtorPayload) |
| 2026-07-10 | #109/#156 `Ui.onSubmit`/`Std.Html.Events.onSubmit` dispatch via `Event::OnForm`; `Arc<dyn Any>` OnRaw removed — zero `dyn Any` in emitted-code paths; #63 `Sky.Http.Middleware.withCsrf` — double-submit CSRF for Sky.Http.Server + `ServerResponse` multi-`Set-Cookie` fix + a same-day well-formedness hardening fix from independent review; Class-8 web remainder (4 AUD-09 items); #45 kernel-scheme exhaustiveness gate; Boundary Scheme Promotion (class-1 inference bug #2) — landed, reverted after independent review found a SEAL violation, re-landed with the exact gap closed and re-verified against the failing shape via real `cargo build`, held under a SECOND independent review with no new finding (`docs/divergences-from-sky.md` B23). #90 ctor-payload-function lift did NOT land — 3 same-day incidents on the same sub-feature (T3 curried-`andMap`), each independent review reproducing a different bypass; needs a real design pass, not another mechanical fix (see `BACKLOG.md`'s `#90` row). |

Closed-as-obsolete / not-a-bug: #75 (Color reservation — superseded by
home-aware type resolution + `Std.Css`'s own `type Color`), #122 (Cli
view printer — reference-correct as-is), #157 (Jwt builder API — was
already fully wired).

---

## A. Critical path — compiler + backend + runtime to completion

**DONE gate.** Completion is defined by the example sweep: every
non-Go-only example passes the GitHub example sweep with all three
checks green — **build ✓, run ✓, equivalent-to-Go-reference ✓.**

The critical path decomposes into four phases: **Sweep to green**
(close every per-example blocker), **Security hardening** (the
pre-push security tier — never deferred past the push), **CI, oracle &
publish** (port the sweep, activate the oracle, fix CI, rename, push),
and **Hardening follow-ups** (correctness/efficiency debts that don't
block the sweep). The #59 rename runs **solo, dead-last before the
push** (per the Tier-1 chain: sweep-green → seal → #110 → #37 → #59 →
push).

Standing precondition: reclaim build-cache / disk headroom before
heavy local builds — a near-full disk fails a build mid-run as
`ENOSPC` *after* type-check and codegen succeed and masquerades as a
codegen regression.

### A-table — Sweep to green

| Done at | Priority | Road map phase | Task | Notes | Spec |
|---|---|---|---|---|---|
| 2026-07-10 | Critical | Sweep to green | Boundary Scheme Promotion — fix untyped top-level bindings sharing ONE monomorphic var across the linked program (class-1 inference bug #2) | Re-landed after a same-day revert (independent review found E0283 on a cross-module field-access getter — `promote_untyped_boundaries`'s `obligation_roots` missed the field-access's own result var). Re-fixed (inserted `fa.result` into `obligation_roots`; ported the typed arm's `used_generics` filter into the untyped arm as defense-in-depth) and re-verified against the EXACT failing shape via a real `cargo build`+`run` golden, not just a `sky_types` unit test. Held under a SECOND independent review — no new SEAL violation found (see the `RecordUpdate.fields` follow-up row below for the one non-exploitable completeness gap it surfaced). See `docs/divergences-from-sky.md` B23. | `docs/architecture/class1-inference-fix-spec-2026-07-09.md` |
| | Medium | Sweep to green | Boundary Scheme Promotion follow-up — add multi-module fuzz templates (cross-module 2-type reuse, value binding, Number-bounded helper, recursive pair) to `scripts/fuzz-well-typed.sh` / `fuzz-ill-typed.sh`, which today only emit single-file `Main.sky` projects | [progdev-safe] Core fix landed 2026-07-10 (`crates/sky_types/src/constrain.rs` + `lib.rs`, lowering in `crates/sky_lower/src/lower.rs`) — this row is ONLY the fuzzer-harness extension the original spec listed as a pre-landing nice-to-have; it needs new multi-file-template infrastructure in the fuzz scripts (today every template writes a single `src/Main.sky`), not just a new template function. | `docs/architecture/class1-inference-fix-spec-2026-07-09.md` |
| | Low | Sweep to green | Boundary Scheme Promotion — `obligation_roots` (`crates/sky_types/src/constrain.rs`) has a symmetric gap for `RecordUpdate.fields`' per-field VALUE vars (analogous to the `fa.result` gap that was just fixed for field access) — `promote_untyped_boundaries`'s `record_updates` loop only inserts `ru.record`, never each field's value var. NOT currently exploitable (the `used_generics` defense-in-depth filter independently strips the resulting stale generic — empirically verified via a real `cargo build` of a `setName r n = { r \| name = n }` repro, which compiles and runs correctly today), but it's a real completeness gap in the primary obligation-exclusion mechanism, not just the backstop. | Found by independent second-pass review of the Boundary Scheme Promotion re-fix (2026-07-10) — same review also confirmed the re-fix itself holds (no new SEAL violation found). | `docs/architecture/class1-inference-fix-spec-2026-07-09.md` |
| 2026-07-11 | High | Sweep to green | #90 SKY-L0114 ctor-payload-function — `Ok`/`Just` holding a function is rejected, making `Result.andMap` / `Maybe.andMap` unusable | **Landed on the 5th attempt** (merged `0e4eac0`; commits `cd9bb1c` attempt-4 base reapply + `d8f5814` fail-closed core + `6daa2f1` error-slot pin). The 4-incident history is in the done-ledger + `ctor-payload-andmap-arity-gate-design.md`. 5th-attempt core: `emitted_bound_satisfied`'s `and_map_payload` arm now fails CLOSED on a bare `Ty::Var` (matching every sibling obligation and `Math.min`'s `ord`, verified differentially), accepting the conservative cost — a legitimately-arity-1 ANNOTATED double forwarder is conservatively rejected (pinned by fixture, mirroring Math.min's own behavior). Gating the salvaged work found + fixed one more positive-path seal hole: the pipe's eta-param annotation pinned a free Result-error var to `JsonVal` while `ok_res` pins `SkyError` — `ir_type_from_ty_json`'s Result arm now pins a free ERROR-slot var to `IrType::Error` (one defaulting policy, both sides). Adversarial review (the 5th, after 4 that each broke prior attempts): **CLEAN** — 20+ fresh fixtures including 6 novel shapes (quadruple forwarder, record-field-stored fn, if-selected, type-alias-annotated forwarder, List.map lambda, a 3-module safe/hazard split targeting the generalization boundary), every curried shape cleanly rejected at skyc time, every positive path builds+runs correct output incl. concrete `Result String/MyErr/Int` error types (the pin didn't overreach). 32/32 l0114 fixture suite under SKY_E2E. 2 findings filed as their own rows (a PRE-EXISTING let-bound-ctor-closure E0308 hole bisected to the Stage-1 base, and a diagnostic-phrasing nit). | `docs/architecture/ctor-payload-function-design.md`, `docs/architecture/ctor-payload-andmap-arity-gate-design.md` |
| | High | Sweep to green | #158 Nested-constructor-payload function-argument patterns (`f (Just (h :: t)) = …`, `f (Ok {name}) = …`) are fail-closed (SKY-L0112/SKY-L0116) where the reference recurses and compiles | Correct fail-closed behavior; the completeness gap itself is the item (divergence A13). | `docs/architecture/class4-pattern-lowering-fix-spec-2026-07-09.md` |
| 2026-07-11 | High | Sweep to green | #99 Refutable match-arm alias over non-Copy payload double-moves — `case m of Just ((a,b) as w) -> use a,b,w` → E0382 | Landed (`67130c0`), spec followed exactly with 2 documented deviations: (1) the fail-closed gate lives in `lower_case` over the assembled arm set, mirroring the emitter's per-match/per-column STR/LIST/WHOLE mode decision — the spec's per-pattern placement couldn't see the mode and would have over-rejected sound by-ref LIST/STR aliases; (2) the diagnostic is SKY-L0128 (SKY-L0127 was taken by #90's T4 gate after the spec was written). New `sky_ir::is_dispatch_free` shared predicate; backend `render_arm_pat_alias_safe` extends the #96 clone-rebuild strategy to by-value match arms (builtin Maybe/Result payloads — the concrete repro's path — user-enum payloads, and whole-arm heads); dispatch-NEEDING alias inners fail closed with an actionable explain page. Green fixture proves a/b/w all read correctly from real compiled Rust under SKY_E2E; red fixture pins the clean SKY-L0128 rejection; the `m3b3_alias` byte-golden regenerated after its go-parity E2E confirmed identical output; the unit test that had PINNED the unsound `y @ x` emission now asserts the clone-rebuild form. | `docs/architecture/seal-noncopy-move-design.md` §4.2 + `docs/architecture/class5-emitter-clone-fix-spec-2026-07-09.md` |
| 2026-07-11 | High | Sweep to green | #125 Decoder thunk coverage: tuple-destructure + record-field binders (loud E0382) | Landed. #89 fixed the bare-`PVar` `Decoder` reuse (thunk-wrap RHS + rewrite each read to a zero-arg thunk call); #125 generalizes to tuple/record destructure binders: when the aggregate value type contains `IrType::Decoder` anywhere (`ir_type_contains_decoder`), the whole destructure is thunked and each read re-destructures a fresh MASKED copy of the pattern (`mask_pattern_except` — every other bound name erased to wildcard) from a fresh thunk call, using `Expr::Destructure` itself as the projector (no new IR node). Sound for the same reason as #89: Decoders are pure builders, so re-evaluation is construction-cost-only. §2.5 refactor (extract `rewrite_var_free_occurrences`, verified byte-identical against the m4h goldens) landed as its own step-1 commit. Wired into BOTH `lower_let` and `lower_case`'s single-arm destructure. 3 golden fixtures (tuple/record/case reuse, exact stdout under SKY_E2E) + a byte-identity guard proving the non-Decoder fast path is untouched. | `docs/architecture/class5-emitter-clone-fix-spec-2026-07-09.md` |
| 2026-07-11 | Medium | Sweep to green | #102 F1 shadow diagnostic: local `type X` shadowing a dep-imported `X` → downstream SKY-T0001 instead of clean SKY-N0012 at the decl | Landed (`a980804`, spec Item D): a dep-shadow pre-pass in `canonicalise_with_env` (`crates/sky_canon/src/resolve.rs`) mirrors `inject_dep_type`'s dep-vs-dep clash check — a local union/alias name already present in `type_home_map` under a different home is rejected with the existing SKY-N0012 (`DuplicateType`) AT THE DECL. Reads-only pre-pass (writes stay in the following loops), unions-then-aliases before either mutates the map. No new diagnostic code. Same-module duplicates keep their better first-declared span. Golden negative fixture + 4 canon unit tests (local union/alias shadow, no-import control, same-module-still-first-span). | `docs/architecture/class4-pattern-lowering-fix-spec-2026-07-09.md` |
| 2026-07-11 | High | Sweep to green | #113 Pseudo-class attrs render to nothing in the static `htmlRender` sink (AttrPseudoRule no-op) | Encoder fix landed with the 2026-07-10 Std.Ui/Html kernel batch (`collect_html_attrs` harvests every `AttrPseudoRule` into one `||`-joined `data-sky-pc-rules` marker; `PseudoClass::wire_tag` colocated with the type, lock-step with `style_inject`'s decode side) — affected every backend, not just the static sink, exactly as the spec predicted. Closed out 2026-07-11 by adding the spec's two remaining tests: the multi-rule merge (exactly ONE marker per element) and the full composed pipeline (`AttrPseudoRule` → `ui_layout` → `assign_sky_ids` → `apply_style_injections` → `render_html` → scoped `<style>` with the `@media (hover: hover)` guard, no marker leak) — the gap the kernel-batch review flagged. | `docs/architecture/class10-ui-html-fix-spec-2026-07-09.md` |
| 2026-07-11 | Medium | Sweep to green | #105 Std.Css hardening (defence-in-depth): @import/expression gating on `raw`/`keyframes` bodies + reject CSS-hex-escaped values in `safeValue` | Both parts landed (`917f79c`). Part 1 (pure Sky): `raw`/`keyframes` drop the whole rule on an `@import` (CSS-level SSRF) / `expression(` smuggle (`keyframes` scans the joined frames — a bare-named-fn `List.any` would trip the first-class-fn Clone limitation; joining only over-detects, fail-safe). Part 2 (`css_safety.rs`): `SafeCssValue` now decodes CSS backslash-hex escapes (`\65 xpression(...)` → `expression(...)`, `\3b` → `;`) and re-runs the shared breakout scan against the decoded form — closing the hidden-keyword bypass; scope is precisely `SafeCssValue` (`SafeCssPropertyName`/`Selector` already reject `\`). Std.Css security golden extended with the `@import` vectors (dropped end-to-end under SKY_E2E) + benign-keyframes non-regression. | `docs/architecture/class10-ui-html-fix-spec-2026-07-09.md` |
| | High | Sweep to green | #32 M5a follow-ups (fail-closed): Task arity-3 ICE + Task-in-ADT-ctor gate | | `docs/architecture/class4-pattern-lowering-fix-spec-2026-07-09.md` |
| 2026-07-10 | High | Sweep to green | #56 Prove row-poly subset/superset record resolution (A7 watch) + gate on sweep | Investigated 2026-07-10: no defect found, all reachable subset/superset shapes resolve or fail-loud in parity with the reference. 5 golden-test fixtures wired into `crates/skyc/tests/golden_row_poly_records.rs` (purely additive, zero compiler code touched) pinning the invariant on the sweep: 2 accept (subset field access, subset pattern matching incl. `case` + `List.map` lambda), 3 reject (closed-superset mismatch SKY-T0001, two-incompatible-superset-instantiations-of-a-local-let SKY-T0001, the pre-existing `#56b` annotation-syntax-gap canary SKY-P0001). Independent review verified all 5 fire for the right reason (not just green) by re-deriving both reject cases from source, and ran the `SKY_E2E` real-build tier (7/7 pass) — found one moderate doc-accuracy issue: the two-supersets fixture's "class-1 coupling tripwire" framing overstated its coverage (it exercises `unify.rs`'s ordinary closed-record-mismatch rule via the no-let-polymorphism path, not `promote_untyped_boundaries`, which only generalizes module-level bindings — no cross-module two-superset fixture exists yet) — corrected in the same pass. | `docs/architecture/row-poly-subset-superset-design.md` |
| 2026-07-10 | High | Sweep to green | #45 Make the constrain kernel-scheme table exhaustive over canon lists (close the exit-0-then-cargo-fail class) | Extended `canon_equals_registry`'s G1 reverse loop (`crates/sky_canon/src/lib.rs`) into a full subset gate over `qual_vars`; two exception mechanisms (`excluded_quals`, member-granular `deliberately_unbacked_members`) keep it from blinding whole modules. Surfaced 20 previously-undocumented unbacked kernels — see the Std.Ui/Std.Html kernel-gaps row below. | `docs/architecture/class3-kernel-registry-fix-spec-2026-07-09.md` + `docs/architecture/html-ui-live-scheme-table.md` + `docs/superpowers/plans/2026-07-03-registry-phase-E.md` |
| 2026-07-10 | High | Sweep to green | #70 Fix kernel arity-table drift (`decl().arity` vs `callee_arity`) — latent exit-0-then-cargo-fail | Found and fixed real drift across ~20 kernels (`Pure.*` companions like `IoReadLine`/`TimeNow`/`SystemArgs`, several `Db.*ById`/`*ByField` variants, TEA `Cmd`/`PubSub.publish`, `Middleware`/`RateLimit` variants) whose `decl().arity` disagreed with the actual call-site arity. Added `callee_arity_matches_decl_arity`, a machine-checked exhaustiveness test (same pattern as #45's `canon_equals_registry`) asserting the two never diverge for any `StdlibKernel` variant, so a future addition that gets this wrong fails the gate immediately. | `docs/architecture/class3-kernel-registry-fix-spec-2026-07-09.md` |
| 2026-07-10 | Medium | Sweep to green | #85 ErrorDetails follow-up: port `ErrorInfo.details : Maybe ErrorDetails` + the 5-variant `ErrorDetails`/`PanicInfo`/`TypeInfo` union | Landed: same registration recipe as `ErrorKind`, verified arm-for-arm identical across all 6 subsystems by independent review (canon ctor registration, constrain ctor schemes, lowerer arms, `IrType` leaf + derivable/serde, `builtin_runtime_enum` + all 4 backend walker arms). New `Error.withDetails : ErrorDetails -> Error -> Error` kernel is the sanctioned attach-details path — fully sound, `SKY_E2E` golden round-trips 3 of the 5 variants through real `cargo build` + run. Dropped `#[derive(Eq)]` on `SkyError`/`SkyErrorInfo` (forced by the new `SkyMaybe`-typed `details` field, which is `PartialEq`-only) — confirmed nothing in the runtime relied on `Eq`/`Ord` for either type. Independent review found one pre-existing (not introduced here, traced to the original ErrorKind/ErrorInfo pass) accept-then-cargo-fail gap on DIRECT record-literal construction of `PanicInfo`/`TypeInfo`/`ErrorInfo` — filed as its own row below per no-deferral; does not affect the sanctioned `Error.withDetails` path this item ships. | `docs/architecture/error-module-design.md` |
| | Medium | Sweep to green | `PanicInfo`/`TypeInfo`/`ErrorInfo` raw record-literal construction is a well-typed Sky program that `skyc` accepts but whose emitted Rust fails `cargo build` | Found by independent review of #85 (2026-07-10) — pre-existing, widened but not introduced by #85. See the matching BACKLOG.md row for full root-cause + remediation shapes. | `docs/architecture/error-module-design.md` |
| 2026-07-10 | High | Sweep to green | Remaining Std.Ui / Std.Html kernel gaps surfaced by the example sweep — wire missing kernels across the layers with the `../sky` reference | 19 of 20 unbacked kernels landed: `Ui.image/disabled/paddingEach/clipX/clipY/scrollbarX/scrollbarY/onFile/onPseudo/hover/focus/focusVisible/active`, `Html.toString/voidNode/doctype/titleNode/htmlNode/headNode`, `Background.linearGradient` — each wired through the full 8-file recipe. Found and fixed a pre-existing latent bug while wiring `Ui.onPseudo`: the sky-id-scoped pseudo-class `<style>` injection pipeline (`live/style_inject.rs`) existed and was tested, but nothing produced its `data-sky-pc-rules` marker input, so the already-shipped `Background/Border/Font.hoverColor/focusColor/activeColor/disabledColor` kernels were silently rendering as no-ops — closed in `render.rs::collect_html_attrs`. Also fixed 2 pre-existing latent mis-arity bugs in `lower_callee`'s legacy-match table (5 `Html.*` + 4 `Ui` clip/scrollbar names mapped onto the wrong generic kernel) — confirmed harmless pre-fix (type-check already fail-closed via `deliberately_unbacked_members`, lowering was unreachable). `Ui.mediaQuery` deferred — see the new row below (needs a genuinely new CSS media-query emission mechanism, not a same-shape wiring job). Independent review (2026-07-10) confirmed the pseudo-class fix, exhaustiveness gates, and the golden E2E all clean; found one pre-existing (non-regression, upstream-matched) CSS-escaping gap on `Ui.onPseudo` — filed as its own Security-hardening row below. | `docs/architecture/ui-html-completeness-design.md` |
| | Medium | Sweep to green | `Ui.mediaQuery` — the 1 of 20 unbacked kernels deliberately deferred from the 2026-07-10 Std.Ui/Html batch | Needs a genuinely new wrapper-Element CSS media-query emission mechanism that doesn't exist for any kernel yet (`Ui.breakpoint`, the closest analog, is itself only a documented Phase-0 eager-passthrough stub — no real CSS emission). | `docs/architecture/ui-html-completeness-design.md` |

### A-table — Security hardening

| Done at | Priority | Road map phase | Task | Notes | Spec |
|---|---|---|---|---|---|
| 2026-07-10 | High | Security hardening | #44 Opaque `Secret` stdlib type (gates WASM hydration island + secrets-are-typed rule) | Landed: sealed newtype (`Sky.Core.Secret`, `fromString`/`reveal`/`redacted` kernels) following the `SqlFragment` (#61) opaque-newtype convention — exactly one hand-written constant-time `PartialEq` (`subtle::ConstantTimeEq`, no early-exit byte compare) and one hand-written always-redacting `SkyStringify`/`Debug`; deliberately no `Display`/`Hash`/`Ord`/serde. Non-serde is load-bearing: `ir_type_is_serde` is exhaustive and recursive through Record/Maybe/List/Set/Tuple/Result/Dict/enum-payloads, so a `Std.Live` Model field of type `Secret` (or any compound containing one) is a compile-time `SKY-L0120`, not a session-store leak. Backing buffer zeroizes on `Drop` via the `zeroize` crate on the in-place buffer (not a moved-out copy). `Auth.signToken`/`verifyToken` re-typed `String → Secret`; reveal happens only at the `AUTH_WRAPPERS` FFI boundary, `runtime/src/sky_runtime/auth.rs` internals untouched. Independent review (2026-07-10) verified constant-time eq, exhaustive leak-surface hunt (no `Display`/`Hash`/`Ord`/`From` bypass found), the compound-nesting `SKY-L0120` gate (`Maybe Secret`, nested records), the auth re-typing boundary, and real zeroize-on-drop — all clean. `SKY_E2E=1` golden suite (6 tests: seal/reveal, constant-time eq, record-containing-Secret, log redaction, auth roundtrip) passes via real `cargo build`s. | `docs/architecture/class6-secret-sqlfragment-fix-spec-2026-07-09.md` |
| 2026-07-10 | High | Security hardening | #61 `SqlFragment` param-query newtype — SQL injection = type error | Landed: opaque `SqlFragment` with typed combinators (`column`/`param`/`eq`/`and`/`inList`/etc.); `Db.findWhere`/`Db.deleteWhere` take `SqlFragment`, not `String`; `Db.unsafeFindWhere` removed outright (not deprecated) per the spec's no-deferral decision. Found+fixed one exit-0-then-cargo-fail during its own verification (`sql_column`'s `&str` param, inconsistent with every other Sky-`String`-typed kernel param). Full workspace gate initially failed after merge — 21 new kernels (19 `Sql.*` + `Db.findWhere`/`deleteWhere`) were missing from `lower_callee`'s legacy string-match table (only exercised by the `id=None` fallback path, which lane-c's own targeted tests never hit); fixed centrally post-merge, gate now green. | `docs/architecture/class6-secret-sqlfragment-fix-spec-2026-07-09.md` |
| 2026-07-10 | Medium | Hardening follow-ups | Pre-existing E2E failures `db_crud`/`db_transaction` (+ `m5a` `error_channel`/`task_map_error_lambda`) — TWO unrelated regressions | Fixed both. (A) `db_crud`: `bdbc572` flipped `DbInsertRow`/`DbUpdateById` to `dict(string,string)` (correct) but left the fixture's list-of-tuples literals stale → wrapped in `Dict.fromList`, dropped the now-redundant `.into_iter().collect::<HashMap>()` emitter conversion. (B) `db_transaction`+`m5a`: `5db4cd3` made `SkyError` a real enum, but `K::TaskFail`'s over-polymorphic `fun(var(1), task(var(0)))` still let `Task.fail "str"` HM-check → emitted ill-typed `task_fail(String)`. Pinned the scheme to `fun(error_ty(), task(var(0)))` (matching `mapError`/`onError` + the bundled `stdlib/Sky/Core/Task.sky` annotation), rewrote the 3 fixtures to `Task.fail (Error.unexpected …)`, added a negative SKY-T0001 gate (`Task.fail "str"` now cleanly rejected) + a `docs/divergences-from-sky.md` entry (Ipê's `Error -> Task Error a` is deliberately less permissive than the reference's `e -> Task e a`). All 4 previously-red E2E tests + 2 new negative gates pass under `SKY_E2E=1`. | `docs/architecture/class7-db-crud-transaction-fix-spec-2026-07-10.md` |
| 2026-07-10 | High | Security hardening | Class-8 web remainder: session cookie `Secure` TLS-gated (not ENV-gated), `/_sky/observability/ingest` CSRF exemption, WS upgrade Origin check outside production (CSWSH), `live_max_body_bytes()` `>0` floor | All 4 sub-items landed: session cookie now reflects the real TLS signal (`request_is_https`, `live/mod.rs`) not just `ENV`; ingest endpoint's dev-mode no-token path now rejects cross-origin POSTs (`live/console.rs`); WS upgrade defaults to same-origin outside production instead of allow-all (`server.rs`); `live_max_body_bytes` floor was already correct. The CSRF cookie's OWN TLS-gating was deliberately left out of scope — see `#63c` below. | `docs/architecture/class8-live-http-security-fix-spec-2026-07-09.md` |
| 2026-07-10 | High | Security hardening | #63b `withCsrf`'s golden/E2E coverage never issues a real HTTP request — `golden_m6_middleware_csrf.rs` only checked `skyc`/`cargo build` succeed; `server_e2e.rs` had zero CSRF tests. | Landed: 3 real HTTP-level E2E tests in `server_e2e.rs` — forged POST (no cookie/header) → 403; cookie present + mismatched/missing header → 403 (both sub-cases); legit GET-mints-cookie-then-POST-echoes-it flow → 200 with the wrapped handler's own body. Proves the full 12-site kernel-registry dispatch chain end to end over a real TCP connection, not just compile-level. | `docs/architecture/class8-live-http-security-fix-spec-2026-07-09.md` |
| 2026-07-10 | High | Security hardening | #63c CSRF cookie ENV-vs-TLS `Secure`-gating bug — a forgotten `ENV=production` on an actually-TLS'd deploy shipped the CSRF cookie without `Secure`/host-lock | Landed: `csrf_set_cookie_value` now takes a `request_is_https` bool captured from `ServerRequest.headers` BEFORE the request is moved into the wrapped handler inside `middleware_with_csrf` (the design adaptation the row called for — the session-cookie path had the request available at cookie-set time, this one doesn't), threaded through as a `Copy` bool into the async cookie-stamping block. `Secure` fires when EITHER `production_from_env()` OR `request_is_https` (`X-Forwarded-Proto: https`, only honoured under the `SKY_TRUSTED_PROXY` opt-in — same untrusted-by-default trust gate `build_request` uses for `remoteAddr` and the session cookie's own fix). Cookie NAME stays process-global (identity stability across a session); only the `Secure` ATTRIBUTE became request-scoped. Unit tests cover the full OR-gate truth table + the trust gate; a real HTTP-level E2E through `middleware_with_csrf` proves the signal survives the capture-before-move adaptation. | `docs/architecture/class8-live-http-security-fix-spec-2026-07-09.md` |
| 2026-07-10 | High | Security hardening | Class-7 SQL/DB remainder: `SqlNull` text-typed NULL breaks Postgres, Postgres driver structurally unreachable, `db_insert_row` fabricated `id=0` on non-integer PK, tenant-prefix SQL enforcement absent from plain `db.rs` | All 4 sub-items landed: `SqlParam::Null` now carries a type witness (`Null(Box<SqlParam>)`) instead of binding as text; new `[database] driver` manifest parsing (`crates/skyc/src/project.rs`) threads a `DbDriver` enum through `RustBackend::with_db_driver` → `EmitCtx` → template/Cargo.toml selection, closing the silent-no-op where `driver = "postgres"` never changed the emitted `config.rs` (proven structurally by the new `crates/skyc/tests/postgres_driver_reachability.rs`, no live Postgres needed); `db_insert_fields` gained a `DB_USES_RETURNING_ID` branch + `extract_returning_id` helper so autoincrement ids are read back instead of fabricated as `0`; `runtime/src/sky_runtime/live/hub.rs` gained `tokio::task_local!`-scoped `TENANT_PREFIX` + `reject_cross_tenant_svc`, closing the plain `db.rs` gap in the tenant-prefix SQL-WHERE enforcement (v0.16.6-equivalent guarantee) that `hub.rs`'s reader path already had. Post-merge clippy gate caught 2 issues fixed centrally: a `clippy::doc_markdown` nit (`` `SQLite` `` backticks) and 3 `clippy::expect_used` errors in the new test file's setup helper (`#[allow]`, matching the established per-test-file convention). **Independent review (commit `ac8b2bfc`) then found a real SEAL violation in sub-item 2:** `db_cargo_toml` selected the sqlx driver feature EXCLUSIVELY (`"sqlite"` xor `"postgres"`), but the always-emitted `telemetry_spill.rs`/`live/hub.rs`/`live/store.rs` runtime modules hardcode `sqlx::sqlite::SqlitePool` for local spill/session persistence independent of the app's `[database]` driver choice — a `driver = "postgres"` project built cleanly through `skyc` (exit 0) but its emitted Rust failed `cargo build` with 3 errors once the `sqlite` sqlx feature was dropped. Root-cause fixed same day (commit `b67a857`): the feature selection is now additive (`sqlite` always on, `postgres` added on top), the contradicting unit test assertion was corrected, and a new `SKY_E2E`-gated `postgres_driver_project_cargo_builds` test actually `cargo check`s the emitted Postgres project (closing the coverage gap — the original test only grepped emitted source text, never built it). `cargo nextest run --workspace` + `cargo clippy --workspace --all-targets -D warnings` both green after the fix. | `docs/architecture/class7-sql-db-fix-spec-2026-07-09.md` |
| 2026-07-10 | High | Security hardening | `url_is_cacheable` substring-`contains("memory")` DoS reopen — fixed to parse the `file:`/`sqlite:` scheme + query string structurally instead of substring-matching; independent review then caught a second soundness hole in the fix itself (`"file::memory:"`, SQLite's documented in-memory URI idiom, was misclassified as cacheable, silently pooling distinct private databases) — closed in the same pass | Commit `1d65aa0`; regression tests `url_is_cacheable_*` in `runtime/src/sky_runtime/db.rs`. | `docs/architecture/class7-sql-db-fix-spec-2026-07-09.md` |
| 2026-07-11 | Medium | Security hardening | `Std.Ui`'s inline/pseudo-rule CSS collector did not escape value-as-data attrs (`Font.family` etc.) — a Sky-level `String` reached a `<style>` block via `Ui.onPseudo` defended only by `</style>` stripping, not brace/`;`/`@import` breakout protection | Fixed (`b4cb69c`): 8 raw-string arms in `build_style_string` (Font.family/decoration/align, border-style, overflow, transition, animation, background-image) now route through the SHARED `SafeCssValue` gate, fail-closed drop-on-failure matching the sibling already-gated arms — no new encoder, does not touch `css_safety.rs` (#105's surface). Repro A (page-wide `body{display:none}` via Font.family) and Repro B (`@import` of remote CSS via Background.image) both neutralised end-to-end; legit values (font stacks, `all 200ms ease`, `dashed`) render untouched. 3 unit + 2 end-to-end pseudo-pipeline regression tests. Design pass + implementation both 2026-07-11. | `docs/architecture/ui-css-escaping-fix-spec-2026-07-10.md` |
| | Medium | Security hardening | **DEFERRED (explicit user override, 2026-07-10) — not blocking this campaign.** #66-T2 Type-directed well-typed AST generator (fuzzer Tier-A): generate arbitrary well-typed programs by construction (typing rules run in reverse), assert skyc accepts + emitted program is no-panic | Guardian-typesystem, multi-session PROJECT — do NOT rush. Adopt the reference design (`../sky` `WellTypedFuzzerGen.hs`) via `proptest`/`arbitrary`. Load-bearing invariant: "well-typed by construction under generation". Scope: pure type-relevant constructs only. | |
| | Medium | Security hardening | **DEFERRED (explicit user override, 2026-07-10) — not blocking this campaign.** #66-N second half — differential rejection fuzzer vs `../sky`: mutate freely, run both compilers, compare accept/reject | The reference is NOT ground truth: a divergence is a REVIEW candidate, never an auto-verdict ("Ipê rejects, Sky accepts" is most likely Ipê being MORE correct). First half (guaranteed-breaking mutation tier, `scripts/fuzz-ill-typed.sh`) landed 2026-07-06. | |
| 2026-07-06 | — | Security hardening | #66 Well-typed no-panic fuzzer (`scripts/fuzz-well-typed.sh`) — landed; wired into the autopilot as the guardian soundness oracle | | |

### A-table — CI, oracle & publish

| Done at | Priority | Road map phase | Task | Notes | Spec |
|---|---|---|---|---|---|
| | High | CI, oracle & publish | #35 Port examples-sweep to skyc + run the full sweep (the source-of-truth gate) | Porting itself is done (spec §1). Declaring "sweep-green" is still blocked on the Critical Boundary-Scheme-Promotion (class-1) row above — do not run the gating full sweep until that lands (spec §0). 2026-07-10: fixed the reproduced same-`CARGO_TARGET_DIR` binary-swap race (`flock`-guarded critical section + PID-suffixed report stamp, commit `6d93e85`) — independent review confirmed this part is correctly implemented. Residual gap found by the SAME review: see the new row below — do not declare the sweep gate fully trustworthy until it lands. | `docs/architecture/class2-tier1-sweep-fix-spec-2026-07-09.md` |
| 2026-07-10 | High | CI, oracle & publish | #35b Residual sweep-concurrency gap (found by review of #35's race fix): per-example diagnostic files (`$HIST/$n.{skyc,cargo,go.build,rust.run,go.run}.log`, `$n.diff.txt`, `$n.equiv/`) are still keyed only by bare example name, not PID/STAMP-suffixed — two invocations with DIFFERENT `CARGO_TARGET_DIR`s (no `flock` contention) but the SAME shared `$HIST` cache can still interleave-corrupt the SAME example's diagnostic files, producing a false `DIFFER` or false equivalence pass. | Fixed via a `diag()` helper routing every per-example diagnostic write through `$HIST/$n.$STAMP.<ext>` (reusing #35's existing `$STAMP`); added a `flock` availability preflight check (previously missing, would have silently no-op'd #35's fix); added `scripts/test-examples-sweep-concurrency.sh`, a real 2-concurrent-invocation regression test. Independent review verified every write/read path routes through `diag()`, `$STAMP` is fixed at script-scope (no within-run divergence risk), the `flock` preflight fires before the critical section, and — critically — reproduced the ORIGINAL bug by reverting `diag()` in a scratch copy and confirming the new regression test correctly fails against it (not just passes trivially against the fix). | `docs/architecture/class2-tier1-sweep-fix-spec-2026-07-09.md` |
| | High | CI, oracle & publish | #110 Oracle full-activation: wire HTML/tui/scenario normalizers + rebuild release skyc + flip CI phase-2 + 65-fixture divergence corpus | | `docs/architecture/class2-tier1-sweep-fix-spec-2026-07-09.md` |
| | High | CI, oracle & publish | #37 Fix CI (port ../sky examples-sweep.yml + ci.yml) + push to `git@github.com:arthurmaciel/ipe-lang` | Includes the CI example-patch-queue (in-repo patches CI applies to upstream examples before build), accepted per `docs/divergences-from-sky.md#planned-future-divergences`. Windows question: `docs/architecture/tui-windows-ci.md` / `docs/architecture/windows-ci-support.md`. Plans: `docs/architecture/sweep-and-parity-plan.md`, `docs/superpowers/plans/2026-07-02-ci-and-push.md`. | `docs/architecture/class2-tier1-sweep-fix-spec-2026-07-09.md` |
| | Critical | CI, oracle & publish | #59 PRE-PUSH: full codebase rename Sky → Ipê/Ipe/ipe (case-preserving; watch the naive-sed trap — upstream-Sky refs stay Sky) | Runs SOLO, dead-last before the push — no other work in flight during the rename. | `docs/superpowers/plans/2026-07-03-rename-sky-to-ipe.md` |
| | Medium | CI, oracle & publish | Publish the README (honest relation-to-Elm-and-Sky framing) | Re-run the divergences review (`docs/divergences-review.md`) first so the ledger the README cites is current. | `docs/README-draft-relation-to-elm-and-sky.md` |
| | Medium | CI, oracle & publish | E2E shared-target + cached-oracle infrastructure (queued) | | `docs/architecture/e2e-and-oracle-caching.md` |

### A-table — Hardening follow-ups

| Done at | Priority | Road map phase | Task | Notes | Spec |
|---|---|---|---|---|---|
| 2026-07-10 | Low | Hardening follow-ups | `is_cross_origin_ingest`/`ws_cross_origin` compared `Origin` host against the raw `Host` header string — a default-port-vs-explicit-port mismatch caused a false-positive same-origin rejection | Fixed: a shared `strip_default_port` helper in `http_header.rs` (strips `:443`/`:80` per scheme) is now used by all three sites (`live/console.rs`, `server.rs`, `live/csrf.rs::origin_mismatch`) so they can't drift, with 3 regression tests (explicit https default port, explicit http default port, non-default port still flagged). Availability nit, always failed CLOSED. | `docs/architecture/class8-live-http-security-fix-spec-2026-07-09.md` |
| 2026-07-10 | Low | Hardening follow-ups | `DbFindOneByField`/`DbFindManyByField` (arity fixed by #70) had no golden E2E coverage — only source inspection + the internal exhaustiveness test | Landed: `m5b_db_find_by_field` golden fixture (inserts 3 rows, exercises findOneByField match + no-match and findManyByField match + no-match, sorted so the assertion doesn't depend on unspecified SQL row order). `sanctioned.divergence` marker follows the established Db-fixture convention (Go+SQLite vs Rust+sqlx; ipê output is the reference). oracle.meta + expected_go.txt authoritatively regenerated via `refresh-oracle` (real build+run), and the `SKY_E2E=1` tier passes (88s real `skyc build` + `cargo build` + run). | `docs/architecture/class3-kernel-registry-fix-spec-2026-07-09.md` |
| 2026-07-11 | Medium | Hardening follow-ups | #34 M5b-db follow-ups: SqlValue variant completeness, exhaustive `emit_db_call`, self-oracle, db-without-live build; wire `db_decode_money` into kernel dispatch | Landed (commit `821d2ab`): `Db.Decode.money` wired through the full kernel-registration recipe (canon allowlist, `StdlibKernel::DbDecMoney` + `decl()`, constrain scheme `String -> Decoder (Decimal, String)`, lower dispatch, standard-path emit) — previously implemented+tested in runtime but unreachable from Sky source. Sibling sub-items confirmed already closed by earlier commits (SqlValue completeness `b2a53d7`, db-without-live `e57d691`, exhaustive `emit_db_call` with `CompilerBug` fallthrough). Independent review verified: all 3 exhaustiveness gates genuinely cover the new kernel (accounting assert makes silent skips impossible); the malformed-money path is total (12 adversarial inputs → all `Err`, zero panics — total by construction, `split_once`, no indexing); the `B-DbDecMoney` divergence entry (`Decoder (Decimal, String)` vs Go's `Decoder Money` — project-generated types unnameable from the shared runtime crate) is accurate, honest, and the round-trip is lossless. SEAL proven via `SKY_E2E=1` golden. 2 non-blocking findings filed as their own rows (parity-index tooling drift; RELOCATED-vs-FIRST_SCHEMED taxonomy). | `docs/architecture/class7-sql-db-fix-spec-2026-07-09.md` |
| | Medium | Hardening follow-ups | #33 M5b-http residual: header-case parity remainder + extra Http builders | Confirmed partially already fixed — the residual is the item (class8 spec §6). | `docs/architecture/class8-live-http-security-fix-spec-2026-07-09.md` |
| 2026-07-10 | Medium | Hardening follow-ups | #129 Runtime audit: `spawn_blocking` for CPU-heavy/blocking kernels (bcrypt/zstd/file) — reactor-starvation guard | Landed: 5 files audited and wrapped via a shared `run_blocking` helper (real `spawn_blocking` under the `tokio` feature, direct-call fallback for the standalone runtime crate's narrow-feature builds only — never a real generated Sky project, which always pulls `tokio`) — `file.rs` (readFile/writeFile/exists/mkdirAll/etc), `compression.rs` (gzip/gunzip/zstd), `config_decode.rs` (load_from_file), `csv.rs` (stream_from_file), `system.rs` (process::run). Panic-inside-spawn_blocking correctly converts to `Task Error`, no swallowing. Independent review verified all 7 new regression tests genuinely detect the ABSENCE of `spawn_blocking` (ticker-yield-starvation pattern), not just functional correctness — and found 2 follow-ups filed as their own rows: those tests don't run under the default CI gate (feature-gated out), and `system_load_env` was missed from the audited surface. | `docs/architecture/class9-kernel-robustness-fix-spec-2026-07-09.md` |
| 2026-07-10 | Medium | Hardening follow-ups | #129's 7 new `spawn_blocking` regression tests never ran under the default CI gate — feature-gated out | Fixed: added a `.github/workflows/ci.yml` step `cargo nextest run -p sky-runtime-rust --features full`, verified locally to actually execute the `*_spawn_blocking_tests` modules (6 matched, all pass) — a future refactor that drops `spawn_blocking` now fails CI. | `docs/architecture/class9-kernel-robustness-fix-spec-2026-07-09.md` |
| 2026-07-10 | Low | Hardening follow-ups | `system_load_env` did a synchronous `std::fs::read_to_string(".env")` not routed through #129's `run_blocking` helper | Fixed: routed through the same `run_blocking` helper as its sibling `system.rs` kernels; regression test `system_load_env_does_not_starve_concurrent_async_work` (ticker-yield-starvation pattern) runs under the new CI `--features full` step. | `docs/architecture/class9-kernel-robustness-fix-spec-2026-07-09.md` |
| 2026-07-10 | Medium | Hardening follow-ups | `File.readFileBytes` silently truncated at its fixed 10 MiB cap instead of erroring | Fixed with the same single-pass `take(cap+1)`-and-check-actual-bytes idiom as its `readFileLimit` sibling — reads one byte past the cap and errors loudly on overflow instead of returning a truncated buffer. Regression test in `file.rs`'s `read_file_limit_tests` sibling module. | `docs/architecture/class9-kernel-robustness-fix-spec-2026-07-09.md` |
| 2026-07-10 | Medium | Hardening follow-ups | `File.readFileLimit` metadata-then-`take(cap)` TOCTOU — a file growing mid-check silently truncated instead of erroring; fixed to a single-pass `take(cap+1)`-and-check-actual-bytes-read idiom, removing the race window structurally | From the AUD-09 gap-sweep (`file.rs:100`). Commit `706f026`; independent review found no issues. | `docs/architecture/class9-kernel-robustness-fix-spec-2026-07-09.md` |
| | Medium | Hardening follow-ups | #142 Precision follow-up: #139 made `Expr::Access` clone unconditionally + blanket `Clone` bounds on fn generics — restore the borrow fast-path | | `docs/architecture/class5-emitter-clone-fix-spec-2026-07-09.md` |
| | Medium | Hardening follow-ups | #31 Phase-4 remainder — make-invalid-states-unrepresentable hardening (non-blocking) | | |
| | Medium | Hardening follow-ups | AUD-09 lower/emitter leftovers: `Match::from_parts_unchecked` pub (`ir.rs:1626`), Bug-29 `any`-return matches any `Ty::Con` (`lower.rs:3405`), unconditional field-access `.clone()` O(n²) (`emit_expr.rs:4794`) | See the audit ledger for loc+fix each. | `docs/architecture/class3-kernel-registry-fix-spec-2026-07-09.md` + `docs/architecture/class5-emitter-clone-fix-spec-2026-07-09.md` |
| | Low | Hardening follow-ups | #53 Emit backend via typed token AST instead of String concatenation | Guardian-design item. | |
| | Low | Hardening follow-ups | Efficiency-audit ledger burn-down (remaining medium/low findings) | | `docs/architecture/efficiency-audit-2026-07-02.md` |
| 2026-07-09 | — | Hardening follow-ups | AUD-01..08 + AUD-10..15 (audit hardening pass, 13/15 landed; AUD-09 partial) | Full findings + ledger: `docs/architecture/principles-audit-2026-07-09.md`. | |

---

## B. FFI subsystem — parked until A completes

The FFI design is complete and reviewed
(`docs/architecture/ffi-port-spec.md`). Implementation does not start
until the compiler is done. Scope: **fully-automatic, shim-free
binding of arbitrary Rust crates** (not Go packages).

**Divergence from Sky:** the reference implementation binds Go
packages; ipê's FFI binds Rust crates. The subsystem is otherwise
designed to reach the same fully-automatic, no-user-written-shim
experience.

*Rationale:* parking FFI keeps the critical path focused; the ordering
puts the security gate ahead of convenience so an untrusted-crate
compile can never run unsandboxed. Prove on pure/sync crates first,
then async SDKs — shim-free binding of async SDKs is the acceptance
metric.

| Done at | Priority | Road map phase | Task | Notes | Spec |
|---|---|---|---|---|---|
| | High | FFI | #40 FFI Phase 0 — inspector hardening (disjoint `tools/` crate) | | `docs/superpowers/plans/2026-07-02-ffi-phase0-inspector.md` + `docs/architecture/ffi-subsystem-design.md` |
| | Critical | FFI | #41 FFI sandbox — blocking security gate before `ipe add` ships (compiling arbitrary third-party crates is execution of untrusted code) | Security precedes any generation work. | `docs/architecture/ffi-sandbox-and-generator-impl-ready.md` |
| | High | FFI | #42 FFI consumer port — generator (Haskell → Rust `ipe_ffi` crate) | Depends on the kernel registry. | `docs/architecture/ffi-port-spec.md` + `docs/architecture/ffi-design.md` + `docs/architecture/ffi-rust-subsystem-design.md` |
| | Medium | FFI | Async FFI bridge — bind async Rust SDKs (tokio-runtime bridge, `AbortOnDrop` cancel propagation, B8 error funnel) | Shim-free async-SDK binding is THE acceptance metric. | `docs/architecture/async-ffi-bridge-design.md` |
| 2026-07-10 | — | FFI | AUD-10 inspector interim mitigation: refuse `custom-build`/`proc-macro` targets unless `--allow-build-scripts` | Full sandboxing remains #41. | |

---

## C. Post-completion program

### C.1 — Project rename to `ipe`

Ship a single `ipe` binary spanning the compiler, the future
interpreter, the project doctor, and watch. Apply consistent naming
throughout the codebase, retaining a single acknowledgement line in the
README. The codebase rename itself (#59) is **pre-push** (see the
CI, oracle & publish phase above, per the Tier-1 chain); C.1 keeps the
single-`ipe`-binary product scope.

*Rationale:* one binary and one name is the coherent product identity;
the acknowledgement line records lineage without diluting it.

### C.2 — Module-namespace redesign

Replace the two-tier core/std split with a **single flat standard
library**, with nothing imported by default and LSP auto-import on
first use. Research prelude handling in Rust, Elm, Gleam, Haskell, Go,
and Zig before committing to the shape.

### C.3 — Source-name de-abbreviation

Rename abbreviated source identifiers for readability — for example
`kernel_ty` → `kernel_type`, `Ty::Var` → `Type::Variable`. Idiomatic
Rust abbreviations are retained.

### C.4 — Guarantee Elm `core` library coverage

Audit the standard library against `elm/core` and add the missing
modules and functions (Array, Tuple, Bitwise, and any others the audit
surfaces). The authoritative `elm/core` inventory is enumerated in
[`elm-core-coverage.md`](docs/architecture/elm-core-coverage.md).

### C.5 — Evaluate more principled compilation strategies

Study the reference Haskell backend and `elm/compiler`, and adopt a
strategy only where it **strictly improves a project principle without
harming a higher one**. Where a strategy is not adopted, record a
comparison table capturing the trade-off.

### C.6 — Implement the filed divergent language features

Implement the language features filed in
[`docs/divergences-from-sky.md#planned-future-divergences`](docs/divergences-from-sky.md#planned-future-divergences):
hot-reloading, Std.Ui-as-IR, standalone TEA, deep-update sugar,
or-patterns, pattern guards, effect-sequencing (`do` block), record
punning, a dev-only time-travel debugger, and the CI patch queue over
upstream examples.

**Divergence from Sky:** these features are intentional departures
from the reference language; each is tracked with its own rationale in
the divergences ledger. Divergences go last, on a verified-complete
base.

| Done at | Priority | Road map phase | Task | Notes | Spec |
|---|---|---|---|---|---|
| | Medium | Post-completion | #155 Route URL changes to a Msg (Elm `Browser.application` parity), demote the magic `page` field to sugar | First Elm-core-coverage item. | `docs/architecture/url-navigation-msg-design.md` |
| | Medium | Post-completion | #116 Entry contract (Option C): auto-run Task/backend-app `main` + drop trailing `\|> Task.run` (v0.17.3 pipeCollapsesTask parity) | | `docs/architecture/adopt-from-sky-v0172.md` |
| | Medium | Post-completion | #128 Drop `Task.run` + `Task.perform` from the Ipê surface (#116 companion) | Departure — first consumer of the CI example-patch-queue. | `docs/architecture/drop-task-run-surface-design.md` |
| | Medium | Post-completion | #131 `Task.map2..5` + `Task.parallel2..5` (expression forms) + `parallelDo` block | | `docs/architecture/task-combinators.md` |
| | Medium | Post-completion | #133 Multiline-string margin stripping (anchor = first string character's column) | Departure — output-changing; records an oracle divergence per patch class. | `docs/architecture/multiline-string-margin-stripping-design.md` |
| | Medium | Post-completion | Idea-7 effect `do` block (scoped effect sequencing; kills the `let _ = TaskExpr` auto-force wart) | DESIGNED 2026-07-04. | `docs/ideas/idea-7-effect-do-block-design.md` |
| | Medium | Post-completion | C.4 Guarantee Elm `core` library coverage — audit the stdlib against `elm/core`, add missing modules/functions (Array, Tuple, Bitwise, …) | | `docs/architecture/elm-core-coverage.md` + `docs/architecture/elm-core-gap-matrix.md` + `docs/architecture/additive-stdlib-features.md` |
| | Medium | Post-completion | C.2 Module-namespace redesign: single flat standard library, nothing imported by default, LSP auto-import on first use | Research prelude handling in Rust, Elm, Gleam, Haskell, Go, Zig first. | `docs/architecture/flat-namespace-redesign.md` |
| | Medium | Post-completion | C.3 Source-name de-abbreviation (`kernel_ty` → `kernel_type`, `Ty::Var` → `Type::Variable`; idiomatic Rust abbreviations retained) | | `docs/architecture/readability-and-naming-audit.md` |
| | Medium | Post-completion | C.5 Evaluate more principled compilation strategies from the reference Haskell backend + `elm/compiler`; adopt only where a principle strictly improves, else record the comparison table | | `docs/architecture/sky-upstream-learnings.md` |
| | Medium | Post-completion | C.6 Implement the filed divergent language features (deep-update sugar, or-patterns, pattern guards, record punning, hot-reload family, time-travel debugger, …) | Divergences go last, on a verified-complete base. | `docs/divergences-from-sky.md#planned-future-divergences` |
| | Medium | Post-completion | #56b Row-var record annotation syntax `{ r \| f : T }` does not parse (SKY-P0001) — reference accepts + monomorphises the callee per record shape | Completeness gap found while investigating #56. Zero corpus usage (not sweep-blocking). Needs per-record-shape callee monomorphisation in the backend first — same machinery the class-1 coupling tripwire needs; land together if possible. | `docs/architecture/row-poly-subset-superset-design.md#gap-filed-row-var-record-annotation-syntax-r--f--t` |

---

## D. Longer-horizon / standing

### D.1 — Incremental compilation (salsa)

Introduce salsa-based incremental compilation across the compiler and
the LSP — the foundation for fast watch and hotpatching.

### D.2 — Standard-library behaviour audit against Elm semantics

Audit standard-library behaviour against Elm semantics, covering at
least: JSON object key order, integer-decoder strictness, float
formatting, and null / oneOf / nullable handling.

### D.3 — Full floating-point Set/Dict keys and locale-correct case mapping

Support full floating-point keys in Set and Dict (ordered-float) and
locale-correct case mapping.

| Done at | Priority | Road map phase | Task | Notes | Spec |
|---|---|---|---|---|---|
| | Low | Longer-horizon | D.1 Incremental compilation (salsa) across the compiler and the LSP — foundation for fast watch + hotpatching | | `docs/architecture/incremental-compilation-and-watch.md` + `docs/superpowers/plans/2026-07-03-incremental-salsa.md` |
| | Low | Longer-horizon | D.2 Standard-library behaviour audit against Elm semantics (JSON key order, integer-decoder strictness, float formatting, null/oneOf/nullable) | | `docs/architecture/stdlib-elm-behaviour-audit-plan.md` |
| | Low | Longer-horizon | D.3 Full floating-point Set/Dict keys (ordered-float) + locale-correct case mapping | Lifts SKY-L0117. | `docs/architecture/float-keys-and-locale-case-design.md` |

---

## E. Designed compilation targets (specs approved; priority to be set)

Each has a complete, security-reviewed design spec; sequencing against
sections A–D is a product decision.

### E.1 — WASM / browser target

Compile ipê programs to WebAssembly so apps run client-side in the
browser (TEA in the browser, reusing the ported VNode/diff to drive the
real DOM), and support an online playground. The design fixes the
public-bundle secret boundary at compile time (server-only effects are
unrepresentable under `--target wasm`; a distinct `HydrationState` type
gates what may enter the SSR hydration island) and preserves the
no-eval / strict-CSP posture. Spec:
[`wasm-target.md`](docs/architecture/wasm-target.md).

### E.2 — Static compilation (portable single binaries)

Produce fully-static, portable binaries — musl on Linux, static-CRT on
Windows, with an honest macOS limitation — with a pure-Rust **`dlmalloc`
default allocator** (clears the musl-malloc throughput cliff without a C
dependency, per the security-first order); mimalloc is an explicit,
notice-emitting opt-in. Spec:
[`static-compilation.md`](docs/architecture/static-compilation.md).

### E.3 — Language server (LSP)

A salsa-backed, editor-agnostic language server: diagnostics, hover,
go-to-definition, completion, semantic tokens, formatting, and rename —
reusing the compiler's single type-checker (no divergent analyzer). Its
headline feature is **TEA scaffolding** — snippets, code actions ("add
`Msg` variant + matching `update` arm", "convert `main = Task.run` to a
worker"), and lints/hints — delivered over standard LSP so it works in
most editors. Every generated edit passes a `VerifiedEdit` gate that
re-checks the whole edit blast radius, so a scaffold can never break the
build. Spec: [`ipe-lsp.md`](docs/architecture/ipe-lsp.md).

### E.4 — TEA everywhere (opt-in worker shape)

Make The Elm Architecture an opt-in program shape for every backend —
including a headless `Std.Worker.program` (init / update / subscriptions,
no view) for CLI and long-running processes, modelled on Elm's
`Platform.worker`. Least-intrusive: existing entries (`main = Task.run`,
`Live.app`, `Server.listen`) are byte-unchanged; TEA is strictly
additive and reuses the ported TEA runtime. The headless loop terminates
soundly by tracking live source-task liveness (a signal-only daemon
stays alive for SIGTERM; a quiescent worker exits cleanly). Spec:
[`tea-everywhere.md`](docs/architecture/tea-everywhere.md).

*Implementation invariants (from the design review, to enforce at build
time):* sequence the counter Acquire-loads before `try_recv`; `select!`
over mailbox-recv and a quit-notify so a full-mailbox daemon still
observes SIGTERM; abort (not await) source tasks during the quit-drain.

| Done at | Priority | Road map phase | Task | Notes | Spec |
|---|---|---|---|---|---|
| | Low | Designed targets | E.1 WASM / browser target (TEA in the browser, playground; compile-time public-bundle secret boundary) | | `docs/architecture/wasm-target.md` |
| | Low | Designed targets | E.2 Static compilation — portable single binaries (musl / static-CRT; `dlmalloc` default allocator) | | `docs/architecture/static-compilation.md` + `docs/superpowers/plans/2026-07-03-static-compilation.md` |
| | Low | Designed targets | E.3 Language server (LSP) — salsa-backed, TEA scaffolding, `VerifiedEdit` gate | | `docs/architecture/ipe-lsp.md` + `docs/superpowers/plans/2026-07-03-lsp.md` |
| | Low | Designed targets | E.4 TEA everywhere — opt-in headless `Std.Worker.program` shape for every backend | Implementation invariants recorded in the spec (Acquire-loads before `try_recv`; `select!` over mailbox + quit-notify; abort source tasks during quit-drain). | `docs/architecture/tea-everywhere.md` |
