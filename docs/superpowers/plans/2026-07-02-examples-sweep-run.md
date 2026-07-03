# Plan — Run the examples-sweep baseline on skyc (#35)

## Goal

Produce the **honest first red/green baseline** of the ported examples-sweep
harness against `skyc` at HEAD, and convert its RED rows into a triaged gap list
that feeds #37 (CI wiring) and #51 (equiv harness). skyc pre-parity → *most rows
RED is the expected, correct outcome* — the baseline **is** the gap map, not a
failure. This plan does **not** modify the harness (it was ported and verified in
`docs/architecture/examples-sweep-port.md`); it *drives* it, captures artifacts,
and files the deltas.

Concretely, the deliverable is:

1. A reproducible baseline command (matching CI's `examples-sweep.yml` env) run
   under a 60-min ceiling.
2. Three captured artifacts: the aligned `TABLE`, the machine-readable
   `rows-$STAMP.tsv`, and the appended `scoreboard.tsv` line.
3. A per-RED-row triage: each RED classified by `(BUILD|RUN)` cell × skyc/cargo
   error code, cross-linked to an existing pending task or a newly-filed gap.

## Architecture

The harness is three sourced bash libs + one driver, plus two backend-agnostic
Python normalizers (dormant in phase 1). Nothing here is Rust source — the plan
is **scripts/docs/run-capture only**, so it has **zero file overlap** with the
two in-flight source efforts (see Global Constraints → Parallel safety).

```
scripts/examples-sweep.sh         driver: one row per example, cols BUILD·RUN·EQUIV(+NOTE)
scripts/lib/env.sh                REPO + SKYC_BIN + CARGO_TARGET_DIR + sccache   (source-only)
scripts/lib/examples.sh           build_set / example_shape / is_out_of_scope    (source-only)
scripts/lib/checks.sh             exercise_{cli,server,live,tui,webview} + resolve_bin
scripts/equiv-classification.tsv  equiv-mode overrides (phase-2; dormant now)
```

Phase-1 flag posture (this baseline): `SKY_SWEEP_NO_EQUIV=1` (BUILD+RUN, EQUIV
column renders `—`), `SKY_SWEEP_FORCE=1` (bypass the opt-in night gate). The Go
reference (`build_go`, `equiv_for`) stays dormant — this repo ships no Haskell
`sky`, so EQUIV is deliberately off until #51.

### Data-flow (verified against HEAD)

`build_set` (`examples.sh:141`) = `all_examples` (`examples.sh:33`) minus Go-FFI
(`is_out_of_scope`, `examples.sh:83`). In this repo the 33 vendored dirs are
*exactly* the in-scope set (the 9 Go-FFI dirs were never vendored), so a run
yields **~33 rows**. For each dir the driver (`examples-sweep.sh:309`):

1. `rm -rf sky-out .skycache .skydeps` (L315), classify `shape`/`mode`.
2. `build_rust` (L110): `skyc build <sky.toml|src/Main.sky> --out sky-out/rust`
   (L116) then `cargo build --manifest-path sky-out/rust/Cargo.toml` (L132).
   → `BUILD_CELL ∈ {ok, skyc-fail, cargo-fail}`.
3. `resolve_bin` (`checks.sh:115`) → the emitted `sky-app`.
4. `run_for` (L239) drives the binary per shape → `RUN ∈ {ok, panic, hang,
   noserve, notty, skip}`.
5. Row appended to `rows-$STAMP.tsv`; verdict tallied (L358).

## Tech Stack

- bash (harness), `skyc` (Rust, `cargo build --release -p skyc`), `cargo`,
  `sccache` (RUSTC_WRAPPER), `rg` (mandatory — the scope filter), `curl` +
  `python3` (RUN phase: server probe + `free_port`). `go` **not** required
  (phase-1 `NO_EQUIV=1`). All present on this host (verified).
- Shared `CARGO_TARGET_DIR=~/.cache/sky-rust-target` (this repo's
  `~/.cargo/config.toml` global `target-dir`; `env.sh:29` agrees with it).

## Global Constraints

**Principle order (non-negotiable):** security > correctness > soundness >
efficiency > completeness > readability. A tie breaks toward the earlier
principle.

**Two fundamental rules:**
- **PARSE, DON'T VALIDATE.** Read the *typed* artifact (`rows-$STAMP.tsv`, 5 tab
  columns) into the triage, not a re-scrape of console text. The `.tsv` is the
  parsed source of truth; the aligned `TABLE` is a human view of it.
- **MAKE INVALID STATES UNREPRESENTABLE.** A baseline row is exactly one of
  `{GREEN, RED, SKIP, AMBER}` as computed by the verdict block
  (`examples-sweep.sh:360-378`). Triage keys off those tokens — never re-invent a
  parallel classification that could disagree with the harness verdict.

**Fail-closed, never panic/wildcard.** Every step asserts its precondition and
exits with a *diagnostic* on failure (missing fresh binary, low disk, empty row
file). No step swallows an error into a green claim. The harness itself already
fails closed (`free_port` FAIL-CLOSED `checks.sh:72`; disk gate
`examples-sweep.sh:60`; `rg`-required gate L83) — the plan preserves that.

**Baseline honesty (the crux).** RED is *expected*. The plan must NOT tune,
skip, or `.tsv`-override any row to manufacture green. The only allowed
green-shifting action is *fixing skyc* — which is out of scope here; those fixes
are the tasks the triage *files*.

**PUBLIC-artifact rule.** `../sky` is a parity/capability REFERENCE only. No
disparagement, no contribute-upstream note anywhere in captured artifacts.

**Parallel safety / file overlap.** This plan touches only `scripts/**`,
`docs/**`, and `~/.cache/**` (out-of-tree). The in-flight registry migration
edits `crates/sky_kernels/src/lib.rs` + `crates/sky_types/src/constrain.rs`;
#49 TCO edits `crates/sky_ir/**` + `lower.rs`/`emit_expr.rs`. **No source-file
overlap.** The *only* shared resource is `CARGO_TARGET_DIR` — a concurrent
`cargo build` of the workspace and this sweep both write there. cargo takes a
**target-dir file lock**, so concurrency is *safe-but-serialized* (the second
`cargo` blocks, it does not corrupt). Consequence for scheduling: run the
baseline when no other workspace `cargo build` is mid-flight, else the sweep's
first `cargo build` blocks until the lock frees — correctness is unaffected,
wall-time is not. Do **not** point a second sweep at the same `CARGO_TARGET_DIR`
concurrently (the emitted `sky-app` is overwritten per example by design,
`checks.sh:112-114`).

**Timeout discipline (CLAUDE.md rule 3).** Per-example skyc build ceiling is
`SKY_SWEEP_BUILD_TIMEOUT=900` (env, `examples-sweep.sh:111`); cargo build ceiling
900s hardcoded (L132); RUN ceilings are `exercise_cli` 25s, `exercise_server`
~15s, `exercise_tui`/`exercise_webview` 8s. The **whole sweep** is the cabal-60-min
analogue: wrap the driver in `timeout 3600`. A sweep that would exceed 60 min is
a stuck child — bisect via `RUST_EXAMPLES=`, never widen the ceiling.

---

## Task 1 — Fresh skyc + preflight (the stale-binary trap)

**Why first / the resolved ambiguity.** `env.sh:73-85` probes
`$CARGO_TARGET_DIR/release/skyc` **before** debug and before `$REPO/target`. On
this host the release binary is **stale** (built Jul 1 22:46, *older* than the
registry-migration commits at HEAD, e.g. `691e275`). A naive `bash
scripts/examples-sweep.sh` would therefore sweep with a **stale compiler** — a
dishonest baseline. CI sidesteps this by *always* rebuilding
(`examples-sweep.yml:126` `cargo build --release -p skyc`). The local baseline
MUST do the same so `SKYC_BIN` resolves to a HEAD binary.

**Files:** none edited. Commands only.

### Interfaces

- **Consumes:** `crates/skyc` workspace member; `runtime/Cargo.toml` package
  `sky-runtime-rust` (verified `runtime/Cargo.toml:2`).
- **Produces:**
  - `~/.cache/sky-rust-target/release/skyc` — freshly built, mtime ≥ HEAD commit.
  - Env probe confirming `SKYC_BIN` (what `env.sh` will pick) points at it.
  - `resolve_runtime()` resolvable to `$REPO/runtime/src/sky_runtime` (`lib.rs:390`).

### Failing check first

```bash
cd /home/arthur/Documentos/comp/sky-rust
# Assert the binary env.sh WOULD pick is at least as new as HEAD's commit time.
HEAD_EPOCH=$(git -C . show -s --format=%ct HEAD)
BIN=~/.cache/sky-rust-target/release/skyc
if [ ! -x "$BIN" ] || [ "$(stat -c %Y "$BIN")" -lt "$HEAD_EPOCH" ]; then
  echo "STALE-OR-MISSING: release skyc older than HEAD ($BIN) — rebuild required"
else echo "FRESH"; fi
```

Expected **before** the build: `STALE-OR-MISSING: release skyc older than HEAD …`
(this is the red-first assertion — it proves the trap is real).

### Make it pass

```bash
cd /home/arthur/Documentos/comp/sky-rust
timeout 1800 cargo build --release -p skyc > /tmp/sweep-t1-skyc.log 2>&1
echo "skyc build rc=$?"
timeout 1800 cargo build -p sky-runtime-rust > /tmp/sweep-t1-rt.log 2>&1 \
  || cargo build --workspace > /tmp/sweep-t1-ws.log 2>&1 || true   # prewarm dep tree
```

Then re-run the check → expect `FRESH`. Sanity the CLI contract (must print
usage, not hang) — `skyc` has no `--version`; use the usage path:

```bash
~/.cache/sky-rust-target/release/skyc 2>&1 | head -1
# expected first line: "usage:"   (USAGE, lib.rs:423)
```

Preflight the run env exactly as the driver will (`examples-sweep.sh:52-83`):

```bash
cd /home/arthur/Documentos/comp/sky-rust
source scripts/lib/env.sh          # sets REPO + SKYC_BIN
echo "SKYC_BIN=$SKYC_BIN"          # MUST be …/release/skyc, and FRESH
[ -x "$SKYC_BIN" ] && echo "skyc OK" || { echo "FAIL: no skyc"; }
df -Pk "$REPO" | awk 'NR==2{ if ($4 < 5242880) print "FAIL: <5G disk"; else printf "disk OK (%.1fG)\n",$4/1024/1024 }'
for t in rg curl python3; do command -v "$t" >/dev/null || echo "FAIL: missing $t"; done
```

Expected: `SKYC_BIN=…/release/skyc`, `skyc OK`, `disk OK (40.0G)`, no `FAIL:`
lines. (Host verified: 40.0G free; rg/curl/python3/cargo/sccache all present.)

**Fail-closed:** any `FAIL:` line here aborts the plan before Task 2 — do not run
the sweep against a stale/absent binary or a <5G disk (builds corrupt under
ENOSPC, CLAUDE.md rule 6).

---

## Task 2 — Single-example harness self-check (prove the plumbing)

**Why.** Before a 33-example, minutes-long run, prove the driver produces a
well-formed row on one cheap example. This is the "failing-test-first" for the
*harness invocation itself*: assert the table has a header + exactly one data
row with 5 columns and a recognized BUILD token. `01-hello-world` is the natural
probe (smallest CLI; if skyc handles `Sky.Core.*` at all, this is the most
likely GREEN).

**Files:** none edited.

### Interfaces

- **Consumes:** `RUST_EXAMPLES` subset override (`examples-sweep.sh:295-298`,
  accepts basenames or paths); `SKY_SWEEP_NO_EQUIV`, `SKY_SWEEP_FORCE` (L72-74,
  `checks.sh:57`).
- **Produces:** `$HOME/.cache/sky/examples-sweep/sweep-$STAMP.table` (aligned) +
  `rows-$STAMP.tsv` (5-col: `EXAMPLE\tBUILD\tRUN\tEQUIV\tNOTE`, L343) +
  `run-$STAMP.log`. The driver prints the `TABLE` path on its last lines (L398).

### Failing-test-first

```bash
cd /home/arthur/Documentos/comp/sky-rust
SKY_SWEEP_NO_EQUIV=1 SKY_SWEEP_FORCE=1 RUST_EXAMPLES="01-hello-world" \
  timeout 900 bash scripts/examples-sweep.sh > /tmp/sweep-t2.log 2>&1
echo "driver rc=$?"     # 0 (no red) or 1 (a red row) — BOTH are valid harness states
```

Assert the artifact shape (parse the `.tsv`, don't scrape the console):

```bash
TSV=$(ls -t ~/.cache/sky/examples-sweep/rows-*.tsv | head -1)
echo "rows file: $TSV"
awk -F'\t' 'END{print "cols="NF" rows="NR}' "$TSV"    # expect cols=5 rows=1
cut -f1,2,3 "$TSV"                                     # e.g. "01-hello-world  ok  ok" OR "… skyc-fail …"
```

Expected: `cols=5 rows=1`; BUILD ∈ {ok, skyc-fail, cargo-fail}; RUN ∈ {ok, panic,
hang, skip, —}. **Either a GREEN or a RED single row passes this task** — the
check is *well-formedness*, not color. A `cols≠5` or `rows=0` is a harness/plumbing
failure and blocks Task 3.

**Fail-closed diagnostic:** if `rows=0`, inspect `run-$STAMP.log` for the gate
that tripped (`skyc binary not at …`, `<5G free disk`, `rg required`,
`night_guard deferred`). Those are the only early-exit-2 paths
(`examples-sweep.sh:52-83`).

---

## Task 3 — Full baseline sweep (bounded, captured)

**Files:** none edited. This is the deliverable run.

### Interfaces

- **Consumes:** `build_set` (33 in-scope dirs, `examples.sh:141`); the phase-1
  env from CI (`examples-sweep.yml:66-72`).
- **Produces (the baseline artifacts):**
  - `sweep-$STAMP.table` — the aligned 5-column table (`examples-sweep.sh:349`).
  - `rows-$STAMP.tsv` — machine-readable, the triage input (Task 4).
  - `warnings-$STAMP.tsv` — `EXAMPLE\tWARN_COUNT` (cargo warnings past the
    generated `#![allow]`, L325).
  - `scoreboard.tsv` — one appended line
    `$STAMP\tgreen=N\tred=N\tskip=N\tamber=N\t<eq-breakdown>` (L401).
  - Driver exit code: `0` = no RED, `1` = ≥1 RED (or a leaked cargo warning),
    `2` = setup/gate (L403-410).

### Run (60-min analogue ceiling)

```bash
cd /home/arthur/Documentos/comp/sky-rust
SKY_SWEEP_NO_EQUIV=1 SKY_SWEEP_FORCE=1 SKY_SWEEP_BUILD_TIMEOUT=900 \
  timeout 3600 bash scripts/examples-sweep.sh > /tmp/sweep-baseline.log 2>&1
echo "sweep rc=$?"
```

Expected rc: **1** (pre-parity → RED rows present → VERDICT FAIL) is the *honest,
correct* baseline. rc `0` would mean skyc already at example parity (surprising
this early — verify it is not a filter bug hiding rows). rc `124` = the sweep hit
the 60-min ceiling → a stuck child; bisect with `RUST_EXAMPLES` (do NOT widen
3600). rc `2` = a setup gate tripped → fix per Task 1.

### Capture the three artifacts by `$STAMP` (not by scanning the stale dir)

The HIST dir carries stale Jun-17 logs from an earlier era; **key every capture
off the newest `$STAMP`**, never the whole dir:

```bash
HIST=~/.cache/sky/examples-sweep
TABLE=$(ls -t "$HIST"/sweep-*.table | head -1)
TSV=$(ls -t "$HIST"/rows-*.tsv | head -1)
echo "== TABLE ==";  cat "$TABLE"
echo "== SCORE ==";  tail -1 "$HIST/scoreboard.tsv"
awk -F'\t' 'END{print "total rows="NR}' "$TSV"   # expect ~33
```

Expected `total rows` ≈ 33 (00,01,02,04,06,09,10,12,14,15,16,17,18,19,20,21,22,
23,24,25,26,27,28,29,30,31,32,33,34,37,38,simple,test_pkg). A materially smaller
count means `is_out_of_scope` over-filtered (a vendored example with an
unresolvable import) — investigate before trusting the baseline.

**Known-expected reds (documented, not harness bugs):**
- `26-ui-showcase` → `skyc-fail` (SKY-N0020): no `sky.toml`, multi-module, so the
  driver single-file-builds `src/Main.sky` which cannot resolve its local
  `RegressionGates` import (`examples-sweep.sh:95-105`, port-doc TODO(verify)).
- Any example importing `Std.Ui`/`Std.Live`/`Std.Db`/server/tui/webview →
  `skyc-fail`/`cargo-fail` until skyc reaches parity (port-doc "Gating posture").

---

## Task 4 — Triage RED rows into the roadmap (the gap list)

**Why.** The baseline's value is the **gap map**. Each RED row is classified by
its failing cell and root cause, then cross-linked to an existing pending task or
filed as a new gap. This is the artifact #37 (CI: flip `continue-on-error` to
false once green) and #51 (equiv harness: turn EQUIV on) consume.

**Files:** none in-tree. Output is the triage table returned to the orchestrator
(and, if the orchestrator directs, a task per unlinked gap via `TaskCreate`).
Do **not** author a summary `.md` (subagent rule).

### Interfaces

- **Consumes:** `rows-$STAMP.tsv` (parsed, PARSE-DON'T-VALIDATE), and per-row the
  skyc/cargo logs `$HIST/<name>.skyc.log` / `<name>.cargo.log`
  (`examples-sweep.sh:116,132`).
- **Produces:** for each RED row a tuple
  `(example, shape, BUILD_cell, RUN_cell, root-cause-code, linked-task)`.

### Steps

1. **Enumerate REDs from the typed artifact** (same classification the harness
   used, `examples-sweep.sh:360-378`):

```bash
HIST=~/.cache/sky/examples-sweep
TSV=$(ls -t "$HIST"/rows-*.tsv | head -1)
awk -F'\t' '
  { b=$2; r=$3; e=$4; red=0
    if (b=="skyc-fail"||b=="cargo-fail") red=1
    if (r=="panic"||r=="hang"||r=="noserve"||r=="notty") red=1
    if (e=="DIFFER") red=1
    if (red) printf "%s\tBUILD=%s\tRUN=%s\t%s\n",$1,b,r,$5 }
' "$TSV"
```

2. **Root-cause each `skyc-fail`** by the first diagnostic code in its log
   (skyc emits `SKY-Nxxxx` codes; group by code so one fix closes many rows):

```bash
for n in $(awk -F'\t' '$2=="skyc-fail"{print $1}' "$TSV"); do
  code=$(rg -o 'SKY-[NE][0-9]{4}' "$HIST/$n.skyc.log" | head -1)
  printf '%s\t%s\n' "$n" "${code:-<no-code>}"
done | sort -k2
```

3. **Root-cause each `cargo-fail`** (skyc emitted, but the generated crate did
   not compile — a codegen/kernel-registry gap; groups map to #45/#46/#47):

```bash
for n in $(awk -F'\t' '$2=="cargo-fail"{print $1}' "$TSV"); do
  err=$(rg -o 'error\[E[0-9]{4}\]|cannot find|no method|unresolved import' "$HIST/$n.cargo.log" | head -1)
  printf '%s\t%s\n' "$n" "${err:-<see-log>}"
done
```

4. **Root-cause `RUN` reds** (BUILD ok but panic/hang/noserve/notty): grep the
   run log for `$PANIC_RE` (`checks.sh:29`) — a `panic` here is a **soundness**
   red (higher priority than a build gap per the principle order) and likely maps
   to #49 (missing TCO → user tail-recursion stack-overflow) or a runtime kernel
   defect.

5. **Cross-link.** Map each root-cause group to the existing pipeline where one
   fits, else flag as an unlinked gap for the orchestrator to file:
   - kernel-scheme exit-0-then-cargo-fail class → **#45**.
   - plain HTML attribute kernels (`class`/`id`/`href`/…) → **#46**.
   - `Std.Css` → **#47**. let-bound app-entry cfg CompilerBug → **#48**.
   - RUN `panic`/stack-overflow from tail recursion → **#49**.
   - `26-ui-showcase` sky.toml gap → note as port-doc TODO(verify) (not a skyc
     bug; a fixture gap).

**Fail-closed:** a RED row whose log carries *no* recognizable code/error string
is itself a finding (opaque failure) — surface it explicitly, never bucket it as
"misc/ignore".

---

## Task 5 — Record posture + handoff to #37 / #51

**Files:** none edited (posture is already correct in-repo; this task *verifies*
and reports, it does not change CI).

### Verify the current CI posture matches the baseline reality

```bash
cd /home/arthur/Documentos/comp/sky-rust
rg -n 'continue-on-error|SKY_SWEEP_NO_EQUIV|SKY_SWEEP_FORCE|cargo build --release -p skyc' \
   .github/workflows/examples-sweep.yml
```

Expected: `continue-on-error: true` (L47), `SKY_SWEEP_NO_EQUIV: '1'` (L66),
`SKY_SWEEP_FORCE: '1'` (L68), `cargo build --release -p skyc` (L127) — i.e. CI is
already informational-only and phase-1. **No edit needed now.** The handoff note
to #37: *flip `continue-on-error` to false only once the baseline reaches
all-green* (port-doc "Gating posture", L129-137). Handoff to #51: the baseline's
`equiv` column is `—` today; turning EQUIV on is the phase-2 wiring change
(build_go / normalizers already ported intact).

### Return to orchestrator

The final message returns: (a) the three artifact paths, (b) the scoreboard line
(`green/red/skip/amber`), (c) the Task-4 triage tuples grouped by root-cause with
task cross-links, (d) any unlinked gap needing a new task.

---

## Definition of done

- [ ] Fresh `release/skyc` at HEAD; `SKYC_BIN` resolves to it (Task 1).
- [ ] Single-example self-check yields a well-formed 5-col row (Task 2).
- [ ] Full sweep ran under `timeout 3600`; `TABLE` + `rows-$STAMP.tsv` +
      `scoreboard.tsv` captured; row-count ≈ 33 (Task 3).
- [ ] Every RED row triaged `(cell × root-cause × linked-task)`; opaque failures
      surfaced explicitly (Task 4).
- [ ] CI posture verified phase-1/informational; handoff notes to #37 and #51
      recorded (Task 5).
- [ ] No harness edits, no manufactured green, no `../sky` disparagement.
- [ ] Background children reaped (`reap`, `checks.sh:75`); no orphan `sky-app`.
