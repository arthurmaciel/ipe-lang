# Registry migration Phase E — the exit-0 SEAL

Status: implementation plan (doc-only; no code, no build). Read-only research complete.
Source of truth: `docs/architecture/kernel-registry-design.md` §Q5 "Phase E — seal" (line 212),
§Q6 "The exhaustiveness gate" (218–234), and the "Phase C re-review" adjustments (272–375).
Closes audit **F1** (the `Ty::Var(u32::MAX)` fail-open) permanently, and finishes **F3** (drift)
and **F8** (dispatch) that Phases B–D began.

---

## Context — where we are entering Phase E

Phases B / C / D-relocation / D-first-scheme / #58 / #69 are committed. Uuid (#54), Encoding
(#55), and the `Css.*` kernels (#47) land **before** this plan executes; this plan **assumes**
`stdlib_scheme` is `Some` for every reachable `StdlibKernel` (i.e. `ALL − KNOWN_UNBACKED`).
Verifying that assumption is Task 0 — if it is false, Phase E does not start.

Current committed shape (verified against HEAD 2026-07-03):

- `crates/sky_types/src/constrain.rs:1457` `constrain_var_kernel(id: Option<StdlibKernel>, module, name, span)`.
  Obligation pre-checks for `MathMin/MathMax` (:1477) and `key_obligation_for` Dict/Set (:1487)
  run **before** the scheme lookup — they must survive Phase E untouched.
- Dual lookup (:1500–1515): `registry = id.and_then(|k| self.stdlib_scheme(k))`,
  `legacy = self.legacy_kernel_ty(module, name)`, combined by
  `kernel_scheme_or_unsupported(registry, legacy, span)` (:1524) which emits **IPE-L0108**
  (`LowerError::Unsupported(Feature::Kernels)`) when BOTH are `None`.
- `legacy_kernel_ty` (:1544) is currently **total**: `Some(self.kernel_ty(module, name))`, so the
  fail-closed arm is dormant (covered directly by `both_miss_is_fail_closed`, :5929).
- `kernel_ty` (:2896) is the legacy string table; its tail is `_ => Ty::Var(u32::MAX)` at
  `constrain.rs:5108` — **the F1 hole**.
- `stdlib_scheme` (:1986) returns `Option<Ty>`; its tail is a catch-all `_ => return None`
  (~:2887), whose sole live consumers are `KNOWN_UNBACKED` (PubSub) plus any not-yet-migrated
  family. Task 0 asserts the not-yet-migrated set is empty.
- `RELOCATED` (:5326), `FIRST_SCHEMED` (:5622), `KNOWN_UNBACKED` (:5753 — `PubSubPublish`,
  `PubSubPublishNoEcho`). Tripwires: `stdlib_scheme_matches_legacy` (:5804),
  `first_schemed_were_holes` (:5867), `migrated_set_burndown` (:5897),
  `known_unbacked_never_schemed` (:5763).
- Canon node `VarKernel { id: Option<StdlibKernel>, module, name }` at
  `crates/sky_canon/src/ast.rs:137`; `VarHome::Kernel(Option<StdlibKernel>, Symbol, Symbol)` at
  `crates/sky_canon/src/env.rs:41`; `stdlib_index` at `env.rs:59`, built from `ALL` at `env.rs:1012+`.
- `canon_equals_registry` at `crates/sky_canon/src/lib.rs:1355`: forward = every `ALL` decl whose
  qualifier is registered appears in `qual_vars` + `stdlib_index`; G1 reverse (:1410) = every
  `qual_vars` entry that **carries `Some(id)`** matches `stdlib_index`. It is
  **propagation-wiring-only**: it does NOT assert that every non-excluded `qual_vars` name carries
  `Some(id)`. Excluded alias namespaces at :1435.
- `crates/sky_lower/src/lower.rs:3914` `lower_callee`: id=`Some` fast path (:3921) returns
  `Callee::Kernel(*sk)`; the ~377-arm legacy `&str` table (:3924+) is the **id=None** fallback,
  tail IPE-L0108 at :4444. `decl_equiv_legacy_match` (:5031) forces `id=None` to keep the legacy
  arm non-vacuous.
- `crates/sky_lower/src/lower.rs:2061` `ir_type_from_ty`: opaque builtin-name arms
  (`Value`/`Length`/`Color`/`Decoder`/`Element`/`Html`) are matched **before** the user-enum
  lookup; the comment (:2064) claims a canon "§3.2 gate rejects any user type/ctor that shadows a
  builtin name". **That gate does not exist in `sky_canon`** — `resolve.rs` has only a
  `seen_types` duplicate-type check, no builtin-name reservation. This is the #45 hazard.

---

## Goal / definition of done

After Phase E, **skyc cannot exit 0 on a program that references an un-typeable kernel**. Concretely:

1. `Ty::Var(u32::MAX)` appears **nowhere** in the source tree (enforced by a source-level test).
2. `stdlib_scheme` is a wildcard-free match: every `StdlibKernel` except the explicit
   `KNOWN_UNBACKED` arm returns a concrete `Some(Ty)`; adding a variant fails to compile until an
   arm exists. F1 is **unrepresentable**, not merely test-guarded.
3. `constrain_var_kernel` resolves purely through the registry `id`; the legacy `kernel_ty` table,
   `legacy_kernel_ty`, and the dual lookup are deleted.
4. `VarKernel` / `VarHome::Kernel` carry the `id` only; `module`/`name` are reconstructed from
   `decl(id)` where diagnostics need them. The ~377-arm lower `&str` table is deleted.
5. `canon_equals_registry` is upgraded to the **full subset gate**: every non-excluded `qual_vars`
   name carries `Some(id)` — no reachable kernel can sit on a `None` id.
6. Whole cabal/cargo suite + example sweep green; the exit-0-then-cargo-fail class is closed **by
   construction**.

Companion (independent, soundness-adjacent): #45 reserved-builtin-type-name gate — decision below.

---

## Global constraints

**PRINCIPLES order — apply in this priority on every task ruling:**
1. **Security** first — no new trust-boundary surface; the totality flip only *removes* a
   silent-accept path, it never widens one. Confirm no user program that previously compiled and
   ran correctly now mis-types (a false BLOCK is cheap; a silent accept is not).
2. **Correctness** second — every deleted legacy arm must be provably dead (its kernel now resolves
   via the registry with a byte-identical `Ty`); Go-parity goldens byte-identical at every commit.
3. **Soundness** third — the seal's whole point: after it, a well-typed program referencing any
   reachable kernel is *guaranteed* a concrete scheme, so no un-typed kernel reaches codegen.

**Two hard project rules (non-negotiable):**
- **R1 — Never `sky build` / `cargo build` from the repo root** (overwrites the compiler binary
  in `sky-out/`). Run `sky check` / `sky build` only inside an `examples/<dir>`; run crate builds
  as `cargo build -p <crate>` / `cargo test -p <crate>` from the workspace, never a root
  `sky build`.
- **R2 — Every long-running command is timeout-bounded and mem-guard is running.** Wrap cabal/cargo
  test and any sweep in `timeout`; tee output to a file once and re-read it; confirm
  `scripts/guards/mem-guard.sh` is alive before a full build. No unbounded waits, no orphan background loops.

**Test-first, each commit cargo-green:** every task lands its assertion (or a compile-time
exhaustiveness change) *before* the deletion it justifies, and the workspace + example sweep stay
green at every commit. The seal itself (Task 1c) is a compile-time flip; its safety net (Task 0) is
already green before it runs.

---

## Task 0 — Pre-flight: prove `stdlib_scheme` is total over the reachable set

**This gates the entire plan. If it fails, stop — Uuid/Encoding/Css did not fully land.**

Test-first, in `crates/sky_types/src/constrain.rs` tests module (alongside `migrated_set_burndown`
:5897):

- Add `stdlib_scheme_total_over_reachable`: iterate `StdlibKernel::ALL`; for every `k` **not** in
  `KNOWN_UNBACKED`, assert `builder.stdlib_scheme(k).is_some()`. This is the machine-checked form of
  the plan's assumption "stdlib_scheme is total over the reachable set".
- Add `known_unbacked_disjoint_from_qual_vars` (in `sky_canon` tests, next to
  `canon_equals_registry` :1355): for every `k` in `KNOWN_UNBACKED`, assert `decl(k).qualifier` is
  **absent** from `env.qual_vars`. This is the load-bearing fact that makes the PubSub `None` arm
  *unreachable* (canon never mints a `VarKernel` for it), so the totality flip stays green.

**Verify:** `timeout 600 cargo test -p sky_types stdlib_scheme_total_over_reachable` and
`timeout 600 cargo test -p sky_canon known_unbacked_disjoint_from_qual_vars` both pass.
If `stdlib_scheme_total_over_reachable` fails, it prints the offending variants — those families
must be schemed first (out of scope for this plan; file them and stop).

No production code changes in Task 0. It is a pure gate.

---

## Task 1 — The totality flip (delete the F1 fallback; make `stdlib_scheme` wildcard-free)

This is the **seal** and the **riskiest** task. Three sub-steps, each its own cargo-green commit.

### 1a — Narrow `stdlib_scheme`'s catch-all to an explicit `KNOWN_UNBACKED` arm

Replace the tail `_ => return None` (~`constrain.rs:2887`) with the explicit arm
`K::PubSubPublish | K::PubSubPublishNoEcho => return None,`. Now `stdlib_scheme` is **wildcard-free**:
the compiler forces every other `StdlibKernel` variant to have a concrete arm. Adding a future
variant fails to compile in `sky_types` until its scheme exists — F1 becomes structurally
unrepresentable at this line.

- Guarded by Task 0's `stdlib_scheme_total_over_reachable` (already green) + `known_unbacked_never_schemed` (:5763).
- **Verify:** `cargo build -p sky_types` compiles (proves exhaustiveness holds — no variant is
  un-schemed); `cargo test -p sky_types` green.

### 1b — Flip `legacy_kernel_ty` to `None` and prove the legacy path is dead

Change `legacy_kernel_ty` (:1544) from `Some(self.kernel_ty(module, name))` to return `None` for
un-typed kernels. Because `stdlib_scheme` is now total over reachable (Task 0) and the registry
branch runs first in `kernel_scheme_or_unsupported`, every reachable kernel is served by
`registry = Some(..)`; `legacy` is `None` and inert. The dormant IPE-L0108 arm
(`both_miss_is_fail_closed`, :5929) now becomes the *live* fail-closed path for any (unreachable)
miss.

- Keep `kernel_scheme_or_unsupported` (:1524) exactly as-is — its `registry.or(legacy)` already does
  the right thing; only `legacy` changes from always-`Some` to always-`None`.
- **Verify:** `cargo test -p sky_types` green (including `both_miss_is_fail_closed`); then run the
  example sweep gate early (Task 5's command) to confirm no previously-green example regresses — this
  is the moment a mis-migrated reachable kernel would surface as IPE-L0108. If green, the legacy table
  is provably dead.

### 1c — Delete the dead legacy table and the `Ty::Var(u32::MAX)` sentinel

Delete `kernel_ty` (:2896–5108) in full, including its `_ => Ty::Var(u32::MAX)` tail at
`constrain.rs:5108`. Delete `legacy_kernel_ty` (:1544). Collapse `constrain_var_kernel`'s dual lookup
(:1512–1514) to `let ty = self.stdlib_scheme(k)…` — see Task 2 for the exact final shape once
`module`/`name` are dropped. Update `stdlib_scheme_matches_legacy` (:5804): the byte-parity oracle it
compared against (`kernel_ty`) is gone. Replace its RELOCATED-vs-legacy assertion with a
**frozen-snapshot** oracle — either a pinned `expect`-serialized `Ty` per RELOCATED kernel captured
at the last green commit, or fold RELOCATED into `stdlib_scheme_total_over_reachable` and retire the
now-oracle-less parity check. (Recommendation: freeze a snapshot so the Go-parity guarantee survives
the loss of the legacy oracle; do not silently drop the check.)

Add the source-level test **`no_ty_var_max_sentinel`** (Q6 secondary test 3): assert
`Ty::Var(u32::MAX)` / `u32::MAX` appears nowhere in `crates/sky_types/src/constrain.rs` (grep the
file at test time). This makes reintroducing the banned sentinel a failing test.

- **Verify:** `cargo build -p sky_types` + `cargo test -p sky_types` green;
  `no_ty_var_max_sentinel` green.

---

## Task 2 — Drop `module`/`name` from the `VarKernel` node; delegate purely by `decl(id)`

Now that `id` is authoritative (Task 1) and the Phase-C `decl(k).(qualifier,name) == node.(module,name)`
equality is proven (by `stdlib_scheme_matches_legacy` historically + the subset gate in Task 3), the
raw pair is redundant on the semantic path.

Test-first / ordering: **Task 3 must land before this task's deletions** — dropping `module`/`name`
removes the only inputs to the lower `id=None` legacy `&str` table, so every reachable kernel must
already be proven to carry `Some(id)` (Task 3's subset gate). Do Task 3, then:

1. `crates/sky_canon/src/ast.rs:137` — `VarKernel { id: StdlibKernel }` (drop `Option`, drop
   `module`/`name`). `id` becomes non-optional: canon either resolves a `StdlibKernel` or emits the
   existing unknown-member error (the FFI `Rust.*` path, when it lands, uses a *separate* callee
   representation per the design — see Open Decision 2, so this drop does not foreclose it).
2. `crates/sky_canon/src/env.rs:41` — `VarHome::Kernel(StdlibKernel)`; propagate through
   `install_prelude_qualifiers` / `env.rs:192,1012+`. Any site that previously matched
   `VarHome::Kernel(Some(sk), m, f)` now binds `sk` and reconstructs `(qual,name)` via `sk.decl()`
   only where a diagnostic string is needed.
3. `crates/sky_types/src/constrain.rs:1457,1575` — `constrain_var_kernel(id: StdlibKernel, span)`;
   obligation pre-checks (:1477, :1487) key off `id` unchanged; final body is
   `let ty = self.stdlib_scheme(id).ok_or(IPE-L0108…)?; self.instantiate(&ty)`.
   The VarKernel arm (:1575) drops its `module`/`name` bindings.
4. `crates/sky_lower/src/lower.rs:3914` — `lower_callee` becomes `Callee::Kernel(*id)` directly; the
   ~377-arm legacy `&str` table (:3924–4444) is **deleted**, and `decl_equiv_legacy_match` (:5031) —
   which forces `id=None` to exercise that table — is deleted with it (its premise no longer exists).
   Any lower site needing the runtime name reads `decl(id).emit` / `decl(id).qualifier` (already the
   Phase-A `EmitRef` mechanism).

Retain a diagnostics breadcrumb by reconstruction, not storage: error/`sky doc` sites call
`id.decl()` on demand. This matches design OPEN DECISION 3.

- **Verify:** `cargo build` workspace + `cargo test` per touched crate (`sky_canon`, `sky_types`,
  `sky_lower`) green; example sweep (Task 5) green.

---

## Task 3 — Flip `canon_equals_registry` to the full QUALIFIERS-subset-registry gate

Currently propagation-wiring-only. Now that the fallback is gone, every reachable kernel MUST carry
`Some(id)` or it is un-typeable. Strengthen the reverse direction in
`crates/sky_canon/src/lib.rs:1355`:

Test-first — extend `canon_equals_registry`'s reverse loop (:1454): for every non-excluded
`(qual_sym, members)` in `env.qual_vars`, assert **every** `(name, home)` has
`home == VarHome::Kernel(Some(sk), …)` (post-Task-2: `VarHome::Kernel(sk)`) — i.e. **no** member sits
on `None`. The excluded-alias set (:1435) is retained verbatim; `KNOWN_UNBACKED` qualifiers are
naturally excluded because Task 0 proved they are absent from `qual_vars`.

This is the canon-side mirror of Task 1's types-side totality: together they prove "every reachable
`qual_vars` name → `Some(id)` → concrete scheme". It is the precondition that makes Task 2's
`module`/`name` drop and lower legacy-table deletion safe.

Do this **before** Task 2's node-field deletion (the assertion needs the `Option` still present to be
meaningful; after Task 2 the `Option` is gone and the property holds by the type itself, at which
point simplify the assertion to a structural walk that the node is well-formed).

- **Verify:** `cargo test -p sky_canon canon_equals_registry` green.

---

## Task 4 — #45 reserved-builtin-type-name gate — DECISION: land as a discrete companion commit, non-blocking

**Ruling: land in this plan, but as an independent commit that does NOT gate Tasks 1–3.**

Rationale (PRINCIPLES-ordered):
- It closes a **real silent-miscompile** (Security/Correctness): `ir_type_from_ty`
  (`lower.rs:2061`) matches opaque builtin names `Value`/`Length`/`Color`/`Decoder`/`Element`/`Html`
  **before** the user-enum lookup, and the "§3.2 gate" the comment (:2064) relies on **does not exist
  in `sky_canon`**. A user `type Color = …` (present in the corpus per memory #69) is silently
  overridden → wrong `IrType` → miscompile with no diagnostic. That is a parse-don't-validate hole:
  the reserved names must be rejected at the canon boundary, not trusted downstream.
- It is **structurally independent** of the kernel-registry totality: it touches type-decl
  resolution (`resolve.rs` `seen_types`), not `stdlib_scheme` / `VarKernel`. Bundling it into the
  one-line seal dilutes the seal and adds an unrelated regression surface (the corpus `type Color`).

Test-first, in `crates/sky_canon/src/resolve.rs` (alongside the `seen_types` duplicate check):

- Add a `RESERVED_BUILTIN_TYPES` const = the exact opaque-name set `ir_type_from_ty` matches ahead of
  the user-enum lookup — audit `lower.rs:2061` and `ir_type_from_ty_json` (:2048) to enumerate it
  (`Value`, `Length`, `Color`, `Decoder`, `Element`, `Html`, plus already-covered
  `Int/Float/Bool/String/Char/Bytes/Task/Maybe/Result/List/Dict/Set/Cmd/Sub` — the latter may already
  error via Prelude collision; only add names not already rejected).
- Reject any user `type`/`type alias`/ctor whose name is in `RESERVED_BUILTIN_TYPES` with a hard
  diagnostic naming the canonical builtin origin (mirror the Haskell audit §3.2 message shape).
- Regression test `user_type_shadowing_builtin_rejected`: `type Color = Red | Green` fails
  canonicalisation with the reserved-name diagnostic; a non-reserved `type Swatch = …` still compiles.
- Once the gate exists, the `lower.rs:2064` comment becomes true; update it to cite the real
  `sky_canon::RESERVED_BUILTIN_TYPES` gate instead of the phantom "§3.2".

**Sequencing:** may land before Task 1 or after Task 5 — it is orthogonal. Recommend landing it
first (it is a pure add + reject, lowest risk) so the opaque-name arms are trustworthy before the
seal ships. **Do not** let it block the seal if the corpus `type Color` needs a rename PR
coordinated separately.

- **Verify:** `cargo test -p sky_canon user_type_shadowing_builtin_rejected` green; grep the example
  corpus for `type Color`/`type Value`/etc. and confirm none newly break (if one does, it was the
  latent miscompile — file the rename).

---

## Task 5 — Whole-suite + example-sweep seal gate

The exit-0-then-cargo-fail class is now closed by construction; this task proves it holds end-to-end.

- `timeout 3600 cabal test` (or the workspace `cargo test` equivalent for the Rust crates) — zero
  failures; pending count unchanged.
- Example sweep via the plugin skill `sky-rust-backend:examples-sweep` (or `scripts/example-sweep.sh`
  with its `run_with_timeout 10` intact) — every example builds **and runs**; a skyc-exit-0 that used
  to fail at `cargo` can no longer occur because an un-typeable kernel now fails skyc at
  type-check with IPE-L0108. Confirm no example newly fails at skyc (that would be a real un-schemed
  reachable kernel — Task 0 should have caught it; if it appears here, Task 0's reachable set was
  incomplete).
- Go-parity goldens byte-identical (per memory: m4/m7 oracle goldens must not shift — the seal is
  type-side only, so emitted Go/Rust is unchanged).
- Confirm `no_ty_var_max_sentinel` (Task 1c), `stdlib_scheme_total_over_reachable` (Task 0),
  the strengthened `canon_equals_registry` (Task 3), and `user_type_shadowing_builtin_rejected`
  (Task 4) are all in the green suite.

R1/R2 reminders apply to every command here: sweep from `examples/<dir>`, never repo root; every
command `timeout`-bounded; mem-guard alive; clean up background loops before declaring done.

---

## Risk register

| # | Risk | Likelihood | Mitigation |
|---|---|---|---|
| **R-A (riskiest)** | Deleting `kernel_ty` (Task 1c) strands a *reachable* kernel that was silently riding the `Ty::Var(u32::MAX)` fallback (a family missed by Uuid/Encoding/Css), turning a previously-"green" example red at skyc. | Med | Task 0 `stdlib_scheme_total_over_reachable` is the machine gate; run it **and** the example sweep at Task 1b (before the 1c deletion) so the failure surfaces as an inert IPE-L0108 while the legacy table still exists as a rollback. |
| R-B | Dropping `module`/`name` (Task 2) removes info a future FFI consumer node reuses. | Low | FFI is parked and resolves `Rust.*` to a *separate* `KernelId::Ffi` callee per design Q4 — not this stdlib `VarKernel`. Recorded as Open Decision 2; reconstruct diagnostics from `decl(id)`. |
| R-C | `stdlib_scheme_matches_legacy` loses its oracle when `kernel_ty` is deleted (Task 1c), silently weakening the Go-parity guarantee for RELOCATED families. | Med | Freeze a per-kernel `Ty` snapshot oracle before deleting `kernel_ty`; never drop the parity check outright. |
| R-D | #45 gate (Task 4) rejects a legitimate corpus `type Color`, breaking an example. | Low-Med | Grep the corpus first; if hit, it is the latent miscompile — coordinate a rename PR; keep Task 4 non-blocking to the seal. |
| R-E | Narrowing `stdlib_scheme`'s wildcard (Task 1a) exposes a variant nobody schemed that was masked by `_ => None`. | Low | This is a *compile* error in `sky_types`, caught before any test — exactly the intended fail-closed behaviour; scheme the variant or classify it KNOWN_UNBACKED. |

---

## Open decisions

1. **`stdlib_scheme_matches_legacy` after `kernel_ty` deletion** — freeze a snapshot `Ty` oracle
   (recommended, preserves Go-parity proof) vs retire the check into
   `stdlib_scheme_total_over_reachable` (simpler, loses byte-parity guarantee). Decide at Task 1c.
2. **FFI forward-compat of the `VarKernel` node** — confirm (when FFI un-parks) that `Rust.*`
   resolves to a separate `Callee::Ffi` / `KernelId::Ffi` path and never needs `module`/`name` back
   on the stdlib `VarKernel`. The design (Q4) says yes; this plan drops the fields on that assumption.
3. **#45 sequencing** — land before the seal (recommended: lowest-risk pure-add, makes the
   opaque-name arms trustworthy) vs after Task 5. Non-blocking either way.
4. **`decl(id)` diagnostics breadcrumb** — reconstruct on demand (recommended, this plan) vs retain
   `module`/`name` purely for error text. Design OPEN DECISION 3; no soundness impact.
