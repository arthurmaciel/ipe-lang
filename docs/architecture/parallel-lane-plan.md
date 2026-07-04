# Parallel-lane plan — next round (after #106/#107 lands)

> **Status:** dispatch-ready. Written 2026-07-04 (read-only planning pass).
> **Scope:** the ONE next round of three concurrent lanes. Verified against
> HEAD `940dd15` + the current in-flight working tree (#106/#107).
> **Governing rule:** `docs/architecture/roadmap-and-velocity.md` §"parallel
> fan-out — DISJOINT FILE SETS ONLY". Two build lanes run concurrently *iff*
> their edited file sets do not intersect. This plan proves disjointness at the
> **crate** level (the strong guarantee), not just the file level.

---

## 0. Ground truth (measured, not assumed)

### The sweep frontier (from `~/.cache/sky/examples-sweep/*.skyc.log`)

| Error code | Meaning | Examples blocked | Owning crates/files |
|---|---|---|---|
| **SKY-L0108** | Ui/Html/Live kernel not wired | 09, 10, 17, 18, 22, 31, 34 (7) | **THE CLUSTER** — see below |
| **SKY-N0004** | unknown module / import | 00, 12, 19, 20, 28, 30, 32, 38 (8) | `skyc` + `sky_canon` (compiled-source path) OR cluster (if kernel-backed) |
| **SKY-N0005** | unknown member | 02, 16, 23, 24, 27, simple (6) | `sky_canon` resolve OR cluster |
| **SKY-P0001** | parse error | 26, 29, 37 (3) | `sky_parse` (fully disjoint crate) |
| L0102 / L0106 / N0001 / N0010 / T0001 / T0004 | assorted | 14, 06, 33, 25, 21, 15 | mixed |

### "THE CLUSTER" — the file set that only ONE lane may hold per round

Confirmed by the file footprints of the last two #76 batches (`017ebbc`,
`940dd15`) and the current in-flight #106/#107 working tree (`git status`):

```
crates/sky_types/src/constrain.rs        <- HM scheme table for each kernel
crates/sky_lower/src/lower.rs            <- callee resolution + type lowering
crates/sky_kernels/src/lib.rs            <- runtime kernel registry rows
crates/sky_backend_rust/src/emit_expr.rs <- call-site emission
crates/sky_backend_rust/src/emit_live.rs <- (Live only; #106/#107)
crates/sky_backend_rust/src/naming.rs    <- kernel Rust-name mapping
crates/sky_ir/src/pretty.rs              <- IR pretty-printer arms
crates/sky_ir/src/ir.rs, lib.rs          <- (#106/#107 in-flight only)
runtime/src/sky_runtime/html.rs          <- Html render support
crates/sky_canon/src/env.rs              <- (#76 batch1 only, 1 line; batch2 did NOT touch it)
```

Every SKY-L0108 fix and every **kernel-backed** module/member fix funnels
through this cluster. **#106/#107 (Live-typing) is IN FLIGHT here now.** After
it lands, the warm target `~/.cache/sky-rust-target` has this whole cluster
hot — that is what makes the next warm-lane continuation cheap.

### The compiled-source stdlib subsystem is LIVE (correction of a stale doc)

`docs/architecture/error-module-design.md` carries a 2026-07-03 "CORRECTION"
claiming the compiled-Sky-source path is infeasible ("nothing calls
`skyc::stdlib::source`; `build`/`build_project` never inject stdlib source").
**That correction is now STALE.** Commits `3a98f2c` (#98 spike, `Std.Palette`)
and `5fe3f7a` (#47, `Std.Css`) wired the subsystem end-to-end: `skyc/src/stdlib.rs`
embeds modules via `include_str!` + a `MODULES` array, and
`skyc/src/{lib,project,stdlib}.rs` + `sky_canon/src/{resolve,lib,env}.rs` inject
and resolve them. `crates/skyc/stdlib/{Sky/Core/*, Std/Palette.sky, Std/Css.sky}`
prove pure-Sky modules (ADT-defining, kernel-aliasing) compile today. **This is
the substrate for Lane B and refreshes the whole premise of #85.**

Its crate footprint — `skyc` + `sky_canon` — is **disjoint from the cluster**.

---

## 1. Lane A (WARM target `~/.cache/sky-rust-target`) — #76 Ui/Html remaining

**Task:** SKY-L0108 batch 3+ — back the remaining corpus-used `Std.Ui` element
kernels (`Ui.button`, `Ui.input`, `Ui.form`, layout primitives) + `Std.Ui.*`
attribute-module extensions, following the exact pattern of batches 1–2.

**Why warm-optimal:** #106/#107 leaves the entire Ui/Html/Live cluster compiled
in target-1. Continuing in the *same* cluster reuses that hot state — no cold
recompile of `sky_types`/`sky_lower`/`sky_backend_rust`. Any other Lane-A choice
would either (a) touch the cluster anyway (so still needs it warm — same footprint)
or (b) leave the cluster and waste the warmth. #76 is the highest-value warm
continuation: it chips directly at the 7 L0108 examples.

**File footprint (crates):** `sky_types`, `sky_lower`, `sky_kernels`,
`sky_backend_rust`, `sky_ir`, `runtime`.

```
crates/sky_types/src/constrain.rs
crates/sky_lower/src/lower.rs
crates/sky_kernels/src/lib.rs
crates/sky_backend_rust/src/emit_expr.rs
crates/sky_backend_rust/src/naming.rs
crates/sky_ir/src/pretty.rs
runtime/src/sky_runtime/html.rs
crates/skyc/tests/golden_m7_*.rs        (new golden files — additive, no conflict)
tests/golden/m7_*/Main.sky              (new)
```

**HARD CONSTRAINT for Lane A this round:** do **NOT** edit `sky_canon/*`
(batch 2 proved #76 does not need it — `sky_canon` is assigned to Lane B this
round; see §2). Register new Ui kernel names via the constrain scheme table +
kernel registry, not via a canon edit.

**Gate:** `timeout 3600 cargo` for build/clippy/test/miri on changed crates;
guardian-impl → mechcheck → adversarial-review per the mandate. Foreground,
bounded — no background jobs.

---

## 2. Lane B (ISOLATED worktree, COLD target-2) — #80 modules, compiled-source ONLY

**Task:** SKY-N0004 — resolve the unknown-module failures by adding the
**pure-Sky compiled-source** members of the #80 module set (`Std.ToString` and
any of `Cli` / others that are pure re-exports/aliases of *already-wired*
kernels or ADT-only definitions). Follow the `Std.Palette` (#98) / `Std.Css`
(#47) template exactly: a new `crates/skyc/stdlib/**/<Module>.sky` + an
`include_str!` const + a `MODULES` row in `crates/skyc/src/stdlib.rs`, plus the
`sky_canon` resolve/expose wiring the #98 spike established.

**THE HARD STOP (this is what preserves disjointness):** the moment a target
module needs a *new runtime kernel* — `Test.runMain` (test-harness kernel),
`Sky.Http.Server.Stream` (streaming runtime), `Sky.Cli` password/tty kernels —
Lane B **STOPS and files it** for a future Lane-A cluster round. Lane B may
**never** edit `sky_kernels`, `sky_types/constrain.rs`, `sky_lower/lower.rs`, or
`sky_backend_rust/*`. A pure-Sky module that only defines ADTs and calls
existing kernels (e.g. `ToString.fromInt = String.fromInt`, where
`String.fromInt` is already a wired kernel) is in scope; anything else is out.

**File footprint (crates):** `skyc`, `sky_canon`.

```
crates/skyc/stdlib/Std/ToString.sky      (new pure-Sky source)
crates/skyc/stdlib/**/<other pure>.sky   (new, as classification greenlights)
crates/skyc/src/stdlib.rs                (include_str! const + MODULES row)
crates/sky_canon/src/resolve.rs          (module resolution / expose)
crates/sky_canon/src/lib.rs              (if the #98 inject path needs a row)
crates/sky_canon/src/env.rs              (module-name registration region ONLY)
crates/skyc/tests/*.rs                   (new golden — additive)
examples/…                              (read-only; drive via the skyc binary)
```

### Disjointness proof (crate-level — the strong guarantee)

```
Lane A crates : { sky_types, sky_lower, sky_kernels, sky_backend_rust, sky_ir, runtime }
Lane B crates : { skyc, sky_canon }
Intersection  : ∅
```

- Lane A does **not** touch `skyc` or `sky_canon` (§1 hard constraint; batch-2
  evidence: `940dd15` edited neither).
- Lane B does **not** touch any cluster crate (§2 hard stop: pure-Sky only; the
  physical rule "may not edit `sky_kernels`/`constrain.rs`/`lower.rs`/backend"
  makes a collision *unrepresentable*, not merely avoided).
- The one historically-shared file, `sky_canon/src/env.rs` (1 line in #76
  batch1; also modified by in-flight #106/#107), is assigned **exclusively to
  Lane B** this round. Because #106/#107 lands *before* the round starts, that
  file is at a clean baseline; Lane A is forbidden to re-touch it.

Empty intersection ⇒ the two worktrees cannot produce a merge conflict by
construction.

### Worktree + cold target-2 + df-abort-guard setup (Lane B brief)

```bash
# 1. Isolate (branch off the post-#106/#107, post-#76-optional master tip)
git worktree add .claude/worktrees/agent-laneB HEAD
cd .claude/worktrees/agent-laneB
export CARGO_TARGET_DIR="$HOME/.cache/sky-rust-target-2"   # COLD, separate from Lane A

# 2. df-abort guard — MUST wrap every cargo invocation.
sky_build() {
  local free_g; free_g=$(df -BG --output=avail / | tail -1 | tr -dc '0-9')
  if [ "${free_g:-0}" -lt 8 ]; then
    echo "ABORT: <8G free (/ has ${free_g}G) — killing Lane B before ENOSPC"; return 3
  fi
  timeout 3600 cargo "$@"                                  # bounded foreground; NO background jobs
}
sky_build build -p skyc
```

**Disk math (verified):** `/` has ~31 G free; warm target-1 = 37 G (Lane A's).
A cold target-2 full workspace build is ~15–20 G → leaves ~11–16 G, above the
8 G floor but **tight**. The df-guard aborts Lane B (not Lane A) if free drops
below 8 G. Note: on an *empty* target-2 the FIRST build is a full workspace
compile regardless of how small the source change is — the "emitted-project-only
~2–3 G" path does **not** apply to any guardian-impl change (it would require
sharing Lane A's warm target, which concurrent cargo cannot do safely). Choose
Lane B as `skyc`-rooted precisely because *incremental* rebuilds after the first
are the cheapest available (leaf-ward): editing a `.sky` + `stdlib.rs` recompiles
only `skyc`; a `sky_canon` edit recompiles `sky_canon`→`sky_types`→`sky_lower`→
`sky_backend_rust`→`skyc` (source-disjoint from Lane A — they merely *rebuild*,
never conflict).

---

## 3. Doc Lane (READ-ONLY — no build, no code edits)

**Task:** produce **`docs/architecture/module-classification.md`** — the
kernel-vs-compiled-source classification for every module behind the SKY-N0004
(8) and SKY-N0005 (6) frontier examples, AND refresh the #85 Error design
against the now-LIVE compiled-source subsystem.

**Why this is the highest-value design-ahead:** it is the *prerequisite that
un-gates Lane B's scope decisions AND every future round.* Concretely it must
decide, per module (`Test`, `Cli`, `Stream`, `ToString`, `Log`, `Ui`,
`Http.Server`, …):

1. **Pure-Sky compiled-source** (ADT-only or aliases existing kernels) →
   routes to a Lane-B (`skyc`/`sky_canon`) round. *List them explicitly with
   the exact `.sky` shape.*
2. **Kernel-backed** (needs a new `sky_kernels` row + constrain scheme + lower
   arm + runtime mirror) → routes to a Lane-A cluster round. *List the kernels.*

Then **rewrite the stale correction in `error-module-design.md`**: with #98/#47
proving `build_project` injects stdlib source, `Sky.Core.Error`'s primary
"compiled Sky source module" plan is feasible again — re-evaluate the
~17-helper / 69-golden flip against the live subsystem and state the corrected
step order.

**Method (design mandate):** the double-swarm — one arm reasoning fresh from the
repo, one arm from the upstream `../sky` Go reference (NB: `../sky-learning` does
**not** exist on this host; substitute `../sky` + `docs/architecture/*`) →
conciliation. Read-only; no build; no code.

**Deliverables:** `docs/architecture/module-classification.md` (new) + an edit
to `docs/architecture/error-module-design.md` (correction refresh). *These are
the only files the Doc Lane writes — both docs, zero conflict with A or B.*

---

## 4. Integration order

Lane B lives in `.claude/worktrees/agent-laneB`; A and Doc live on the master
working copy. Because A∩B = ∅ at the crate level, cherry-pick order is
conflict-free:

1. **#106/#107 lands** on master (current in-flight work) — the round's
   sequential CORE step; freezes the cluster + `sky_ir` shape Lane A builds on.
2. **Lane A (#76)** lands on master — warm, fastest; guardian → mechcheck →
   review → gate → commit.
3. **Cherry-pick Lane B (#80)** commits onto master — disjoint crates ⇒ clean
   apply. `git worktree prune` + `rm -rf .claude/worktrees/agent-laneB` after.
4. **Full gate on the merged tree** (`timeout 3600 cargo` fmt/clippy/test/miri +
   the examples sweep) — the single serialized verification point.
5. **Doc Lane** merges anytime (docs-only; never conflicts).

Steps 2 and 3 are order-independent (crate-disjoint); do A first only because
the warm target makes it finish sooner. **Conflict risk: effectively zero** —
the sole shared-file surface (`sky_canon/src/env.rs`) is single-owned by Lane B.

---

## 5. 2nd / 3rd round sketch (keep 3 lanes saturated)

| Lane | Round-1 (this plan) | Round-2 rotation | Round-3 rotation |
|---|---|---|---|
| **A (warm cluster)** | #76 Ui/Html L0108 | **kernel-backed #80** deferred by Lane B (`Test.runMain` harness kernel, `Stream` runtime) — needs the cluster, so only A may do it | SKY-N0005 kernel-member fixes + L0102/L0106 (14, 06) — cluster-resident |
| **B (cold, non-cluster crates)** | #80 pure-Sky compiled-source | **P0001 parser** (`sky_parse`, fully disjoint crate): `-cents` prefix-neg desugared to the existing `negate` kernel at parse (clears ex 37 parse); empty-record `{}` parse-accept (ex 26/29) — see §6 caveat | **#85 Error** as compiled-source (`skyc`/`sky_canon` — same non-cluster lane crates, target-2 warm by now) |
| **Doc (read-only)** | module-classification + #85 refresh | **#51 Go-equiv harness** design (activate the EQUIV column; `scripts/`+`tools/oracle`) | **#59 rename** plan (Sky→Ipê, pre-push, plan-only, huge) |

Rotation invariant: **exactly one lane holds the cluster at a time** (always
Lane A). Lane B stays permanently in the non-cluster crate belt
`{skyc, sky_canon, sky_parse}`. Doc Lane never builds. This keeps all three
saturated without ever letting two lanes into `constrain.rs`/`lower.rs`/
`sky_kernels`.

---

## 6. Pairs considered and REJECTED (footprint overlap)

- **#104 (backend arg-then-reuse move) or #99 (match-arm alias) as Lane B while
  A does #76** — **REJECTED.** Both edit
  `crates/sky_backend_rust/src/emit_expr.rs`, which #76 batches 1 *and* 2 both
  edited (`017ebbc`, `940dd15`). Direct same-file collision. #104/#99 can only
  run in the SAME lane as the Ui/Html work (serialized behind it), never
  parallel to it.
- **#80 FULL (incl. `Test`/`Stream`/`Cli`-tty) as Lane B** — **REJECTED.**
  `Test.runMain` + `Server.Stream` need *new* runtime kernels → `sky_kernels` +
  `constrain.rs` + `lower.rs` + backend = the cluster Lane A owns. Only the
  pure-Sky subset (§2 hard stop) is parallel-safe.
- **#94/#95 (Msg/lambda-view seal → `emit_model_gate.rs`) as Lane B** —
  **REJECTED (soft).** `emit_model_gate.rs` is file-distinct from #76's
  `emit_expr.rs`/`naming.rs`, but it lives in `sky_backend_rust` (a cluster
  crate) and the seal typically reads `sky_lower`/`sky_ir` shapes → high risk of
  needing a `lower.rs` edit mid-flight, which would collide. Kept out of the
  parallel slot; fold into a Lane-A cluster round instead.
- **SKY-N0005 members as Lane B, if any fix needs a kernel member** —
  **REJECTED conditionally.** Canon-resolution-only member fixes are safe
  (`sky_canon`), but a missing-kernel member drags in `sky_kernels`/`constrain`.
  Deferred to the classification (Doc Lane) before assignment.
- **P0001 empty-record `{}` as a *full-build* Lane B this round** —
  **REJECTED for round-1, deferred to round-2.** The parser can *accept* `{}`
  in `sky_parse` alone, but an empty `TRecord([])`/`Record([])` has **no**
  `constrain.rs`/`lower.rs` support today (verified: no `fields.is_empty()` /
  empty-record arm). Closing it end-to-end ripples into the cluster (Lane A).
  Round-2 Lane B may land the *parse acceptance + a golden parse test* and file
  the constrain/lower follow-up for a Lane-A round — keeping Lane B in
  `sky_parse` by construction. (`-cents`, by contrast, desugars to the existing
  `negate` kernel entirely within `sky_parse` — fully self-contained, no
  ripple.)

---

## 7. One-screen dispatch summary

- **Lane A — WARM target-1:** #76 Ui/Html L0108 remaining. Crates
  `{sky_types, sky_lower, sky_kernels, sky_backend_rust, sky_ir, runtime}`.
  **Must not touch `sky_canon`.**
- **Lane B — COLD target-2, worktree `agent-laneB`, df-guard <8G abort:** #80
  pure-Sky compiled-source modules (ToString + greenlit pure). Crates
  `{skyc, sky_canon}`. **Hard stop → file if a kernel is needed.**
- **Doc Lane — read-only:** `module-classification.md` + #85 Error refresh.
  Writes docs only.
- **Disjointness:** Lane-A crates ∩ Lane-B crates = ∅ (proved §2).
- **Merge:** #106/#107 → A → cherry-pick B → full gate → Docs.
