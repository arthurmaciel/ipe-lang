# Adopt from Ipê v0.17.2 (`feat/runtime-rust`) — synthesized roadmap

> **Status:** filed roadmap. Source: upstream `/home/arthur/Documentos/comp/sky`
> at branch `feat/runtime-rust`, tag **v0.17.2** — a COMPLETE, working Rust
> backend (Haskell that emits Rust) + Rust runtime that builds AND runs every
> applicable Ipê example (37 green / 0 red full Go-parity in their sweep).
> This doc synthesizes six facet audits into ADOPT / ADAPT / SYNC / REJECT
> verdicts, a runtime-sync plan, prioritized actions, and backlog changes.
> Critiqued in STRICT principle order: (1) security (2) correctness
> (3) soundness (4) efficiency (5) completeness (6) readability, holding
> "PARSE, DON'T VALIDATE" and "MAKE INVALID STATES UNREPRESENTABLE" as
> non-negotiable design rules.

---

## 1. Honest framing — what v0.17.2 gives us, and what NOT to copy

v0.17.2 is a **strong prior, not a blueprint.** It proves a Rust backend +
runtime can build and run the full example corpus at byte-parity with the Go
backend. That converts several of our open questions from "is this even
possible?" into "here is a working shape to measure ours against." But the two
codebases are asymmetric across the two comparison axes:

- **Axis A — RUNTIME (`../sky/runtime-rust/src/sky_runtime`, 51 top-level `.rs`
  + subdirs, vs our `runtime/src/sky_runtime`, 51 top-level `.rs`).** Both are
  Rust. Their runtime is the *upstream* of ours — we vendored an older cut.
  This code can sync **~verbatim**, function-for-function, because it is the
  same language and the same contract surface. This is where the cheap, high-
  value wins live (escape kernels, HTTP header canonicalisation, FFI redaction
  boundary).

- **Axis B — BACKEND.** Theirs is **Haskell that emits Rust**
  (`../sky/src/Sky/Generate/Rust` + `Build/Rust`). Ours is a **native Rust
  reimplementation** (`crates/sky_backend_rust` + the ipe pipeline
  `sky_canon`/`sky_types`/`sky_lower`). Here the **logic transfers but the code
  does not.** More importantly, our architecture differs *fundamentally*: the
  Go/Haskell backend postpones type resolution to **emit time** (untyped AST +
  `CallInstance` monomorphization record + `coerceCallArgsAt` substitution),
  whereas we lower to a **typed IR (`sky_ir::Expr` with concrete `IrType`)** at
  **lower time** (`sky_lower`). Several of their most celebrated recent fixes
  — the `coerceCallArgsAt` identity-recovery gate, the T9000-space α-rename
  leak guard — are **mechanisms that exist only because they resolve types
  late.** We do not have that class of bug, so we must **not** port those
  mechanisms; doing so would import complexity to solve a problem our
  architecture already dissolves.

**What we KEEP that is strictly better than theirs.** A working upstream does
not override a more sound local decision:

- Our **typed SHOW obligation** (record-derive validation at lower time,
  planned) beats their untyped emit-then-hope. They emit `#[derive]`
  unconditionally and lean on Rust cargo to catch it — a post-exit-0 failure.
- Our **auth hardening** already exceeds theirs on two fail-closed vectors
  (negative-TTL rejection, id-decode fail-closed) where upstream silently
  clamps / defaults to user-0. Retain both; document as audit wins.

**The one-line thesis:** SYNC the runtime aggressively (same language, same
contract); MIRROR the backend *logic* selectively (derive gate, HOF closure
capture) while REJECTing their late-resolution machinery; and never regress a
place where our fail-closed / parse-don't-validate posture already wins.

---

## 2. RUNTIME SYNC recommendation

**Verdict: cherry-pick, NOT sync-wholesale.** A wholesale `cp -r` of their
`runtime-rust/src/sky_runtime` over ours would be *unsafe* for three reasons:

1. **Our vendored runtime carries hardening deltas upstream lacks** —
   `auth.rs` negative-expiry rejection + fail-closed id-decode, and `+9` lines
   of security docs on `AttrAttribute` in `ui/element.rs`. A wholesale
   overwrite would *silently revert* our security wins. That violates
   principle (1).
2. **Our runtime has files upstream's cut may gate differently** — e.g.
   `system.rs` must be **always-compiled** (process-global env `RwLock`), and
   `jwt.rs` needs `cfg(all(feature="json", feature="crypto"))`. Upstream's
   feature-gating here is *wrong* (system tokio-gated, jwt json-only). A
   verbatim sync would import those gating bugs.
3. **The ipe emitter's expectations are a contract.** Our backend emits code
   that calls specific runtime symbols with specific signatures (e.g. the
   `impl Fn(..) + Clone` HOF bounds on `list.rs`). A runtime file that changed
   a signature upstream would break our emitter silently — cargo-fail, not
   type-fail. Every synced file must be **diffed against emitter call sites**,
   not blindly replaced.

### File-level plan (security-first ordering)

| Priority | File(s) | Action | Risk / gate |
|---|---|---|---|
| **P1 — security** | `css_safety.rs`, `css.rs` | **AUDIT byte-identical** to upstream render/escape kernels. Any drift = XSS. | Escape-policy mismatch is a live XSS vector. Diff their `SafeCssValue`/`SafeCssPropertyName`/`SafeCssSelector` against ours; reconcile toward the stricter. Closes #47, #76-T2. |
| **P1 — security** | `core.rs` | **PORT** `sky_error_from_foreign` + `log_foreign_error` + `scrub_log_controls` + `short_err_id` (upstream lines 31–94, `[B8 SECURITY]`). | Foreign-error `Debug` MUST NOT reach Sky — redact to `…(ref <id>)`. Gate FFI Phase 0 (#40) on this being in the emitted preamble. |
| **P2 — refactor** | `http_header.rs` | **KEEP** ours (we already extracted `canonical_header_name`; upstream leaves it inline). Verify `live/` + `server.rs` both route through it. | No code change; confirm exports + call sites. |
| **P2 — parity** | `list.rs` | **VERIFY SYNCED** — HOF signatures (`foldl`/`foldr`/`filter`/`any`/`all`/`concat_map`/`indexed_map`/`list_sort_by`/`list_sort_with`) carry `impl Fn(..) + Clone`. Finding reports byte-identical. | No change; signature contract is load-bearing for emitter. |
| **P2 — parity** | `ui/element.rs`, `html.rs` | **KEEP ours** (byte-faithful + our defensive docs). Verify hand-impl `Debug`/`PartialEq` on `Attribute`/`Event` (closure-ignoring structural eq) survives; emitter must NOT `#[derive]` over them. | Custom `PartialEq` matches Ipe.Live diff-only semantics; sound. |
| **P3 — hold** | `auth.rs` | **DO NOT SYNC** the diverging arms — retain our negative-TTL rejection (137–143) + fail-closed id-decode (318–325). | Our version is *more sound*. Document as hardening delta in the audit trail so a future sync doesn't clobber it. |
| **P3 — hold** | `basics.rs` (Error kernels), `crypto.rs` (constant-time) | **KEEP ours** — already v0.17.2-equivalent. | No change. |

**Contract-safety conclusion:** a cherry-pick preserves our vendored-runtime
contract and our emitter's expectations; a wholesale sync would break both. The
sync is *mechanically small* (2 security ports + a handful of audits), because
the runtime already largely converged — the divergence is in **backend
codegen**, not runtime.

---

## 3. ADOPT / ADAPT / SYNC / REJECT table

| # | Learning (facet) | Verdict | Our action / task |
|---|---|---|---|
| 1 | `system.rs` always-compiled for global env `RwLock` (runtime-sync) | **ADOPT** | Keep always-compiled; verify comment only. |
| 2 | JWT needs BOTH `json`+`crypto` features; upstream json-only is a latent build bug (runtime-sync) | **ADOPT** | Keep `cfg(all(json,crypto))`; audit `crate-specs.toml` emits both. |
| 3 | `css_safety.rs`/`css.rs` escape kernels (runtime-sync) | **SYNC** | Byte-identical audit vs upstream. **Security P1.** #47, #76-T2. |
| 4 | `http_header.rs` extracted-shared policy (runtime-sync) | **SYNC** | Keep ours; verify call sites. |
| 5 | Non-derivable record derives — upstream emits unconditional, cargo-fails (runtime-sync + SEAL) | **ADAPT** | Add `is_derivable`/`has_fn_field` gate; emit `#[derive]` only when proven derivable. **Closes #87.** |
| 6 | Ipe.Ui/Html completeness via `PortStatus{Backed\|Deferred}` manifest (runtime-sync) | **SYNC** | Land **Batch 0** (`VarHome::Kernel(Backing,..)`) atomically, then T1–T5. **#76.** |
| 7 | `coerceCallArgsAt` identity-recovery gate — recover type identity after α-rename at emit time (SEAL) | **REJECT** | N/A to our architecture: we lower to typed IR; instantiation already concrete. No port. |
| 8 | T9000-space α-rename T-var leak guard (SEAL) | **REJECT** | N/A: no separate α-rename step; IR type vars are `Symbol`-keyed in `GenericScope`. |
| 9 | Record-field function-type derive gate (`hasFnField` → `#[derive(Clone)]` only + manual `impl Default`) (SEAL) | **ADOPT** | Implement `has_fn_field()` in `emit_types.rs::emit_record_struct`; reduce derives + emit `disconnected_fnN()` Default. **Closes #87 (the correctness bug).** |
| 10 | Transitive `hasCallbackField` — struct holding an fn-field struct inherits the restriction (SEAL) | **ADAPT** | Thread an fn-field-struct registry through `EmitCtx`; cascade the Clone-only reduction. Follow-up if the sweep surfaces nested cases. |
| 11 | `#[serde(skip)]` on fn-fields + `Default`-reconstruct on deserialize (SEAL) | **ADAPT** | Gate on `ctx.uses_live && has_fn_field`. Low priority; Live-model-with-callback pattern only. File follow-up. |
| 12 | Error constructors are **kernels**, not source modules; `Error=String` so all 8 message ctors → one `sky_error_from_message` (Error) | **SYNC** | Already wired (#86 in flight). Verify lower/backend emit the kernel calls. |
| 13 | FFI boundary `E: From<String>` + correlation-ID redaction; foreign `Debug` never reaches Ipê (Error) | **ADOPT** | Port `sky_error_from_foreign` et al. into emitted preamble. **[B8 SECURITY]**, gates #40. |
| 14 | Nullary Error ctors → distinct hardcoded strings; `withMessage` = `fn(new,_)→new` (Error) | **SYNC** | Already have; no change. |
| 15 | Source-module Error approach infeasible (circular dep); kernel-path breaks the cycle → #85 parked (Error) | **ADOPT** | **Close #85** with note: runtime stays String-backed kernel-dispatched; rich ADT lives in `Ipe.Error.ipe` but ctors lower to kernels. |
| 16 | `runtimeOpaqueTypes` registry `{M}` bridge; `RPubUseAlias` vs `RAliasDefGen` (Ui/Html) | **SYNC** | Verify `emit_types.rs` has all 10+ Ipe.Ui entries + correct alias-kind split. |
| 17 | `synCtor` auto-ctors for non-opaque record aliases (Ui/Html) | **ADOPT** | Confirm IPE-N0001 (#82) covers it; else implement synCtor equivalent. **Validate our just-shipped IPE-N0001 vs theirs.** |
| 18 | `SkyStringify` placeholder-string strategy on opaque containers, no `M` recursion (Ui/Html) | **SYNC** | Runtime done. Emitter must gate `#[derive(SkyStringify)]` off non-Clone fields → same **#87** gate. |
| 19 | ~160 unbacked Ipe.Ui/Html members are pure-Ipê builders + kernels, NOT runtime ADT gaps (Ui/Html) | **REJECT** (as runtime issue) | Reframe #76 as codegen/kernel wiring: audit `sky_canon` registry for `id=None` members; implement missing kernels. |
| 20 | #84 `html_p_` wrong-tag is Ipe.Ui codegen tag-mapping, not runtime render (Ui/Html) | **REJECT** (as runtime issue) | Fix in the element-builder codegen; `render_into_ctx` is correct. |
| 21 | Three-gate non-Clone Task capture (single-use move / all-discard Arc-wrap / residual thunk-closure) (HOF) | **ADOPT** | Port to `emit_expr.rs` lambda emission (replaces bare `Box::new(move ...)`). Soundness gate vs E0599. Ties to #87. |
| 22 | `collectLambdaCapturedVars` precision — only walk captures INSIDE lambdas, don't Arc-wrap stored handlers (HOF) | **ADOPT** | Port precision; verify we don't over-Arc-wrap non-captured handlers in record fields. Lower priority. |
| 23 | Record-literal resolution by field-name set during emission (HOF) | **SYNC** | Already identical (`record_name_for_literal`). #82 done. No change. |
| 24 | Multi-use non-Clone capture handled at binding site vs our lowering-time handling (HOF) | **REJECT** (1-for-1 port) | Verify our lowerer surfaces usage-count/discard info for emit-time gates; port gates only if lowering doesn't already solve it. |
| 25 | `emitDefaultCall` peephole is narrow (only `Task.retryWith`), not a record-ctor path (HOF) | **SYNC** | If we add a peephole, mirror the strictness — specific kernels only, never record ctors. |
| 26 | 6-class panic-lint denials identical both repos (soundness) | **SYNC** | No change; pre-commit `clippy -D` active. |
| 27 | Constant-time eq is a library kernel (`subtle::ct_eq`), not codegen-emitted; no Secret newtype upstream (soundness) | **SYNC** | Keep ours identical. |
| 28 | Our negative-expiry rejection beats upstream silent clamp (soundness) | **ADOPT (keep ours)** | Retain `auth.rs:137–143`; document as security hardening. Closes a negative-TTL DoS upstream doesn't defend. |
| 29 | Our fail-closed id-decode beats upstream `unwrap_or(0)` silent user-0 auth (soundness) | **ADOPT (keep ours)** | Retain `auth.rs:318–325`; **closes #29** silent-auth-bypass vector. |
| 30 | #87 is NOT lint-preventable; it's an emit-time gap (soundness) | **REJECT** (lint strategy) | Fix via derive gate (#9 above) + exhaustive kernel-scheme table (#45). No new lints. |
| 31 | CLI determinism auto-probe (run Go twice, downgrade not false-DIFFER) (build-verify) | **SYNC** | Already ported (`examples-sweep.sh`); phase-2 wiring under #51. |
| 32 | Server per-route determinism gate (curl Go twice per route) (build-verify) | **SYNC** | Already ported (`checks.sh:276–355`); phase-2 under #51. **Deepest correctness win — non-negotiable.** |
| 33 | HTML normalizer canonicalises legitimate backend freedom (build-verify) | **SYNC** | Ported verbatim; phase-2. |
| 34 | DERIVED equivalence-mode + overrides-only manifest (build-verify) | **ADOPT** | Ported; wire phase-2 comparison step. #51. |
| 35 | `go-ref-broken` AMBER clause (upstream bug ≠ Rust regression) (build-verify) | **ADOPT** | Verdict logic ported; triggers when EQUIVALENCE≠—. #51. |
| 36 | Resumable numbered-phase state file (not commit-derived) (build-verify) | **ADOPT** | Adopt for a future phase-2 `keep-go-parity.sh`. #51. |
| 37 | Night-gate as soft opt-in (our adaptation) (build-verify) | **ADAPT (keep ours)** | CI shouldn't be soft-gated; opt-in default is correct. No change. |

---

## 4. Prioritized action list + backlog changes

### Top actions (do these first, in order)

1. **#87 — Emit the record/enum derive gate (SEAL HOLE, correctness bug).**
   Confirmed live: `emit_types.rs:460` (record) and `:349` (enum) emit
   `#[derive(Clone, Debug, PartialEq{serde_derives})]` **unconditionally**.
   When a field lowers to `IrType::Fun` → `Box<dyn Fn>`, cargo fails
   post-exit-0 (E0277/E0599). Add `has_fn_field(&[(String, IrType)]) -> bool`;
   reduce to `#[derive(Clone)]` when true; emit a manual `impl Default` using
   `disconnected_fnN()` for bare fn fields. Prefer encoding the property in the
   IR — **add `is_derivable: bool` to `sky_ir::RecordType`, computed at
   lowering (`embeds_nonderivable_function`)** — so the emitter reads a proven
   fact rather than re-deriving it (MAKE INVALID STATES UNREPRESENTABLE: an
   un-derivable struct can't reach a `#[derive]` site). This is the single
   highest-leverage change: it seals the exit-0-then-cargo-fail class and
   unblocks any Model/record holding `Decoder`/`Cmd`/`Sub`/`Task`/callbacks.

2. **Port the two `[B8 SECURITY]` runtime pieces (`core.rs` FFI redaction +
   `css_safety.rs` escape audit).** Security-first (principle 1). Foreign-error
   `Debug` redaction to `…(ref <id>)` must be in the emitted preamble before
   FFI Phase 0 (#40); the CSS escape kernels must be byte-identical or we ship
   an XSS vector. Both are cheap same-language ports/audits.

3. **Port the three-gate non-Clone HOF closure-capture pattern to
   `emit_expr.rs`.** Replaces the bare `Box::new(move |..| ..)` with
   single-use-move / all-discard-Arc-wrap / residual-thunk-closure selection.
   Closes our HOF closure-emission soundness gap (E0599 on multi-use non-Clone
   captures) and likely unblocks several examples — but **first verify** the
   lowerer isn't already solving this earlier; if it converts non-Clone Tasks
   to thunks at lower time, this reduces to a verification task.

4. **Land Ipe.Ui/Html #76 Batch 0** — migrate `VarHome::Kernel(Option<...>)`
   → `VarHome::Kernel(Backing, ..)` in `sky_canon/src/env.rs` as one atomic
   commit (tree won't build until every `id=None` becomes `Deferred`), making
   an unbacked member **unrepresentable** (PARSE, DON'T VALIDATE). Then wire
   T1–T5 kernels sequentially. Reframe #76 explicitly as **codegen/kernel
   wiring**, not runtime completeness — the runtime ADTs are already complete.

5. **Complete #86 + close #85.** Verify lower/backend emit kernel calls for
   `Error.unexpected`/`Error.toString`/etc. (canon + kernels already wired).
   Then **close #85** with the parked-by-design note: runtime stays
   String-backed, kernel-dispatched; rich ADT lives in `Ipe.Error.ipe`
   source but constructors lower to kernels, never runtime constructors.

6. **Wire #51 phase-2 EQUIVALENCE** once the Haskell `sky` binary is on PATH as
   `IPE_GO_BIN` — flip `IPE_SWEEP_NO_EQUIV=0`. All normalizers, the AMBER
   `go-ref-broken` discrimination, and the per-route determinism gate are
   already ported; only the comparison step needs activation.

### Backlog changes (does v0.17.2 change our plan?)

- **#76 (Ipe.Ui/Html, ~160 unbacked members):** **Re-plan, do not re-scope.**
  v0.17.2 confirms our `PortStatus` manifest is a Pareto-optimal interim and
  their pure-Ipê compilation is the north star (reachable only once the
  canonicaliser gains polymorphism). Runtime ADTs are already complete —
  reframe the task as kernel wiring + Batch 0 atomic migration. **Revise
  `docs/architecture/ui-html-completeness-design.md`** to state the
  runtime-complete / codegen-incomplete split explicitly and cite v0.17.2's
  ~43 rendering kernels as the target count.

- **#85 (Error rich-ADT):** **Close as parked-by-design.** v0.17.2 validates
  the kernel-path over the infeasible source-module route. Runtime
  representation stays `SkyError = String`. **Revise
  `docs/architecture/error-module-design.md`** to record: rich ADT is
  post-parse validation deferred to v0.18+, not a runtime type change.

- **#86 (minimal Error qualifier, in flight):** **Keep; nearly done.** Our
  design already mirrors theirs exactly. Only verification of lower/backend
  kernel emission remains.

- **#80 (Test/Cli/Stream/ToString module ports):** **Unchanged.** v0.17.2 has
  these working as pure-Ipê-onto-kernels; no new mechanism needed. Proceed on
  our existing plan.

- **#87 (derive over non-derivable fields → ipe-0-cargo-fail):** **Re-plan
  and elevate to top priority.** v0.17.2 gives us the exact fix (derive gate +
  IR `is_derivable`). This is a confirmed pre-existing seal hole. **Revise
  `docs/architecture/html-ui-live-scheme-table.md` and any seal doc** to make
  the derive gate part of the seal contract: *no exit-0 without a proven
  cargo-buildable derive set.*

- **#51 (Go-oracle equivalence harness):** **Unchanged plan, de-risked.** v0.17.2's
  full-parity sweep (37/0) plus our already-ported normalizers means phase-2 is
  a wiring step, not a design task. Keep the per-route determinism gate as
  non-negotiable.

- **The SEAL generally:** **Strengthen the contract.** The exit-0-then-cargo-
  fail class is closed by the #87 derive gate + exhaustive kernel-scheme table
  (#45). **Revise the record-alias-ctor design (`record-alias-ctor-design.md`)**
  to validate our just-shipped IPE-N0001 auto-ctors against upstream `synCtor`:
  confirm we skip `runtimeOpaqueTypes` + marker-only AppCfg configs and guard
  duplicate names — the two exclusions their generator enforces.

---

## 5. Rejected — and why

- **`coerceCallArgsAt` identity-recovery gate + T9000-space α-rename leak
  guard (their headline SEAL fixes).** REJECT. These are artifacts of *late*
  (emit-time) type resolution over an untyped AST + `CallInstance`
  monomorphization record. We lower to a **typed IR with concrete `IrType`** at
  lower time; polymorphic instantiation is already resolved before we emit, so
  the `undefined: T9001` leak class **cannot occur** in our pipeline. Porting
  the gate would import a substitution/α-rename machinery to defend against a
  bug our architecture dissolves — a net loss on soundness *and* readability.
  Our design is strictly better here.

- **Wholesale runtime `cp -r`.** REJECT (see §2). Would silently revert our two
  auth hardening wins and import upstream's wrong feature-gating (system,
  jwt). Cherry-pick only.

- **Adding new clippy lints to catch #87.** REJECT. It's an emit-time
  validation gap, not a lintable pattern — upstream runs identical lints and
  still has the hole. The fix is the derive gate + exhaustive kernel-scheme
  table (#45), not linting.

- **Treating #76 / #84 as runtime completeness gaps.** REJECT. The runtime
  `Attribute`/`Element`/`Html`/`Color`/`Length` layer is byte-faithful and
  feature-complete. Both are **codegen/kernel wiring** issues; fixing them in
  the runtime would be misdirected effort.

- **Their untyped stringify / silent auth defaults.** REJECT (keep ours). Our
  typed SHOW obligation and fail-closed auth (negative-TTL rejection,
  id-decode) are more sound. A working upstream does not license a soundness
  regression.

---

### Principle-order scorecard (why the top actions rank as they do)

1. **Security** → FFI `Debug` redaction + CSS escape audit (action 2); retain
   auth fail-closed wins (#28/#29).
2. **Correctness** → per-route determinism gate (never false-DIFFER); #84 tag
   mapping; #86 kernel-call emission.
3. **Soundness** → #87 derive gate + `is_derivable` IR field (action 1);
   three-gate HOF capture (action 3); REJECT of the late-resolution machinery.
4. **Efficiency** → our early-lowering may avoid Arc-wraps their emit-time
   gates can't elide; keep it.
5. **Completeness** → #76 Batch 0 makes unbacked members unrepresentable;
   normalizers keep all 37 examples reachable.
6. **Readability** → early-lowering is simpler though less discoverable;
   document the IR `is_derivable` contract so the property is visible.
