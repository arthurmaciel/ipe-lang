# Upstream audit — ../sky v0.16.29 → v0.17.2 vs ipê

## Framing

This document surveys 512 upstream commits on the reference Sky compiler
(`../sky`) spanning **v0.16.29 → v0.17.2**, whose organising theme is the
v0.17.0 **"typed-emit soundness floor"** (2026-06-28): a ground-up rebuild of
Go-type manipulation from stringly-typed passes into a structural `GoType` ADT,
plus the emission-time/runtime/fuzzer three-leg no-panic proof. ipê
(`../sky-rust`) was ported from Sky at roughly the **v0.16.6** era, so this
entire range is **un-incorporated upstream work**. Churn by area: tools 102,
docs 80, test 75, sky-stdlib 37, src (Haskell compiler) 32, runtime-go 22. The
audit judges each change strictly against ipê's principle order —
**security > correctness > soundness > efficiency > completeness > readability**
— and the two governing rules: **parse, don't validate** and **make invalid
states unrepresentable**. `../sky` is the public reference; entries state what it
added and whether ipê should adopt.

A CHANGELOG discontinuity is noted for provenance: the top entry jumps
v0.17.0 → v0.15.3, so v0.16.x curated notes and v0.17.1/.2 were recovered from
git tags + point-release merge commits.

## Main table

Sorted soundness-hardening + compiler-improvement first, then security, then
correctness/parity, then new-feature/DX.

| Change | What | Kind | Principle | ipê status | Adopt? + roadmap slot |
|---|---|---|---|---|---|
| **GoType ADT rebuild (C1..C25, 335eb524..e248b242)** | Replaced String-based Go-type manipulation with a structural `GoType` ADT + `RenderEnv` + total `renderGoType`; drained ~30 stringly passes (`parseGoType` round-trip prop test, `mapSkyTypeToGo`, `CoerceTarget` ADT, structural slot-shape destructure, `coerceCallArgsAt`) | soundness-hardening / compiler-improvement | soundness (parse-don't-validate at codegen — Go types become a parsed ADT, not re-inspected strings) | **lack** — `emit_expr.rs` still ~248 `push_str`/`format!` sites | **YES**, post-parity. Highest-leverage item; = existing **task #53**. Port the ADT + round-trip property test as the correctness oracle |
| **resolveWrapParams enclosing-scope-tvar gate (4571da08, v0.17.0 headline)** | Typed Maybe/Result/Task wrap-target no longer unconditionally overridden by source-expr HM type; consults HM only when slot-derived fallback mentions an enclosing caller type-param. 8 build errors→0 on 00-standard-libs; 26-ui-showcase rt.Coerce floor 171→74 | soundness-hardening | soundness (well-typed emit Go rejects = exit-0-then-build-fail, ipê's canonical hole class) | **maybe/verify** — Go-generics-vs-`any` specific, but the CLASS is the same monomorphization hazard | **maybe** — audit ipê generic-kernel wrap path. NEW task: "audit typed wrap-target vs slot in monomorphizer" |
| **identityRecovered caller-scope gate (38cde3e6, v0.17.2)** | `identityRecovered` self-pinned α-renamed callee tvars (`T9001→T9001`) leaking `rt.Coerce[T9001]` → `undefined: T9001`; fix gates on `enclosingTypeParamInScopeCtx`, α-renamed tvars fall to `any` so Go pins the callee generic. Hit skydeploy every deploy | soundness-hardening | soundness | **maybe** (same wrap-target class) | **maybe** — bundle with resolveWrapParams audit above |
| **Per-panic-class emission-time regression locks (v0.17.0)** | `PanicClassGateSpec` (11 tests C1-C7) + `ScopeStateRefAuditSpec` machine-verify IORef write semantics — emission-time leg of the three-leg soundness stool | soundness-hardening (test infra) | soundness | **partial** — ipê has golden + Go-oracle, not emission-time panic-class locks | **maybe** — cheap defense-in-depth for ipê's panic-class taxonomy |
| **CPS rewrites of non-tail list ops (a0b63e4e..222a4a25, Limitation #8)** | `map/filter/foldr/concat/take/append/range/zip/length/indexedMap/concatMap` + `Maybe.combine`/`Result.combine` rewritten to constant-stack CPS/accumulator `*Help` form, each with a `*Spec` stack-bound gate | soundness-hardening | soundness (unbounded stack = reachable overflow from well-typed code on 200k+ lists) | **partial/lack** — VERIFIED: `stdlib/Ipê/Core/List.ipe` still ships naive body recursion; ipê TCO (task #49) never fires on non-tail bodies → Rust stack overflow/abort | **YES, high priority** — mechanical 1:1 `.ipe` port into `List.ipe`/`Maybe.ipe`/`Result.ipe`; ipê TCO picks it up. Fold into task #49 or a new "stdlib stack-safety" slice, before example sweep signs off |
| **list-literal pattern element discriminator (9b04f9b5, #587)** | Pattern-arm discrimination on list literals | compiler-improvement | soundness/correctness | N/A-ish (Go-emit specific) | maybe — assess ipê pattern lowering separately |
| **Stage C curried-shape lambda lowering (9f783439, #590)** | Curried lambda shape lowering | compiler-improvement | correctness | N/A-ish | maybe — case-by-case |
| **sealed-iface classifier (a33cad57)** | FFI sealed-interface classification | compiler-improvement | correctness | N/A-ish | maybe — case-by-case |
| **Shell-injection fix in `ipe add`/`remove`/`verify` (b5f9dd54, v0.17.1)** | Package/dep names + verify scenario body/url/method were interpolated into `sh -c` strings; fixed to argument vectors (`proc`/`callProcessIn`, curl-via-argv, explicit cwd, no shell) | security-hardening | **security** (untrusted input crossing a shell trust boundary; argv makes the shell-interpretation path unrepresentable) | **lack/at-risk** — `ipe add` will invoke cargo on untrusted crate names (FFI tasks #40/#41) | **YES** — bind "argv only, never `sh -c`/String-composed cargo" as a hard **acceptance criterion on task #41** before `ipe add` ships. Analogue: `Command::new("cargo").args([...])` |
| **Ipe.Http.Middleware.withCsrf (78faa349, task #663)** | Double-submit-cookie CSRF middleware — `__Host-sky_csrf` 32B token, safe-methods set cookie, unsafe-methods require cookie + `X-Csrf-Token`/`_csrf` with constant-time compare, 403 on mismatch | new-feature (security stdlib) | **security** | **lack** — no `Ipe.Http.Middleware` module ported (whole `withCors/withLogging/withBasicAuth/withRateLimit` family absent); Live runtime carries its own client.js CSRF path but Server-side middleware absent | **YES**, gated on porting the Middleware module. Enforce constant-time compare + `__Host-` prefix at the Rust runtime port; keep the parse-once token boundary |
| **Db.migrate tenant-gate bypass — documented by-design (1181f856, v0.17 G4)** | Documents that `Db.migrate` runs before/outside the runtime tenant-prefix SQL WHERE-gate | soundness-hardening (trust-boundary exemption doc) | **security** | **N/A-yet** — tenant-prefix SQL gating + Db.migrate not ported | **NOTE for later** — carry the exemption doc when ipê ports `HubStoreReaderWithTenant` so migrations aren't assumed tenant-scoped |
| **Task.parallel early-cancel on first Err (04a4abf3 + 40c9f748)** | Go returns immediately on first Err *observed*, cancels shared context; siblings drain non-blocking (load-bearing test: 10ms-Err + 2s-Ok returns <500ms) | runtime/parity | correctness + efficiency (latency + wasted work) | **partial** — `task.rs:194` awaits handles in **input order**; a slow Ok at index 0 blocks return past a fast Err at index 1; dropped handles detach (not abort) so sibling effects still fire | **YES/maybe** — reimplement with `select`/`FuturesUnordered` + `JoinHandle::abort` on first Err, preserving output order for the all-Ok case. Tradeoff: gains latency, loses error-order determinism — note before adopting. Efficiency-tier, opportunistic |
| **Math.min/max preserve Float/String (4778ab9b, v0.17.1)** | Go coerced min/max args through `AsInt`, truncating Float (`[0.4…1.3]`→`0..1`, mis-scaling Ui.Chart); fixed to `skyLessThan` polymorphic comparator | runtime/parity | correctness | **HAVE + structurally immune** — `math.rs:86` `math_min<T: PartialOrd>` monomorphized; the Go bug class is unrepresentable | **no** (verify only): confirm `Math.min 0.4 1.3 == 0.4` |
| **Math.isNaN kernel (40c9f748, v0.17 G3)** | Adds `Math.isNaN : Float -> Bool` + `rt.Math_isNaN` typed route | new-feature / soundness-hardening | correctness + soundness (user NaN guards before div/compare) | **lack** — `Math.ipe:204` literally says "guard via `Math.isNaN` if added in a later release"; ipê has internal `f64::is_nan()` but no exposed kernel | **YES, trivial** — register `("Math","isNaN")` scheme `Float→Bool` + float-predicate emit route; total (`is_nan()` cannot panic). Next parity-gap batch (skydex `parity --gaps`) |
| **Uuid.v4/v7 → `Task Error String` (082fb32b, Limitation #7)** | Arity-0 entropy kernels moved off bare `: String` that made entropy look pure | soundness-hardening | soundness + correctness (effect-boundary: entropy must be `Task`) | **lack / tracked** — ipê surfaces Uuid via kernel registry; arity-0-entropy-as-pure is **task #54** | **YES** — align ipê's Uuid scheme to `Task Error String` when closing #54 |
| **List.sortWith (476dc260)** | New backend sort-with-comparator | new-feature | completeness | **HAVE + ahead** — `list.rs:195` `list_sort_with` with a total-order-consistency guard that stays total on an inconsistent comparator (hardening Go lacks) | no — already present |
| **String.dropLeft/dropRight (861d425a, #544/#132)** | Elm-shaped rune-based | new-feature | completeness | **HAVE** — `String.ipe:184-191`, rune-based, parity tests present | no |
| **Webview view `model -> any` → `model -> Html` (44787c3a)** | Closed-record view sig, clean missing-field errors | soundness-hardening | soundness | **HAVE** — matches ipê Phase-1d webview gate | no |
| **insertFieldsReturning (18f58267, #586)** | `INSERT … RETURNING <projection>` decoded via `Ipe.Db.Decode` | new-feature | completeness | **lack** — Db un-ported at stdlib level | **YES**, bundled with eventual `Ipe.Db` port (not standalone); pair with the `SqlFragment` param-newtype hardening queued (task #61) so RETURNING projections can't reopen injection |
| **Css.* zero-arg keyword constants as bare values (5e86c720, Limitation #9)** | `Css.zero`/`auto`/`none` no longer leak a function pointer; bare misuse is a clean type error | soundness-hardening | soundness | **lack** — no Css module; ipê **task #47** | **YES** with the Css port (#47); arity-0-clean-error shape overlaps ipê Limitation #7 codegen fix (#54) |
| **Property-based well-typed fuzzer (9e170314, `fuzz-well-typed.sh`)** | Generates random well-typed Ipê programs; asserts `build && run` doesn't panic; 6 HM-valid templates, deterministic LCG seeding, ≥10k iters target | tooling/DX (soundness-verification infra) | soundness (the "real-world leg" of the three-leg no-panic proof) | **partial/lack** — ipê has WellTypedFuzzer reference (M7 gate) + golden oracle, no random-generator harness | **YES** — after 5-shape/CI-sweep parity (tasks #35/#37), before FFI. Cheap to port the 6 templates; deterministic-seed reproduction fits the log-and-re-read discipline |
| **`sky build` refuses repo-root run (18e2b44e, task #662)** | cwd guard: if `sky-compiler.cabal` present, exit 1 with cd-hint instead of overwriting the compiler binary | tooling/DX (footgun-guard) | correctness/DX (make destructive state unreachable) | **mostly N/A** — ipê is non-self-hosting Rust; root build hits cargo target, not `sky-out/` | **maybe (low)** — the pattern (cheap cwd guard refusing a destructive invocation with a clear diagnostic) is good hygiene; opportunistic when `ipe build` UX is touched |
| **`default*`/`with*` builders for Csv/Email/Ui.Animation/Console.Identity/Db.Migration (40a04d86, 35bb0d98, 05b9809d)** | Forward-compat record-builder convention so new fields don't break call sites | tooling/DX (make-invalid-states convention) | completeness/readability | **lack** (modules un-ported) | **adopt the convention** when porting each module; not a discrete task |
| **Cited stdlib-correctness + compiler-architecture docs (2026-06-23)** | `sky-stdlib-correctness.md` (1422 lines, every guarantee cited to file:line or flagged UNVERIFIED with its proving spec) + `sky-compiler-architecture.md` (585 lines) | tooling/DX (documentation) | readability/completeness | **partial** — ipê has divergence ledgers + guardian memory, no single cited-per-claim reference | **maybe (methodology)** — adopt the format (cited, UNVERIFIED-flagged, regression-spec-anchored), not Ipê content. Post-parity docs pass, aligns with LSP/kind-teacher |
| **view-panic diagnostics (04002121, d4f1fea4)** | Live view-panic recovery stack 8→40 lines + plain-log detail | tooling/DX | soundness-adjacent | **partial gap** — ipê has no per-view `catch_unwind` in `live/` (only core.rs/list.rs totality wraps) | maybe/DX (low) — a kernel-bug panic inside a live `view` isn't recovered to a structured log |
| **ipe fmt header-comment fix (889bf01f, #572)** | Formatter idempotence regression fix | tooling/DX | readability/correctness | **N/A** — ipê formatter not at this surface | no |
| **Ipe.Ui.dispatchTag sig + Haddock batches (64ea7e7d, ed89eda2, 93817c90, 2873eb79)** | Signature annotation + docstrings | tooling/DX | readability | **lack** (modules un-ported) | opportunistic, no standalone action |

## ADOPT shortlist (items that raise ipê's 6-principle / 2-rule adherence)

Ordered by principle.

1. **[SECURITY] argv-only cargo invocation** (b5f9dd54) → bind as an
   **acceptance criterion on task #41** (FFI sandbox gate). Highest-leverage
   even though `ipe add` doesn't ship yet — it prevents the vulnerable shape from
   ever landing. Rule: a crate/dep name must never reach a shell; use
   `Command::new("cargo").args([...])`.
2. **[SECURITY] withCsrf + Middleware module** (78faa349) → NEW task:
   "port `Ipe.Http.Middleware` (withCors/withLogging/withBasicAuth/withRateLimit
   + withCsrf)"; enforce constant-time compare + `__Host-` prefix + parse-once
   token at the Rust runtime port.
3. **[SOUNDNESS] CPS stack-safety rewrites** (a0b63e4e..222a4a25) → fold into
   **task #49** (TCO) or a NEW "stdlib stack-safety" slice. Mechanical 1:1 `.ipe`
   port; converts a class of well-typed-code Rust stack-overflow aborts into
   constant-stack execution. Do before the example sweep signs off (large-list
   examples would otherwise abort).
4. **[SOUNDNESS] GoType ADT typed-emit rebuild** (C1..C25) → **task #53**
   (already filed). Post-parity; the single highest-leverage soundness item —
   closes the exit-0-then-cargo-fail class by parsing Go types once into an ADT.
   Port the round-trip property test as the oracle.
5. **[SOUNDNESS] Uuid.v4/v7 → `Task Error String`** (082fb32b) → align when
   closing **task #54** (arity-0 entropy-as-pure hole).
6. **[SOUNDNESS] well-typed fuzzer** (9e170314) → NEW task: "port property-based
   well-typed no-panic fuzzer (6 templates, deterministic seed)"; after CI-sweep
   parity (#35/#37), before FFI.
7. **[SOUNDNESS] Css.* bare zero-arg constants** (5e86c720) → adopt with the Css
   port, **task #47**.
8. **[CORRECTNESS] Math.isNaN kernel** (40c9f748) → NEW trivial task: register
   `("Math","isNaN") : Float → Bool`; closes a self-documented gap
   (`Math.ipe:204`).
9. **[CORRECTNESS/EFFICIENCY] Task.parallel early-cancel** (04a4abf3) →
   opportunistic; reimplement `task_parallel` with `FuturesUnordered` +
   `JoinHandle::abort` on first Err, preserving all-Ok output order. Note the
   error-determinism tradeoff.
10. **[COMPLETENESS] insertFieldsReturning** (18f58267) → adopt bundled with the
    `Ipe.Db` stdlib port; pair with the `SqlFragment` newtype (task #61).
11. **[DX] audit typed wrap-target vs slot** (4571da08 + 38cde3e6) → NEW audit
    task: verify ipê's monomorphizer wrap path can't emit a wrap-target that
    mismatches the slot / leak an α-renamed tvar.
12. **[DX] cited stdlib-correctness doc format** (2026-06-23) → methodology
    adopt in the post-parity docs pass.

## ipê already has / already more principled (do NOT re-adopt)

- **Math.min/max Float-preserve** — ipê's `math_min<T: PartialOrd>` is
  monomorphized; the Go `AsInt`-truncation bug class is unrepresentable.
- **List.sortWith** — present + ahead (`list.rs:195` stays total on an
  inconsistent comparator; Go lacks that guard).
- **String.dropLeft/dropRight** — present, rune-based, parity-tested.
- **Webview view sig `model -> Html`** — matches ipê's Phase-1d gate.
- **CSRF double-submit** — `live/csrf.rs` implements it and goes beyond Go by
  using the `__Host-` prefix only when cookies are Secure.
- **slider Number→String narrowMsgArg panic** (277ee217) — N/A-Go-only; ipê
  types every wire-event arg as `String` via typed `Event::OnString`/`OnBool`
  closures, not `reflect.MakeFunc` over arbitrary ctor arg types. The
  Float64-into-String-ctor state cannot occur (parse-don't-validate at the wire
  boundary).
- **msgDisplayName silent-drop on reflect.MakeFunc** (46b7eaf7) — N/A-Go-only;
  dispatch keys on `(sky_id, event)`, never `FuncForPC` name reflection.
- **SkyVariant / MsgDispatch typed per-Msg dispatch** (P1–P4) — N/A-Go-only; ipê
  dispatches on typed enums natively.
- **Go-toolchain/CI plumbing** (f85133f7 darwin inlining, 3408cca1 macOS
  timeout, 77fef99e CI self-test guard) — N/A for a Rust backend.

## Un-incorporated-range note

This audit covers **v0.16.29 → v0.17.2** only. The intervening
**v0.16.7 → v0.16.29** range is likewise un-incorporated upstream work (ipê
forked at ~v0.16.6) but was **out of scope** for this pass — it includes the
v0.16.7/#417 + v0.16.8/#423 `init req` context expansion, the v0.16.13
parametric-record-alias `identityRecovered` path (whose v0.17.2 gate 38cde3e6 is
audited above), the v0.16.16 sky-nav `r.ok` session-lost recovery, the v0.16.24
Db Maybe-param / kernel-implicit-Prelude-type / nullable-decoder closures, and
the v0.16.26/#582 `SqlValue`/`SqlField` typed-parameter work. A follow-up pass
should audit that range on the same principle basis before the FFI milestone.
