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
| 2026-07-10 | #109/#156 `Ui.onSubmit`/`Std.Html.Events.onSubmit` dispatch via `Event::OnForm`; `Arc<dyn Any>` OnRaw removed — zero `dyn Any` in emitted-code paths; #90 SKY-L0114 ctor-payload-function Stage 1 — `Ok`/`Just` holding a function (construction, declared payload, `andMap` arity-1) now lowers and runs; new SKY-L0127 fn-carrier-reuse residual gate; `Sky.Test`'s own `Leaf String (() -> TestResult)` now compiles (new ex00 blocker: SKY-L0115 tuple-pattern-match); Boundary Scheme Promotion (class-1 inference bug #2) core fix — cross-module untyped-binding generalization via `promote_untyped_boundaries`, 8 new `sky_types` tests + manual 3-module E2E; #63 `Sky.Http.Middleware.withCsrf` — double-submit CSRF for Sky.Http.Server + `ServerResponse` multi-`Set-Cookie` fix |

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
| | Medium | Sweep to green | Boundary Scheme Promotion follow-up — add multi-module fuzz templates (cross-module 2-type reuse, value binding, Number-bounded helper, recursive pair) to `scripts/fuzz-well-typed.sh` / `fuzz-ill-typed.sh`, which today only emit single-file `Main.sky` projects | [progdev-safe] Core fix landed 2026-07-10 (`promote_untyped_boundaries`, `crates/sky_types/src/constrain.rs` + `lib.rs`, lowering in `crates/sky_lower/src/lower.rs`); full sky_types unit test matrix (8 new tests, cross-module accept + same-module/Super/obligation-gated reject cases) passes, plus a manual 3-module skyc→cargo build→cargo run E2E confirmed the seal holds and Rust generics (`fn lib1_ident<T1: Clone>(x: T1) -> T1`) emit correctly. This row is ONLY the fuzzer-harness extension the original spec listed as a pre-landing nice-to-have; it needs new multi-file-template infrastructure in the fuzz scripts (today every template writes a single `src/Main.sky`), not just a new template function. | `docs/architecture/class1-inference-fix-spec-2026-07-09.md` |
| | High | Sweep to green | SKY-L0115 tuple-pattern-match — a tuple `case` with more than one arm / a refutable element is rejected (`Sky.Test.summarise`: `case pair of ( _, Passed ) -> [] ; ( name, Failed _ ) -> [ name ]`) | New ex00 first blocker (`Sky.Test:163`) after #90 landed 2026-07-10 (SKY-L0114 ctor-payload-function lifted for `Maybe`/`Result`/user-union heads — verified: `Sky.Test`'s own `Leaf String (() -> TestResult)` now lowers cleanly, the sweep advances to this unrelated gap). Needs the richer product/literal-pattern exhaustiveness machinery `Feature::TuplePatternMatch`'s own doc comment names as the follow-on beyond M3b-1's single-irrefutable-destructure support — no existing design doc found for this specific gate; likely needs its own design pass, not mechanical. | |
| | High | Sweep to green | #158 Nested-constructor-payload function-argument patterns (`f (Just (h :: t)) = …`, `f (Ok {name}) = …`) are fail-closed (SKY-L0112/SKY-L0116) where the reference recurses and compiles | Correct fail-closed behavior; the completeness gap itself is the item (divergence A13). | `docs/architecture/class4-pattern-lowering-fix-spec-2026-07-09.md` |
| | High | Sweep to green | #99 Refutable match-arm alias over non-Copy payload double-moves — `case m of Just ((a,b) as w) -> use a,b,w` → E0382 | Pre-existing, out of #96 scope. | `docs/architecture/seal-noncopy-move-design.md` §4.2 + `docs/architecture/class5-emitter-clone-fix-spec-2026-07-09.md` |
| | High | Sweep to green | #125 Decoder thunk coverage: tuple-destructure + record-field binders (loud E0382) | Pre-existing. | `docs/architecture/class5-emitter-clone-fix-spec-2026-07-09.md` |
| | Medium | Sweep to green | #102 F1 shadow diagnostic: local `type X` shadowing a dep-imported `X` → downstream SKY-T0001 instead of clean SKY-N0012 at the decl | Low-risk, fail-closed today. | `docs/architecture/class4-pattern-lowering-fix-spec-2026-07-09.md` |
| | High | Sweep to green | #113 Pseudo-class attrs render to nothing in the static `htmlRender` sink (AttrPseudoRule no-op) | | `docs/architecture/class10-ui-html-fix-spec-2026-07-09.md` |
| | Medium | Sweep to green | #105 Std.Css hardening (defence-in-depth): optional @import/expression gating on `raw`/`keyframes` bodies + reject CSS-hex-escaped values in `safeValue` | | `docs/architecture/class10-ui-html-fix-spec-2026-07-09.md` |
| | High | Sweep to green | #32 M5a follow-ups (fail-closed): Task arity-3 ICE + Task-in-ADT-ctor gate | | `docs/architecture/class4-pattern-lowering-fix-spec-2026-07-09.md` |
| | High | Sweep to green | #56 Prove row-poly subset/superset record resolution (A7 watch) + gate on sweep | Investigated 2026-07-10: no defect found, all reachable subset/superset shapes resolve or fail-loud in parity with the reference. Remaining work is purely additive: wire the spec's 5 golden-test fixtures into `crates/skyc/tests/golden_row_poly_records.rs` to pin the invariant on the sweep — no compiler code changes. | `docs/architecture/row-poly-subset-superset-design.md` |
| | High | Sweep to green | #45 Make the constrain kernel-scheme table exhaustive over canon lists (close the exit-0-then-cargo-fail class) | | `docs/architecture/class3-kernel-registry-fix-spec-2026-07-09.md` + `docs/architecture/html-ui-live-scheme-table.md` + `docs/superpowers/plans/2026-07-03-registry-phase-E.md` |
| | High | Sweep to green | #70 Fix kernel arity-table drift (`decl().arity` vs `callee_arity`) — latent exit-0-then-cargo-fail | | `docs/architecture/class3-kernel-registry-fix-spec-2026-07-09.md` |
| | Medium | Sweep to green | #85 ErrorDetails follow-up: port `ErrorInfo.details : Maybe ErrorDetails` + the 5-variant `ErrorDetails`/`PanicInfo`/`TypeInfo` union | Additive; same registration recipe as `ErrorKind` (canon ctor registration + lowerer arms + `IrType` leaf + `builtin_runtime_enum` + constrain ctor schemes). Core `Error ErrorKind ErrorInfo` ADT landed 2026-07-09. | `docs/architecture/error-module-design.md` |
| | High | Sweep to green | Remaining Std.Ui / Std.Html kernel gaps surfaced by the example sweep — wire missing kernels across the layers with the `../sky` reference | [progdev-safe] Per-example blockers live in `docs/architecture/remeasure-snapshot.tsv`; computed gaps via `scripts/ipe-index parity --gaps`. Template = the landed `Border.shadow`/`Border.glow`/`Border.innerShadow` wirings. | `docs/architecture/ui-html-completeness-design.md` |

### A-table — Security hardening

| Done at | Priority | Road map phase | Task | Notes | Spec |
|---|---|---|---|---|---|
| | High | Security hardening | #44 Opaque `Secret` stdlib type (gates WASM hydration island + secrets-are-typed rule) | | `docs/architecture/class6-secret-sqlfragment-fix-spec-2026-07-09.md` |
| | High | Security hardening | #61 `SqlFragment` param-query newtype — SQL injection = type error | | `docs/architecture/class6-secret-sqlfragment-fix-spec-2026-07-09.md` |
| | High | Security hardening | Class-8 web remainder: session cookie `Secure` TLS-gated (not ENV-gated), `/_sky/observability/ingest` CSRF exemption, WS upgrade Origin check outside production (CSWSH), `live_max_body_bytes()` `>0` floor | From the AUD-09 gap-sweep. Now a 2-cookie issue: #63's new `__Host-sky_csrf` cookie (`csrf_set_cookie_value`, `runtime/src/sky_runtime/server.rs`) inherits the same ENV-vs-TLS gating — code comment there acknowledges it, review of #63 (2026-07-10) confirmed it's real (a forgotten `ENV=production` on an actually-TLS'd deploy ships the cookie without `Secure`/host-lock on BOTH the session and CSRF cookies). Fix once, apply to both call sites. | `docs/architecture/class8-live-http-security-fix-spec-2026-07-09.md` |
| | High | Security hardening | #63b `withCsrf`'s golden/E2E coverage never issues a real HTTP request — `golden_m6_middleware_csrf.rs` only checks `skyc`/`cargo build` succeed; `server_e2e.rs` has zero CSRF tests. The spec's own §1.3 "Regression-test summary" (row 1) calls for exactly this (forge a cross-site POST with no header → assert 403; legit POST with matching header → assert 200) and it wasn't added. The 4 in-process unit tests in `server.rs` call `middleware_with_csrf` directly, bypassing the 12-site kernel-registry chain — a kernel-wiring regression there would currently go undetected by CI. | Found by independent review of #63 (2026-07-10). | `docs/architecture/class8-live-http-security-fix-spec-2026-07-09.md` |
| | High | Security hardening | Class-7 SQL/DB remainder: `SqlNull` text-typed NULL breaks Postgres, Postgres driver structurally unreachable, `db_insert_row` fabricated `id=0` on non-integer PK, tenant-prefix SQL enforcement absent from plain `db.rs` | From AUD-09 + gap-sweep. The `url_is_cacheable` DoS-reopen clause of this row was fixed 2026-07-10 (see done-ledger row below) — dropped from this Task's wording. | `docs/architecture/class7-sql-db-fix-spec-2026-07-09.md` |
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
