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
| | High | Sweep to green | #90 SKY-L0114 ctor-payload-function — `Ok`/`Just` holding a function is rejected, making `Result.andMap` / `Maybe.andMap` unusable | **NEEDS A REAL DESIGN PASS, not another mechanical fix — 3 incidents same day (2026-07-10), all on the SAME sub-feature (T3, curried-`andMap` payload rejection):** (1) `f80f05a` landed, reverted — reuse gate missing for lambda params (E0382) + T3 bypassed via a `let`-bound alias (E0277). (2) `39d9a57` re-landed with both fixes, reverted again — a THIRD violation reproduced: a bare/re-exported alias (`myAndMap = Result.andMap`) still reaches `cargo build` as E0277, because the check lives in `lower_call_uniform`'s Call-node arm, which a bare-value kernel reference (lowered via the `VarTopLevel`/`VarKernel` arm) never passes through. **Root cause of the repeated failure:** the hazard is a TYPE-LEVEL property (payload arity) that can flow through arbitrary Sky-level aliasing; AST-shape pattern matching at any single lowering site cannot be exhaustive against it. **What a correct fix needs:** move the check to the actual kernel-call EMISSION boundary (where the payload's solved type is concretely known), not any AST-shape match upstream. T4 (lambda-param reuse) is independently solid (held under a second review) and doesn't need redoing — just restore its fixtures alongside a correct T3 fix. Guardian-typesystem item — do NOT rush a fourth attempt. | `docs/architecture/ctor-payload-function-design.md` |
| | High | Sweep to green | #158 Nested-constructor-payload function-argument patterns (`f (Just (h :: t)) = …`, `f (Ok {name}) = …`) are fail-closed (SKY-L0112/SKY-L0116) where the reference recurses and compiles | Correct fail-closed behavior; the completeness gap itself is the item (divergence A13). | `docs/architecture/class4-pattern-lowering-fix-spec-2026-07-09.md` |
| | High | Sweep to green | #99 Refutable match-arm alias over non-Copy payload double-moves — `case m of Just ((a,b) as w) -> use a,b,w` → E0382 | Pre-existing, out of #96 scope. | `docs/architecture/seal-noncopy-move-design.md` §4.2 + `docs/architecture/class5-emitter-clone-fix-spec-2026-07-09.md` |
| | High | Sweep to green | #125 Decoder thunk coverage: tuple-destructure + record-field binders (loud E0382) | Pre-existing. | `docs/architecture/class5-emitter-clone-fix-spec-2026-07-09.md` |
| | Medium | Sweep to green | #102 F1 shadow diagnostic: local `type X` shadowing a dep-imported `X` → downstream SKY-T0001 instead of clean SKY-N0012 at the decl | Low-risk, fail-closed today. | `docs/architecture/class4-pattern-lowering-fix-spec-2026-07-09.md` |
| | High | Sweep to green | #113 Pseudo-class attrs render to nothing in the static `htmlRender` sink (AttrPseudoRule no-op) | | `docs/architecture/class10-ui-html-fix-spec-2026-07-09.md` |
| | Medium | Sweep to green | #105 Std.Css hardening (defence-in-depth): optional @import/expression gating on `raw`/`keyframes` bodies + reject CSS-hex-escaped values in `safeValue` | | `docs/architecture/class10-ui-html-fix-spec-2026-07-09.md` |
| | High | Sweep to green | #32 M5a follow-ups (fail-closed): Task arity-3 ICE + Task-in-ADT-ctor gate | | `docs/architecture/class4-pattern-lowering-fix-spec-2026-07-09.md` |
| | High | Sweep to green | #56 Prove row-poly subset/superset record resolution (A7 watch) + gate on sweep | Investigated 2026-07-10: no defect found, all reachable subset/superset shapes resolve or fail-loud in parity with the reference. Remaining work is purely additive: wire the spec's 5 golden-test fixtures into `crates/skyc/tests/golden_row_poly_records.rs` to pin the invariant on the sweep — no compiler code changes. | `docs/architecture/row-poly-subset-superset-design.md` |
| 2026-07-10 | High | Sweep to green | #45 Make the constrain kernel-scheme table exhaustive over canon lists (close the exit-0-then-cargo-fail class) | Extended `canon_equals_registry`'s G1 reverse loop (`crates/sky_canon/src/lib.rs`) into a full subset gate over `qual_vars`; two exception mechanisms (`excluded_quals`, member-granular `deliberately_unbacked_members`) keep it from blinding whole modules. Surfaced 20 previously-undocumented unbacked kernels — see the Std.Ui/Std.Html kernel-gaps row below. | `docs/architecture/class3-kernel-registry-fix-spec-2026-07-09.md` + `docs/architecture/html-ui-live-scheme-table.md` + `docs/superpowers/plans/2026-07-03-registry-phase-E.md` |
| 2026-07-10 | High | Sweep to green | #70 Fix kernel arity-table drift (`decl().arity` vs `callee_arity`) — latent exit-0-then-cargo-fail | Found and fixed real drift across ~20 kernels (`Pure.*` companions like `IoReadLine`/`TimeNow`/`SystemArgs`, several `Db.*ById`/`*ByField` variants, TEA `Cmd`/`PubSub.publish`, `Middleware`/`RateLimit` variants) whose `decl().arity` disagreed with the actual call-site arity. Added `callee_arity_matches_decl_arity`, a machine-checked exhaustiveness test (same pattern as #45's `canon_equals_registry`) asserting the two never diverge for any `StdlibKernel` variant, so a future addition that gets this wrong fails the gate immediately. | `docs/architecture/class3-kernel-registry-fix-spec-2026-07-09.md` |
| | Medium | Sweep to green | #85 ErrorDetails follow-up: port `ErrorInfo.details : Maybe ErrorDetails` + the 5-variant `ErrorDetails`/`PanicInfo`/`TypeInfo` union | Additive; same registration recipe as `ErrorKind` (canon ctor registration + lowerer arms + `IrType` leaf + `builtin_runtime_enum` + constrain ctor schemes). Core `Error ErrorKind ErrorInfo` ADT landed 2026-07-09. | `docs/architecture/error-module-design.md` |
| | High | Sweep to green | Remaining Std.Ui / Std.Html kernel gaps surfaced by the example sweep — wire missing kernels across the layers with the `../sky` reference | [progdev-safe] Per-example blockers live in `docs/architecture/remeasure-snapshot.tsv`; computed gaps via `scripts/ipe-index parity --gaps`. Template = the landed `Border.shadow`/`Border.glow`/`Border.innerShadow` wirings. 2026-07-10: #45's exhaustiveness gate enumerated the CONCRETE current list — 20 unbacked `qual_vars` members with zero matching `StdlibKernel` entry: `Ui.image/disabled/paddingEach/clipX/clipY/scrollbarX/scrollbarY/onFile/mediaQuery/onPseudo/hover/focus/focusVisible/active`, `Html.toString/voidNode/doctype/titleNode/htmlNode/headNode`, `Background.linearGradient`. All fail closed today (loud `SKY-L0108`, not silent) — tracked in `crates/sky_canon/src/lib.rs`'s `deliberately_unbacked_members` exception list until wired. | `docs/architecture/ui-html-completeness-design.md` |

### A-table — Security hardening

| Done at | Priority | Road map phase | Task | Notes | Spec |
|---|---|---|---|---|---|
| | High | Security hardening | #44 Opaque `Secret` stdlib type (gates WASM hydration island + secrets-are-typed rule) | | `docs/architecture/class6-secret-sqlfragment-fix-spec-2026-07-09.md` |
| 2026-07-10 | High | Security hardening | #61 `SqlFragment` param-query newtype — SQL injection = type error | Landed: opaque `SqlFragment` with typed combinators (`column`/`param`/`eq`/`and`/`inList`/etc.); `Db.findWhere`/`Db.deleteWhere` take `SqlFragment`, not `String`; `Db.unsafeFindWhere` removed outright (not deprecated) per the spec's no-deferral decision. Found+fixed one exit-0-then-cargo-fail during its own verification (`sql_column`'s `&str` param, inconsistent with every other Sky-`String`-typed kernel param). Full workspace gate initially failed after merge — 21 new kernels (19 `Sql.*` + `Db.findWhere`/`deleteWhere`) were missing from `lower_callee`'s legacy string-match table (only exercised by the `id=None` fallback path, which lane-c's own targeted tests never hit); fixed centrally post-merge, gate now green. | `docs/architecture/class6-secret-sqlfragment-fix-spec-2026-07-09.md` |
| | Medium | Hardening follow-ups | Pre-existing E2E failures found while landing #61 (unrelated, reproduced identically on unmodified master `00c4d32`): `crates/skyc/tests/golden_m5b_db.rs`'s `db_crud`/`db_transaction` fail — code paths involve Dict/List(String,String) typing and the `Task.fail : String -> SkyError` channel conversion, neither touched by #61. | Found by lane-c's own verification pass (2026-07-10) while confirming its changes weren't the cause. | |
| 2026-07-10 | High | Security hardening | Class-8 web remainder: session cookie `Secure` TLS-gated (not ENV-gated), `/_sky/observability/ingest` CSRF exemption, WS upgrade Origin check outside production (CSWSH), `live_max_body_bytes()` `>0` floor | All 4 sub-items landed: session cookie now reflects the real TLS signal (`request_is_https`, `live/mod.rs`) not just `ENV`; ingest endpoint's dev-mode no-token path now rejects cross-origin POSTs (`live/console.rs`); WS upgrade defaults to same-origin outside production instead of allow-all (`server.rs`); `live_max_body_bytes` floor was already correct. The CSRF cookie's OWN TLS-gating was deliberately left out of scope — see `#63c` below. | `docs/architecture/class8-live-http-security-fix-spec-2026-07-09.md` |
| 2026-07-10 | High | Security hardening | #63b `withCsrf`'s golden/E2E coverage never issues a real HTTP request — `golden_m6_middleware_csrf.rs` only checked `skyc`/`cargo build` succeed; `server_e2e.rs` had zero CSRF tests. | Landed: 3 real HTTP-level E2E tests in `server_e2e.rs` — forged POST (no cookie/header) → 403; cookie present + mismatched/missing header → 403 (both sub-cases); legit GET-mints-cookie-then-POST-echoes-it flow → 200 with the wrapped handler's own body. Proves the full 12-site kernel-registry dispatch chain end to end over a real TCP connection, not just compile-level. | `docs/architecture/class8-live-http-security-fix-spec-2026-07-09.md` |
| | High | Security hardening | #63c CSRF cookie (`csrf_set_cookie_value`, `runtime/src/sky_runtime/server.rs`) still has the ENV-vs-TLS `Secure`-gating bug the session cookie just had fixed (2026-07-10, Class-8 lane) — a forgotten `ENV=production` on an actually-TLS'd deploy still ships the CSRF cookie without `Secure`/host-lock. Needs a real design change, not a copy-paste of the session-cookie fix: the request-scoped TLS signal (`request_is_https`) must be captured BEFORE `ServerRequest` is moved into the wrapped handler inside `middleware_with_csrf`, unlike the session-cookie path where the request is still available at cookie-set time. | Deliberately left out of the Class-8 lane's scope (2026-07-10) — flagged as needing its own pass. | `docs/architecture/class8-live-http-security-fix-spec-2026-07-09.md` |
| 2026-07-10 | High | Security hardening | Class-7 SQL/DB remainder: `SqlNull` text-typed NULL breaks Postgres, Postgres driver structurally unreachable, `db_insert_row` fabricated `id=0` on non-integer PK, tenant-prefix SQL enforcement absent from plain `db.rs` | All 4 sub-items landed: `SqlParam::Null` now carries a type witness (`Null(Box<SqlParam>)`) instead of binding as text; new `[database] driver` manifest parsing (`crates/skyc/src/project.rs`) threads a `DbDriver` enum through `RustBackend::with_db_driver` → `EmitCtx` → template/Cargo.toml selection, closing the silent-no-op where `driver = "postgres"` never changed the emitted `config.rs` (proven structurally by the new `crates/skyc/tests/postgres_driver_reachability.rs`, no live Postgres needed); `db_insert_fields` gained a `DB_USES_RETURNING_ID` branch + `extract_returning_id` helper so autoincrement ids are read back instead of fabricated as `0`; `runtime/src/sky_runtime/live/hub.rs` gained `tokio::task_local!`-scoped `TENANT_PREFIX` + `reject_cross_tenant_svc`, closing the plain `db.rs` gap in the tenant-prefix SQL-WHERE enforcement (v0.16.6-equivalent guarantee) that `hub.rs`'s reader path already had. Post-merge clippy gate caught 2 issues fixed centrally: a `clippy::doc_markdown` nit (`` `SQLite` `` backticks) and 3 `clippy::expect_used` errors in the new test file's setup helper (`#[allow]`, matching the established per-test-file convention). **Independent review (commit `ac8b2bfc`) then found a real SEAL violation in sub-item 2:** `db_cargo_toml` selected the sqlx driver feature EXCLUSIVELY (`"sqlite"` xor `"postgres"`), but the always-emitted `telemetry_spill.rs`/`live/hub.rs`/`live/store.rs` runtime modules hardcode `sqlx::sqlite::SqlitePool` for local spill/session persistence independent of the app's `[database]` driver choice — a `driver = "postgres"` project built cleanly through `skyc` (exit 0) but its emitted Rust failed `cargo build` with 3 errors once the `sqlite` sqlx feature was dropped. Root-cause fixed same day (commit `b67a857`): the feature selection is now additive (`sqlite` always on, `postgres` added on top), the contradicting unit test assertion was corrected, and a new `SKY_E2E`-gated `postgres_driver_project_cargo_builds` test actually `cargo check`s the emitted Postgres project (closing the coverage gap — the original test only grepped emitted source text, never built it). `cargo nextest run --workspace` + `cargo clippy --workspace --all-targets -D warnings` both green after the fix. | `docs/architecture/class7-sql-db-fix-spec-2026-07-09.md` |
| 2026-07-10 | High | Security hardening | `url_is_cacheable` substring-`contains("memory")` DoS reopen — fixed to parse the `file:`/`sqlite:` scheme + query string structurally instead of substring-matching; independent review then caught a second soundness hole in the fix itself (`"file::memory:"`, SQLite's documented in-memory URI idiom, was misclassified as cacheable, silently pooling distinct private databases) — closed in the same pass | Commit `1d65aa0`; regression tests `url_is_cacheable_*` in `runtime/src/sky_runtime/db.rs`. | `docs/architecture/class7-sql-db-fix-spec-2026-07-09.md` |
| | Medium | Security hardening | #66-T2 Type-directed well-typed AST generator (fuzzer Tier-A): generate arbitrary well-typed programs by construction (typing rules run in reverse), assert skyc accepts + emitted program is no-panic | Guardian-typesystem, multi-session PROJECT — do NOT rush. Adopt the reference design (`../sky` `WellTypedFuzzerGen.hs`) via `proptest`/`arbitrary`. Load-bearing invariant: "well-typed by construction under generation". Scope: pure type-relevant constructs only. | |
| | Medium | Security hardening | #66-N second half — differential rejection fuzzer vs `../sky`: mutate freely, run both compilers, compare accept/reject | The reference is NOT ground truth: a divergence is a REVIEW candidate, never an auto-verdict ("Ipê rejects, Sky accepts" is most likely Ipê being MORE correct). First half (guaranteed-breaking mutation tier, `scripts/fuzz-ill-typed.sh`) landed 2026-07-06. | |
| 2026-07-06 | — | Security hardening | #66 Well-typed no-panic fuzzer (`scripts/fuzz-well-typed.sh`) — landed; wired into the autopilot as the guardian soundness oracle | | |

### A-table — CI, oracle & publish

| Done at | Priority | Road map phase | Task | Notes | Spec |
|---|---|---|---|---|---|
| | High | CI, oracle & publish | #35 Port examples-sweep to skyc + run the full sweep (the source-of-truth gate) | Porting itself is done (spec §1). Declaring "sweep-green" is still blocked on the Critical Boundary-Scheme-Promotion (class-1) row above — do not run the gating full sweep until that lands (spec §0). 2026-07-10: fixed the reproduced same-`CARGO_TARGET_DIR` binary-swap race (`flock`-guarded critical section + PID-suffixed report stamp, commit `6d93e85`) — independent review confirmed this part is correctly implemented. Residual gap found by the SAME review: see the new row below — do not declare the sweep gate fully trustworthy until it lands. | `docs/architecture/class2-tier1-sweep-fix-spec-2026-07-09.md` |
| | High | CI, oracle & publish | #35b Residual sweep-concurrency gap (found by review of #35's race fix): per-example diagnostic files (`$HIST/$n.{skyc,cargo,go.build,rust.run,go.run}.log`, `$n.diff.txt`, `$n.equiv/`) are still keyed only by bare example name, not PID/STAMP-suffixed — two invocations with DIFFERENT `CARGO_TARGET_DIR`s (no `flock` contention) but the SAME shared `$HIST` cache can still interleave-corrupt the SAME example's diagnostic files, producing a false `DIFFER` or false equivalence pass. Same bug class as #35, one layer down — undermines trust in the sweep verdict itself. Also fix while here: no preflight check for `flock` availability (script has no `-e`, so a missing `flock` makes the fix silently no-op with zero signal — add it alongside the existing `curl`/`python3`/`go`/`rg` preflight checks); add an automated 2-concurrent-invocation regression test (none exists — the original fix was verified manually only). | `docs/architecture/class2-tier1-sweep-fix-spec-2026-07-09.md` |
| | High | CI, oracle & publish | #110 Oracle full-activation: wire HTML/tui/scenario normalizers + rebuild release skyc + flip CI phase-2 + 65-fixture divergence corpus | | `docs/architecture/class2-tier1-sweep-fix-spec-2026-07-09.md` |
| | High | CI, oracle & publish | #37 Fix CI (port ../sky examples-sweep.yml + ci.yml) + push to `git@github.com:arthurmaciel/ipe-lang` | Includes the CI example-patch-queue (in-repo patches CI applies to upstream examples before build), accepted per `docs/divergences-from-sky.md#planned-future-divergences`. Windows question: `docs/architecture/tui-windows-ci.md` / `docs/architecture/windows-ci-support.md`. Plans: `docs/architecture/sweep-and-parity-plan.md`, `docs/superpowers/plans/2026-07-02-ci-and-push.md`. | `docs/architecture/class2-tier1-sweep-fix-spec-2026-07-09.md` |
| | Critical | CI, oracle & publish | #59 PRE-PUSH: full codebase rename Sky → Ipê/Ipe/ipe (case-preserving; watch the naive-sed trap — upstream-Sky refs stay Sky) | Runs SOLO, dead-last before the push — no other work in flight during the rename. | `docs/superpowers/plans/2026-07-03-rename-sky-to-ipe.md` |
| | Medium | CI, oracle & publish | Publish the README (honest relation-to-Elm-and-Sky framing) | Re-run the divergences review (`docs/divergences-review.md`) first so the ledger the README cites is current. | `docs/README-draft-relation-to-elm-and-sky.md` |
| | Medium | CI, oracle & publish | E2E shared-target + cached-oracle infrastructure (queued) | | `docs/architecture/e2e-and-oracle-caching.md` |

### A-table — Hardening follow-ups

| Done at | Priority | Road map phase | Task | Notes | Spec |
|---|---|---|---|---|---|
| | Medium | Hardening follow-ups | #34 M5b-db follow-ups: SqlValue variant completeness, exhaustive `emit_db_call`, self-oracle, db-without-live build; wire `db_decode_money` into kernel dispatch (implemented + tested in runtime but unreachable — no `StdlibKernel` variant, no constrain scheme, no lower arm) | `ipe-index parity --gaps` flags it (`DbDec.money go=1 rust=0`). | `docs/architecture/class7-sql-db-fix-spec-2026-07-09.md` |
| | Medium | Hardening follow-ups | #33 M5b-http residual: header-case parity remainder + extra Http builders | Confirmed partially already fixed — the residual is the item (class8 spec §6). | `docs/architecture/class8-live-http-security-fix-spec-2026-07-09.md` |
| | Medium | Hardening follow-ups | #129 Runtime audit: `spawn_blocking` for CPU-heavy/blocking kernels (bcrypt/zstd/file) — reactor-starvation guard | | `docs/architecture/class9-kernel-robustness-fix-spec-2026-07-09.md` |
| | Medium | Hardening follow-ups | `File.readFileBytes` silently truncates at its fixed 10 MiB cap instead of erroring when the file exceeds it — sibling of the already-fixed `readFileLimit` TOCTOU (`file_read_file_bytes`'s `read_to_end` has no post-read size check) | Flagged as a side-note when `readFileLimit` was fixed (2026-07-10) but not itself addressed then. | `docs/architecture/class9-kernel-robustness-fix-spec-2026-07-09.md` |
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
