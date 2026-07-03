# CI port (ubuntu + macOS + windows) + push to the public repo — implementation plan

> Guardian planner output for task **#37** ("Fix CI — port `../sky`'s
> `examples-sweep.yml` + `ci.yml` to ipê, ubuntu+macOS+windows per
> `windows-ci-support.md`, wire the mechcheck/sweep, push to
> `arthurmaciel/ipe-lang`"). Follows the source specs
> `docs/architecture/sweep-and-parity-plan.md` (A4/A5 sequencing +
> informational-vs-gating) and `docs/architecture/windows-ci-support.md`
> (the authoritative Windows change-set — followed, **not** redesigned).
> `../sky` is a **parity/capability reference** only: this plan states what
> ipê's CI does differently and why; it does not disparage upstream and files
> no upstream contribution.

---

## Goal

Bring ipê's CI to the "port complete" state the task requires:

1. **`examples-sweep.yml`** already exists (ubuntu + macOS, phased, non-gating).
   Extend it to a **three-host** informational sweep (ubuntu + macOS + windows)
   per `windows-ci-support.md`, with the harness Windows-hardened so Windows adds
   **no new spurious-RED class** and **no silent-green**.
2. **`ci.yml`** already exists as the Rust-native mirror of
   `plugins/sky-compiler/scripts/mechcheck.sh` (fmt / clippy / test+doctest /
   miri / sharded e2e, nightly, `cancel-in-progress`). **Verify** it mirrors
   mechcheck (a drift test), add the one missing lane the Windows spec calls for
   (a `windows-latest` unit lane exercising `element_to_cells`), and leave the
   rest byte-stable.
3. **Push** the result to the public repo, fail-closed on the `ipe` vs `ipe-lang`
   remote ambiguity, honouring the **cancel-in-progress-CI-on-main** discipline
   (never cancel tag/release runs) and the branch-first rule.

The sweep stays **informational** (`continue-on-error: true` via a per-OS
`experimental` flag). No gate is flipped in this plan — flipping is a later,
separately-tracked event (see § Sequencing & the informational→gating decision).

## Architecture

Two GitHub Actions workflows, one bash harness, driven by one OS-aware library:

```
.github/workflows/
  ci.yml              # mechanical gate ≡ mechcheck.sh: fmt·clippy·test·miri·e2e (sharded)
  examples-sweep.yml  # informational 3-host sweep: BUILD·RUN·EQUIV(phased off)
scripts/
  examples-sweep.sh   # per-example driver: build_rust / exercise_* / verdict
  equiv-classification.tsv
  lib/
    env.sh            # SKYC_BIN/CARGO_TARGET_DIR/REPO resolution  (sourced 1st)
    examples.sh       # build_set + is_out_of_scope (rg Go-FFI filter)
    checks.sh         # SKY_HOST_OS + resolve_bin + exercise_{cli,server,live,tui,webview}
    equiv_normalize_html.py / equiv_tui_grid.py   # phase-2 normalizers
.gitattributes        # NEW — LF pinning (B7)
```

The Windows change-set is **entirely** in the harness + workflow + `.gitattributes`
— it does **not** touch any Rust source under `crates/` or `runtime/`. That
disjointness is what makes this plan parallel-safe against the in-flight
registry migration and #49 TCO (see § Parallel-safety).

## Tech stack

- **GitHub Actions** — hosted runners `ubuntu-latest` / `macos-latest` /
  `windows-latest`; `dtolnay/rust-toolchain@stable`; `actions/cache@v4`;
  `actions/upload-artifact@v4`; `actions/setup-python@v5`; `taiki-e/install-action@nextest`.
- **Git Bash** on Windows (`shell: bash`) — one behavioural contract across all
  three hosts (a native PowerShell reimplementation is rejected as a second SSOT
  per `windows-ci-support.md` Q1).
- **Local test/lint tools (verified present on this box):** `shellcheck`
  (`ShellCheck` present), `python3` (3.10.12), `rg` (13.0.0), `jq` (1.6),
  `gh` (2.95.0). **Absent:** `actionlint`, `yamllint`, `bats` — so YAML tests in
  this plan use `python3` structural assertions + `rg` token checks (no new tool
  fetch), and shell tests use `bash -n` + `shellcheck` + purpose-built bash unit
  harnesses under `scripts/tests/`.

## Global Constraints

**PRINCIPLES order (every decision is resolved in this priority):**
`security > correctness > soundness > efficiency > completeness > readability`.

**Two fundamental rules, restated as constraints on this work:**

- **PARSE, DON'T VALIDATE.** Resolve ambiguous inputs into a typed/normalized
  form **once**, at the boundary, then operate on the parsed value. Concretely
  here: resolve `SKY_PYTHON`, `SKY_HOST_OS`, the `.exe` suffix, the
  `CARGO_TARGET_DIR` path shape, and the push **remote** exactly once each, up
  front — never re-sniff them ad hoc downstream. Line-ending normalization is the
  parse-don't-validate move applied to the EQUIV diff input: canonicalize the
  terminator once, then compare.
- **MAKE INVALID STATES UNREPRESENTABLE.** The matrix carries a per-OS
  `experimental` flag so "a Windows-only flake blocks a reference-host-green PR"
  cannot be represented. A built-but-unlocatable binary is a **counted RED**
  (`binmiss`), never a SKIP and never a freshest-file guess — "we ran the wrong
  binary and called it green" is made unrepresentable. The push target is
  resolved to a concrete, reachable remote before any push; an unresolved target
  **stops**, it does not guess.

**Fail-closed, not fail-open.** Every missing-tool preflight is a hard `exit 2`
(python / timeout / rg). No wildcard `_ -> skip`. No `continue-on-error` masking a
*genuine* `skyc-fail` / `cargo-fail` / `panic` / `noserve` / `DIFFER` once a host
is gating — informational status is a *deliberate, per-OS, filed-to-flip* state,
not a silent catch-all.

**Reference framing.** Where a fix says "port from `../sky`", `../sky` is the
capability reference; the ported code is re-verified against ipê's harness
anchors (cited below) — upstream line numbers are not trusted blind.

---

## Sequencing & the informational→gating decision (read before Task 0)

Per `sweep-and-parity-plan.md` § "The DONE gate", gating flips are **staged and
later**, not part of this plan:

- **Now (this plan):** BUILD+RUN on 3 hosts, EQUIV phased off (`SKY_SWEEP_NO_EQUIV=1`),
  whole job `continue-on-error: ${{ matrix.experimental }}` with **all three
  hosts `experimental: true`**. The RED table *is* the compiler to-do list,
  regenerated every push. **No flip here.**
- **A4 flip (separate, tracked):** ubuntu+macOS `experimental → false` only after
  the *first all-green BUILD+RUN sweep* on those hosts. Windows stays
  `experimental: true` until it *independently* reaches one all-green Windows
  BUILD+RUN sweep (`windows-ci-support.md` D-B / Q6). Both flips are filed gates,
  never calendar timeboxes, never informational-forever.
- **A5 flip (separate, tracked):** vendor cached example oracles + wire the
  `equiv_for()` cached-compare branch + drop `SKY_SWEEP_NO_EQUIV` → EQUIV gates
  (`DIFFER` fails; `go-ref-broken` stays AMBER). Go≡Rust oracle stays
  **ubuntu-only** so CRLF never touches the gating path (`windows-ci-support.md` Q4).

**Dependency on #35.** Task #35 ("Port examples-sweep to skyc + run initial
test") owns *the harness existing and producing a first table*. That is already
true on disk (the scripts exist and are anchored below), but Task 0 of this plan
**re-confirms a local dry-run produces a scoreboard** before we touch the
workflow — so Windows changes ride a known-good harness, not a broken one.

**Parallel-safety with in-flight work.** This plan's file set —
`.github/workflows/*.yml`, `scripts/*.sh`, `scripts/lib/{env,checks}.sh`,
`scripts/lib/*.py`, `.gitattributes` — is **disjoint** from:

- the **registry migration** (`crates/*/constrain.rs`, `crates/sky_kernels/**`,
  `lower.rs` callee threading — commit `691e275` Phase B), and
- **#49 TCO** (`sky_ir` +2 variants, `lower.rs`, `emit_expr.rs`).

There is **zero file overlap**, so this work can land in parallel. The *coupling*
is behavioural, not textual, and it is why gating stays deferred: registry churn
changes skyc breadth (RED-set churn in the BUILD column) and TCO changes the RUN
column (user tail-recursion stack-overflow → `panic` today; `ok` after #49). **Do
not flip BUILD-gating while the registry migration is in flight; do not flip
RUN-gating until #49 TCO has merged.** Encode that as commit-message notes, not
code.

---

## Task 0 — Sequencing gate: confirm the harness runs green-neutral locally, and files are disjoint

**Goal.** Prove the ported harness produces a scoreboard on *this* host before
editing it, and record the disjointness with the two in-flight branches. No code
change; a verification checkpoint that de-risks every later task.

**Files consumed (read-only):** `scripts/examples-sweep.sh`,
`scripts/lib/{env,checks,examples}.sh`.

**Interfaces**

- Consumes: `bash scripts/examples-sweep.sh` with `SKY_SWEEP_BUILD_ONLY=1`
  `RUST_EXAMPLES="01-hello-world"` `SKY_SWEEP_FORCE=1`.
- Produces: a scoreboard file at `~/.cache/sky/examples-sweep/sweep-*.table`
  (existence is the assertion; RED content is fine — honest measurement).

**Steps**

1. Disk preflight (CLAUDE.md ENOSPC trap): `df -h /` — must show > 15 GB free;
   if not, `go clean -cache 2>/dev/null; rm -rf "$CARGO_TARGET_DIR"` before
   proceeding.
2. Build skyc once: `cargo build --release -p skyc` — expect `Finished
   \`release\` profile`.
3. Dry-run one thin `Sky.Core`-only example, build-only, forced:
   ```bash
   SKY_SWEEP_FORCE=1 SKY_SWEEP_BUILD_ONLY=1 RUST_EXAMPLES="01-hello-world" \
     bash scripts/examples-sweep.sh > /tmp/sweep-task0.log 2>&1; echo "exit=$?"
   ```
   Expected: exit 0 (build-only, no RED gating locally), and
   `ls -t ~/.cache/sky/examples-sweep/sweep-*.table | head -1` prints a path.
   Re-READ `/tmp/sweep-task0.log` (do not re-run) to confirm a `BUILD` cell was
   emitted for `01-hello-world`.
4. Record disjointness: `git diff --name-only HEAD~3..HEAD | rg -c
   'constrain.rs|sky_kernels|lower.rs|emit_expr.rs|sky_ir'` — note the count in
   the commit body of Task 1 as evidence this plan's files don't overlap.
5. **Commit:** none (verification only). If step 3 fails, STOP — that is #35
   territory (a broken harness), not #37; file against #35 and do not proceed.

---

## Task 1 — `.gitattributes`: pin LF at the source (B7)

**Goal.** Close the single CRITICAL Windows breakage (`windows-ci-support.md`
B1/B7): an autocrlf checkout rewrites `scripts/*.sh` to CRLF → Git Bash dies
`$'\r': command not found` mid-sweep. LF pinning also stabilizes the phase-2
`sha256(source)` oracle staleness key cross-OS (Q4).

**Files produced:** `/.gitattributes` (NEW — verified absent at HEAD).

**Interfaces**

- Consumes: nothing (repo-root config).
- Produces: `git check-attr text eol -- scripts/examples-sweep.sh` reports
  `text: set` + `eol: lf`.

**Steps (TDD)**

1. **Failing test.** Create `scripts/tests/test_gitattributes.sh`:
   ```bash
   #!/usr/bin/env bash
   set -euo pipefail
   cd "$(git rev-parse --show-toplevel)"
   test -f .gitattributes || { echo "FAIL: .gitattributes missing"; exit 1; }
   for f in scripts/examples-sweep.sh scripts/lib/equiv_normalize_html.py \
            examples/01-hello-world/src/Main.sky scripts/equiv-classification.tsv; do
     eol="$(git check-attr eol -- "$f" | sed 's/.*: //')"
     [ "$eol" = "lf" ] || { echo "FAIL: $f eol=$eol (want lf)"; exit 1; }
   done
   echo "PASS"
   ```
   Run: `bash scripts/tests/test_gitattributes.sh` → expect
   `FAIL: .gitattributes missing`.
2. **Minimal impl.** Create `/.gitattributes`:
   ```gitattributes
   # Line endings: LF everywhere. Git Bash on windows-latest runs these scripts;
   # a CRLF rewrite corrupts them ($'\r': command not found) and desyncs the
   # phase-2 sha256(source) oracle staleness key. See docs/architecture/windows-ci-support.md B1/B7.
   * text=auto eol=lf
   *.sh   text eol=lf
   *.py   text eol=lf
   *.sky  text eol=lf
   *.mjs  text eol=lf
   *.toml text eol=lf
   scripts/equiv-classification.tsv text eol=lf
   # phase-2 oracle fixtures (present once A5 vendors them)
   *.expected text eol=lf
   tests/golden/**/expected_go.txt text eol=lf
   # true binaries — never normalize
   *.png binary
   *.ico binary
   ```
3. **Passing run.** `bash scripts/tests/test_gitattributes.sh` → `PASS`.
   (`git check-attr` reads `.gitattributes` from the worktree — no commit needed
   for the assertion, but renormalization applies on next checkout.)
4. **Commit:** `git add .gitattributes scripts/tests/test_gitattributes.sh &&
   git commit` — message notes files disjoint from registry/TCO branches.

---

## Task 2 — `env.sh`: parse `SKY_HOST_OS`, `SKY_PYTHON`, `.exe` SKYC_BIN, normalized `CARGO_TARGET_DIR`, timeout preflight

**Goal.** `env.sh` is sourced **first** (`examples-sweep.sh:45`, before
`checks.sh:47`), so it currently has **no** host detection and cannot gate the
`.exe` suffix on `SKY_HOST_OS`. **Spec correction to `windows-ci-support.md`
step 12:** that step assumes `SKY_HOST_OS` is available in `env.sh`; it is not.
Fix: **move** the host-detect block into `env.sh` (the first-sourced file);
`checks.sh` keeps its block behind a `[ -n "${SKY_HOST_OS:-}" ] ||` guard so it
stays correct if sourced standalone. Then resolve `SKY_PYTHON` once, normalize
`CARGO_TARGET_DIR` on Windows, and put `.exe` candidates ahead of the PATH
fallback in the SKYC_BIN loop.

**Files produced:** `scripts/lib/env.sh`.

**Interfaces**

- Consumes: `OSTYPE` / `uname -s`; `command -v python3|python`;
  `CARGO_TARGET_DIR` (may be `D:\a\...` under Git Bash).
- Produces (exact):
  - `SKY_HOST_OS ∈ {linux, macos, windows}` (exported).
  - `SKY_PYTHON` = absolute path to `python3` or `python`; **empty → later hard
    `exit 2`** at the free_port/preflight call sites (fail-closed).
  - `SKYC_BIN` — on Windows, `$CARGO_TARGET_DIR/release/skyc.exe` precedes the
    non-`.exe` candidates and the PATH fallback.
  - `CARGO_TARGET_DIR` forward-slash normalized on Windows via `cygpath -u`.

**Steps (TDD)**

1. **Failing test.** Create `scripts/tests/test_env_windows.sh` — fakes a Windows
   host and a built `skyc.exe`, asserts the `.exe` is chosen and `SKY_PYTHON`
   resolves:
   ```bash
   #!/usr/bin/env bash
   set -euo pipefail
   cd "$(git rev-parse --show-toplevel)"
   tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
   mkdir -p "$tmp/release"; : > "$tmp/release/skyc.exe"; chmod +x "$tmp/release/skyc.exe"
   OSTYPE="msys" CARGO_TARGET_DIR="$tmp" SKY_REPO="$PWD" SKYC_BIN="" \
     bash -c 'source scripts/lib/env.sh
              [ "$SKY_HOST_OS" = windows ] || { echo "FAIL host=$SKY_HOST_OS"; exit 1; }
              case "$SKYC_BIN" in *skyc.exe) ;; *) echo "FAIL skyc=$SKYC_BIN"; exit 1;; esac
              [ -n "$SKY_PYTHON" ] || { echo "FAIL: SKY_PYTHON empty"; exit 1; }
              echo PASS'
   ```
   Run → expect `FAIL skyc=...release/skyc` (no `.exe` today) or a host mismatch.
2. **Minimal impl.**
   - At the top of `env.sh` (before the `SKYC_BIN detection` block), add the
     host-detect block moved verbatim from `checks.sh:32-45`, exporting
     `SKY_HOST_OS`.
   - After `export CARGO_TARGET_DIR=...` (env.sh:29), on Windows normalize:
     ```bash
     if [ "$SKY_HOST_OS" = windows ] && command -v cygpath >/dev/null 2>&1; then
       CARGO_TARGET_DIR="$(cygpath -u "$CARGO_TARGET_DIR")"
     fi
     ```
   - Add `SKY_PYTHON` resolution (fail-closed downstream, not here):
     ```bash
     export SKY_PYTHON="${SKY_PYTHON:-$(command -v python3 || command -v python || true)}"
     ```
   - In the `for _cand in` loop (env.sh:74), prefix `.exe` candidates on Windows:
     ```bash
     _ext=""; [ "$SKY_HOST_OS" = windows ] && _ext=".exe"
     for _cand in \
       "$CARGO_TARGET_DIR/release/skyc$_ext" \
       "$CARGO_TARGET_DIR/debug/skyc$_ext" \
       "$CARGO_TARGET_DIR/release/skyc" \
       "$CARGO_TARGET_DIR/debug/skyc" \
       "$REPO/target/release/skyc$_ext" \
       "$REPO/target/debug/skyc$_ext" \
       "$REPO/target/release/skyc" \
       "$REPO/target/debug/skyc"; do
     ```
     and change the final default (env.sh:85) to append `$_ext`:
     `export SKYC_BIN="${SKYC_BIN:-$CARGO_TARGET_DIR/release/skyc$_ext}"`.
   - Add a `timeout` fail-loud preflight near the top (after host detect):
     ```bash
     command -v timeout >/dev/null 2>&1 || { echo "env.sh: timeout(1) required (GNU coreutils)"; return 1 2>/dev/null || exit 2; }
     ```
3. **Passing run.** `bash scripts/tests/test_env_windows.sh` → `PASS`. Regression
   on native host: `bash -c 'source scripts/lib/env.sh; echo "$SKY_HOST_OS
   $SKYC_BIN"'` → `linux .../release/skyc` (no `.exe`, unchanged).
4. **Lint gate:** `shellcheck -x scripts/lib/env.sh` → no new warnings.
5. **Commit.**

---

## Task 3 — `checks.sh`: `.exe` `resolve_bin` (+`binmiss`), `SKY_PYTHON` free_port, server-body CR-strip, guarded host-detect

**Goal.** Mirror the `.exe`-first / fail-closed discipline into the RUN side.
`resolve_bin` (`checks.sh:115`, `for b in` at `:118`, `find` fallback at
`:126-129`) must probe `sky-app.exe` **before** the `ls -t` freshest-file guess,
and a miss must be a **counted RED `binmiss`** rather than the race-prone guess.
`free_port` (`checks.sh:72`) must use `"$SKY_PYTHON"`. The server-body compare
gets a line-ending-scoped CR strip (defense-in-depth for phase-2 EQUIV).

**Files produced:** `scripts/lib/checks.sh`.

**Interfaces**

- Consumes: `SKY_HOST_OS` (now set by env.sh), `SKY_PYTHON`, `CARGO_TARGET_DIR`.
- Produces:
  - `resolve_bin <dir>` → prints an executable path and returns 0; on total miss
    returns a **distinct rc** the caller maps to `binmiss` (RED), **never** the
    `find | ls -t` guess on Windows.
  - `free_port` → an ephemeral port via `"$SKY_PYTHON"`.
  - server-body comparison strips only trailing `\r` (`sed 's/\r$//'`), never
    mid-line CR.

**Steps (TDD)**

1. **Failing test.** Create `scripts/tests/test_resolve_bin_windows.sh`:
   ```bash
   #!/usr/bin/env bash
   set -euo pipefail
   cd "$(git rev-parse --show-toplevel)"
   tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
   mkdir -p "$tmp/debug"; : > "$tmp/debug/sky-app.exe"; chmod +x "$tmp/debug/sky-app.exe"
   : > "$tmp/debug/other-stale";  chmod +x "$tmp/debug/other-stale"   # newer decoy
   OSTYPE=msys CARGO_TARGET_DIR="$tmp" SKY_REPO="$PWD" SKYC_BIN=skyc \
     bash -c 'source scripts/lib/env.sh; source scripts/lib/checks.sh
              got="$(resolve_bin /nonexistent-example)"; rc=$?
              case "$got" in *sky-app.exe) echo PASS;; *) echo "FAIL got=$got rc=$rc"; exit 1;; esac'
   ```
   Run → expect `FAIL` (today `resolve_bin` has no `.exe` candidate → falls to
   `find | ls -t` and may pick `other-stale`).
2. **Minimal impl.**
   - Guard `checks.sh`'s host-detect (`:32-45`) with
     `if [ -z "${SKY_HOST_OS:-}" ]; then … fi` (env.sh already set it; keep
     standalone-source correctness).
   - In `resolve_bin` (`:118`) add `.exe` candidates first when Windows:
     ```bash
     local ext=""; [ "${SKY_HOST_OS:-}" = windows ] && ext=".exe"
     for b in \
       "$CARGO_TARGET_DIR/debug/sky-app$ext" \
       "$CARGO_TARGET_DIR/release/sky-app$ext" \
       "$CARGO_TARGET_DIR/debug/sky-app" \
       "$CARGO_TARGET_DIR/release/sky-app" \
       "$CARGO_TARGET_DIR/debug/$name$ext" \
       "$d/sky-out/rust/target/debug/sky-app$ext" \
       "$d/sky-out/rust/target/debug/$name$ext"; do
       [ -n "$b" ] && [ -x "$b" ] && [ ! -d "$b" ] && { echo "$b"; return 0; }
     done
     ```
   - **On Windows only, disable the `find | ls -t` fallback** (the freshest-file
     race is the exact silent-wrong-binary vector). Replace the fallback tail so
     that when `SKY_HOST_OS=windows` and every explicit candidate missed, it
     `return 3` (a distinct "binmiss" rc) instead of guessing:
     ```bash
     if [ "${SKY_HOST_OS:-}" = windows ]; then return 3; fi   # binmiss — never the ls -t guess
     b="$(find "$CARGO_TARGET_DIR/debug" "$d/sky-out/rust/target/debug" -maxdepth 1 -type f -executable 2>/dev/null | xargs -r ls -t 2>/dev/null | head -1)"
     [ -n "$b" ] && { echo "$b"; return 0; }
     return 1
     ```
   - `free_port` (`:72`): `"$SKY_PYTHON" -c 'import socket;...'` (replace bare
     `python3`); if `SKY_PYTHON` empty the call fails → caller surfaces `noserve`,
     which is honest (fail-closed).
   - Server-body compare (the `exercise_server` body path): append `| sed
     's/\r$//'` to the body-normalize pipeline (defense-in-depth; moot until
     EQUIV wakes, cheap now).
3. **Passing run.** `bash scripts/tests/test_resolve_bin_windows.sh` → `PASS`.
   Native regression: same test with `OSTYPE=linux` and only `sky-app` present
   still resolves it.
4. **Wire `binmiss` into the RUN column + verdict** — done in Task 4 (the row
   emitter lives in `examples-sweep.sh`); this task only *produces* rc 3.
5. **Lint:** `shellcheck -x scripts/lib/checks.sh`. **Commit.**

---

## Task 4 — `examples-sweep.sh`: `_win_reap_app` + os-error-5 retry in `build_rust`, `binmiss` RED word, `norm()` CR-strip

**Goal.** Close the spurious-RED cascade (`windows-ci-support.md` B3): all
examples share one `CARGO_TARGET_DIR/.../sky-app.exe`; a lingering RUN holds the
handle so the next `cargo build` dies `Access is denied (os error 5)`, falsely
reddening every downstream row. Fix: a pre-build `taskkill` reap + an os-error-5
retry arm, both **inside** `build_rust` (the `reap()` in `checks.sh:75` is a
`pkill`-guarded Windows no-op and runs *between* examples — wrong locus). Also
map `resolve_bin` rc 3 → `binmiss` (RED), and give `norm()` (`:151`) the
line-ending-scoped CR strip.

**Files produced:** `scripts/examples-sweep.sh`.

**Interfaces**

- Consumes: `SKY_HOST_OS`, `resolve_bin`'s rc 3, cargo build log.
- Produces:
  - `_win_reap_app` — `taskkill //F //T //IM <name>` over the DELIBERATE SUPERSET
    `sky-app.exe msedgewebview2.exe winpty.exe winpty-agent.exe` (no-op off
    Windows). The `msedgewebview2.exe //IM` entry is UNPROVEN (no real webview
    build observed on Windows) — carried behind that documented caveat.
  - `build_rust` — calls `_win_reap_app` **pre-build**; on `cargo build` failure
    whose log matches `Access is denied \(os error 5\)|failed to remove file`
    (**ERE alternation `|` UNESCAPED** — `\|` would be a literal pipe and the
    retry would never fire), reaps + `sleep 3` + retries (bounded).
  - `binmiss` — a new RED word: when `resolve_bin` returns 3, the RUN cell is
    `binmiss` and the row is RED (never `skip`, never `ok`).
  - `norm()` — `grep -v '^[[:space:]]*$' | sed 's/\r$//'` (payload CR preserved).

**Steps (TDD)**

1. **Failing test — `norm()` CR handling.** Create
   `scripts/tests/test_norm_crlf.sh`:
   ```bash
   #!/usr/bin/env bash
   set -euo pipefail
   cd "$(git rev-parse --show-toplevel)"
   src="$(sed -n '/^norm() {/,/^}/p' scripts/examples-sweep.sh 2>/dev/null || rg -n 'norm\(\)' scripts/examples-sweep.sh)"
   # source just the harness far enough to reach norm(); simplest: eval the one-liner def.
   eval "$(rg -N '^norm\(\)' scripts/examples-sweep.sh)"
   printf 'a\r\nb\r\n\r\nc\r\n' > /tmp/norm.in
   out="$(norm /tmp/norm.in | od -c | tr -s ' ')"
   echo "$out" | rg -q '\\r' && { echo "FAIL: trailing CR survived"; exit 1; }
   echo "$out" | rg -q 'a *\\n *b *\\n *c' || { echo "FAIL: content garbled: $out"; exit 1; }
   echo PASS
   ```
   Run → expect `FAIL: trailing CR survived` (today `norm()` is
   `grep -v '^[[:space:]]*$'`, no CR strip).
2. **Failing test — reap + retry present & correct ERE.** Create
   `scripts/tests/test_build_rust_win.sh`:
   ```bash
   #!/usr/bin/env bash
   set -euo pipefail
   cd "$(git rev-parse --show-toplevel)"
   rg -q '_win_reap_app' scripts/examples-sweep.sh || { echo "FAIL: no _win_reap_app"; exit 1; }
   # os-error-5 retry uses UNESCAPED ERE alternation (not \|)
   rg -q 'Access is denied \(os error 5\)\|failed to remove file' scripts/examples-sweep.sh \
     || { echo "FAIL: os-error-5 retry arm missing/malformed"; exit 1; }
   rg -q 'binmiss' scripts/examples-sweep.sh || { echo "FAIL: binmiss RED word not wired"; exit 1; }
   echo PASS
   ```
   Run → expect `FAIL: no _win_reap_app`.
3. **Minimal impl.**
   - Define near the top of `examples-sweep.sh` (after sourcing):
     ```bash
     _win_reap_app() {
       [ "${SKY_HOST_OS:-}" = windows ] || return 0
       local p
       for p in sky-app.exe msedgewebview2.exe winpty.exe winpty-agent.exe; do
         taskkill //F //T //IM "$p" >/dev/null 2>&1 || true
       done
     }
     ```
   - In `build_rust` (`:110`), call `_win_reap_app` **before** the `cargo build`
     (`:132`), and add an os-error-5 retry arm around it. Since `build_rust`
     already has an `attempt` loop for skyc, add a small bounded cargo retry:
     ```bash
     local cargo_ok=0 catt
     for catt in 1 2 3; do
       _win_reap_app
       if ( cd "$d" && timeout 900 cargo build --manifest-path sky-out/rust/Cargo.toml >"$HIST/$n.cargo.log" 2>&1 ); then cargo_ok=1; break; fi
       if [ "$catt" -lt 3 ] && grep -qiE 'Access is denied \(os error 5\)|failed to remove file' "$HIST/$n.cargo.log"; then
         _win_reap_app; sleep 3; continue
       fi
       break
     done
     if [ "$cargo_ok" = 1 ]; then
       WARN_CELL="$(rg -o 'generated [0-9]+ warning' "$HIST/$n.cargo.log" 2>/dev/null | rg -o '[0-9]+' | tail -1)"; : "${WARN_CELL:=0}"
       BUILD_CELL="ok"; return 0
     fi
     BUILD_CELL="cargo-fail"; return 1
     ```
     (Replaces the single `if ( cd "$d" && … cargo build … )` block at `:132-136`.)
   - **`binmiss` wiring** in the RUN dispatch: where the harness calls
     `resolve_bin` to get `rbin`, capture rc; if rc == 3 emit `binmiss` and mark
     RED, e.g.:
     ```bash
     rbin="$(resolve_bin "$d")"; rbrc=$?
     if [ "$rbrc" = 3 ]; then printf 'binmiss\tbuilt but no locatable sky-app.exe\n'; return 0; fi
     ```
     and add `binmiss` to the RED classification in the verdict loop
     (`:376-377`): a row whose RUN cell is `binmiss` sets `row_red=1`.
   - `norm()` (`:151`):
     `norm() { grep -v '^[[:space:]]*$' "$1" 2>/dev/null | sed 's/\r$//' | head -200; }`
4. **Passing runs.** `bash scripts/tests/test_norm_crlf.sh` → `PASS`;
   `bash scripts/tests/test_build_rust_win.sh` → `PASS`.
5. **Syntax + lint:** `bash -n scripts/examples-sweep.sh`;
   `shellcheck -x scripts/examples-sweep.sh` (accept pre-existing warnings; add
   none). **Regression:** re-run Task 0's build-only dry-run — still emits a
   table, exit 0. **Commit.**

---

## Task 5 — Python normalizers: `\r\n → \n` at ingest (Q4 layer-3)

**Goal.** Make the phase-2 EQUIV normalizers CRLF-airtight so turning EQUIV on
later cannot spurious-DIFFER on a Windows-touched fixture. Line-ending-scoped
only — **never** `tr -d '\r'` (a mid-line CR sledgehammer that can make two
genuinely-different outputs compare EQUAL, a worse false-EQUAL).

**Files produced:** `scripts/lib/equiv_normalize_html.py`,
`scripts/lib/equiv_tui_grid.py`.

**Interfaces**

- Consumes: an HTML/grid string possibly containing `\r\n`.
- Produces: output with `\r\n` collapsed to `\n` at ingest; interior lone `\r`
  preserved; stdout pinned to `\n`.

**Steps (TDD)**

1. **Failing test.** Create `scripts/tests/test_normalizers_crlf.py`:
   ```python
   import subprocess, sys, pathlib
   root = pathlib.Path(subprocess.check_output(["git","rev-parse","--show-toplevel"]).decode().strip())
   html = b"<div sky-id=\"x\">a\r\nb</div>\r\n"
   p = subprocess.run([sys.executable, str(root/"scripts/lib/equiv_normalize_html.py")],
                      input=html, capture_output=True)
   assert b"\r\n" not in p.stdout, f"FAIL: CRLF survived: {p.stdout!r}"
   print("PASS")
   ```
   Run: `python3 scripts/tests/test_normalizers_crlf.py` → expect an
   `AssertionError` (today the reader is text-mode utf-8 without an explicit
   `\r\n`→`\n` and may echo CRLF on a Windows-authored input).
2. **Minimal impl.** In `equiv_normalize_html.py`, immediately after the read
   (`encoding='utf-8'` path), add `data = data.replace('\r\n', '\n')`; pin the
   final write to `sys.stdout` (text mode, `\n`). In `equiv_tui_grid.py` (reads
   `'rb'` + per-row `rstrip()`), add an explicit `\r\n`→`\n` at ingest before row
   splitting (moot while tui SKIPs, cheap for the filed D-D headless mode).
3. **Passing run.** `python3 scripts/tests/test_normalizers_crlf.py` → `PASS`.
   Regression: feed an interior lone `\r` (`b"a\rb"`) and assert it is **not**
   stripped (payload CR preserved).
4. **Commit.**

---

## Task 6 — `examples-sweep.yml`: three-host matrix + Windows steps (the workflow change-set)

**Goal.** Apply `windows-ci-support.md` § "Concrete change-set" items 1-9 to the
workflow: per-OS `experimental` matrix, `continue-on-error: ${{
matrix.experimental }}`, a Windows-only `core.autocrlf false` **before**
checkout, `actions/setup-python@v5`, a preinstalled-`rg`-preferred Windows
ripgrep guard, Windows Node install **without** Playwright browsers, forward-slash
`CARGO_TARGET_DIR` for the Windows job, and keeping any future Go≡Rust step
ubuntu-only. **No gate flip** — all three hosts stay `experimental: true`.

**Files produced:** `.github/workflows/examples-sweep.yml`.

**Interfaces**

- Consumes: the hardened harness (Tasks 2-5).
- Produces a workflow where `python3 -c "import yaml,sys;
  d=yaml.safe_load(open('.github/workflows/examples-sweep.yml'))"` shows:
  - `jobs['examples-sweep']['strategy']['matrix']['include']` contains an entry
    with `os: windows-latest`, `experimental: true`.
  - `jobs['examples-sweep']['continue-on-error'] == '${{ matrix.experimental }}'`.
  - a step with `if` gating `windows` for `core.autocrlf false` before checkout.

**Steps (TDD)**

1. **Failing test.** Create `scripts/tests/test_sweep_yml.py`:
   ```python
   import yaml, pathlib
   d = yaml.safe_load(pathlib.Path(".github/workflows/examples-sweep.yml").read_text())
   job = d["jobs"]["examples-sweep"]
   inc = job["strategy"]["matrix"]["include"]
   oses = {e["os"] for e in inc}
   assert {"ubuntu-latest","macos-latest","windows-latest"} <= oses, f"FAIL matrix: {oses}"
   assert all("experimental" in e for e in inc), "FAIL: per-OS experimental flag missing"
   assert job["continue-on-error"] == "${{ matrix.experimental }}", "FAIL: continue-on-error not per-OS"
   text = pathlib.Path(".github/workflows/examples-sweep.yml").read_text()
   assert "core.autocrlf false" in text, "FAIL: no autocrlf pre-checkout step"
   assert "setup-python@v5" in text, "FAIL: no setup-python"
   assert "playwright install --with-deps" not in text, "FAIL: forbidden --with-deps present"
   print("PASS")
   ```
   Precondition: `python3 -c "import yaml"` — if PyYAML is absent locally, install
   into a throwaway venv (`python3 -m venv /tmp/pv && /tmp/pv/bin/pip install
   pyyaml` — no repo/network policy change) or fall back to `rg` token
   assertions. Run → expect `FAIL matrix` (only ubuntu+macOS today).
2. **Minimal impl.** Edit `examples-sweep.yml`:
   - Replace `os: [ubuntu-latest, macos-latest]` (`:55`) with:
     ```yaml
     matrix:
       include:
         - { os: ubuntu-latest,  experimental: true }
         - { os: macos-latest,   experimental: true }
         - { os: windows-latest, experimental: true }   # stays true until independently green (D-B)
     ```
   - Change `continue-on-error: true` (`:47`) to
     `continue-on-error: ${{ matrix.experimental }}`.
   - **Before** the `actions/checkout@v4` step (`:77`), add:
     ```yaml
     - name: Disable autocrlf (Windows)
       if: startsWith(matrix.os, 'windows')
       run: git config --global core.autocrlf false
     ```
   - Add `- uses: actions/setup-python@v5` (with `python-version: '3.x'`) after
     checkout so `python` exists on Windows.
   - Windows toolchain/deps step:
     ```yaml
     - name: Windows deps (ripgrep preferred, Node without Playwright)
       if: startsWith(matrix.os, 'windows')
       shell: bash
       run: |
         command -v rg >/dev/null 2>&1 || choco install ripgrep -y   # LATEST (unpinned) — image-regression fallback only
         [ -f package.json ] && npm ci --no-audit --ignore-scripts || true   # NO playwright browsers
     ```
   - Windows job env: set a forward-slash target dir. Simplest robust form —
     override in a Windows step via `$GITHUB_ENV`:
     ```yaml
     - name: Windows CARGO_TARGET_DIR (forward slash)
       if: startsWith(matrix.os, 'windows')
       shell: bash
       run: echo "CARGO_TARGET_DIR=$(cygpath -u "$CARGO_TARGET_DIR")" >> "$GITHUB_ENV"
     ```
     Keep MSYS argv conversion **ON** (D-A: `//F` in the reap depends on it) — do
     **not** set `MSYS_NO_PATHCONV`/`MSYS2_ARG_CONV_EXCL` globally.
   - Leave the "Free disk space (Linux)" step Linux-gated; leave the commented
     `examples-sweep-equiv` stub's Go path ubuntu-only (Q4). Run-sweep step is
     already `shell: bash` — unchanged.
3. **Passing run.** `python3 scripts/tests/test_sweep_yml.py` → `PASS`. Validate
   YAML parses: `python3 -c "import yaml;
   yaml.safe_load(open('.github/workflows/examples-sweep.yml'))"` (exit 0). If
   `actionlint` can be obtained (`go install
   github.com/rhysd/actionlint/cmd/actionlint@latest`), run it as an extra gate;
   otherwise the `yaml.safe_load` + structural test is the floor.
4. **Commit** — body notes: informational-only, no gate flip, Windows
   `experimental: true` per D-B.

---

## Task 7 — `ci.yml`: mechcheck drift test + Windows `element_to_cells` unit lane; confirm cancel-in-progress + nightly

**Goal.** `ci.yml` already mirrors `mechcheck.sh` (fmt / clippy / test+doctest /
miri / sharded e2e) and already carries `concurrency.cancel-in-progress: true`
(`:15-17`) and the nightly `schedule` (`:12`). This task (a) adds a **drift test**
that fails if `ci.yml` and `mechcheck.sh` diverge on the core gate set — making
"CI silently stopped running a mechcheck gate" unrepresentable — and (b) adds the
one lane `windows-ci-support.md` items 17 calls for: a `windows-latest` unit lane
exercising `element_to_cells` (real headless cell-render coverage on Windows,
where the sweep's tui shape honestly SKIPs).

**Files produced:** `.github/workflows/ci.yml`,
`scripts/tests/test_ci_mirrors_mechcheck.sh`.

**Interfaces**

- Consumes: `plugins/sky-compiler/scripts/mechcheck.sh` (the SSOT gate list).
- Produces: `ci.yml` jobs cover `{fmt, clippy, test, doctest, miri}`; a
  `tui-windows` job runs `cargo test` for `element_to_cells` on `windows-latest`;
  drift test green.

**Steps (TDD)**

1. **Failing test.** Create `scripts/tests/test_ci_mirrors_mechcheck.sh`:
   ```bash
   #!/usr/bin/env bash
   set -euo pipefail
   cd "$(git rev-parse --show-toplevel)"
   ci=.github/workflows/ci.yml
   # Every mechcheck gate must appear in ci.yml.
   rg -q 'cargo fmt --all -- --check' "$ci"                 || { echo "FAIL: fmt gate"; exit 1; }
   rg -q 'cargo clippy .* -D warnings' "$ci"                || { echo "FAIL: clippy gate"; exit 1; }
   rg -q 'cargo nextest run' "$ci"                          || { echo "FAIL: test gate"; exit 1; }
   rg -q 'cargo test --doc' "$ci"                           || { echo "FAIL: doctest gate"; exit 1; }
   rg -q 'miri test' "$ci"                                  || { echo "FAIL: miri gate"; exit 1; }
   rg -q 'cancel-in-progress: true' "$ci"                   || { echo "FAIL: no cancel-in-progress"; exit 1; }
   rg -q "cron: '0 4 \* \* \*'" "$ci"                       || { echo "FAIL: no nightly"; exit 1; }
   rg -q 'element_to_cells|tui-windows' "$ci"               || { echo "FAIL: no windows tui unit lane"; exit 1; }
   echo PASS
   ```
   Run → expect `FAIL: no windows tui unit lane` (the first seven pass today;
   only the Windows `element_to_cells` lane is missing).
2. **Minimal impl.** Add one job to `ci.yml`:
   ```yaml
   tui-windows:
     runs-on: windows-latest
     steps:
       - uses: actions/checkout@v4
       - uses: dtolnay/rust-toolchain@stable
       - uses: Swatinem/rust-cache@v2
       # element_to_cells is a pure, pty-free cell renderer (runtime/src/sky_runtime/
       # tui/layout.rs). Real Windows cell-render coverage where the sweep's tui
       # shape honestly SKIPs (needs a console). See windows-ci-support.md Q3.
       - run: cargo test -p sky-runtime-rust --features tui element_to_cells
   ```
   Verify the crate/feature name against HEAD before committing: `rg -n
   'name = "sky-runtime-rust"|\[features\]|^tui' runtime/Cargo.toml` — if the
   package is named differently, use the actual name (do not hardcode blind). If
   `element_to_cells` is not yet reachable by a `#[test]`, gate this job behind a
   filed follow-up rather than shipping a red lane (fail-closed: a job that can't
   pass yet is `experimental`/commented with a filed task, never a silent skip).
3. **Passing run.** `bash scripts/tests/test_ci_mirrors_mechcheck.sh` → `PASS`;
   `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml'))"`
   exit 0.
4. **Note (no code):** the miri job scopes to specific crates
   (`ci.yml:65`) whereas `mechcheck --miri` is workspace-wide — a **deliberate**
   efficiency divergence (workspace miri is prohibitively slow on hosted
   runners). The drift test asserts *presence* of a miri gate, not scope, so this
   stays intentional and documented. **Commit.**

---

## Task 8 — Push to the public repo (fail-closed remote resolution + cancel-in-progress discipline)

**Goal.** Land the CI work on the public repo. **Spec ambiguity resolved
fail-closed:** the task/memory say push to `arthurmaciel/ipe-lang`, but the live
`origin` is `git@github.com:arthurmaciel/ipe.git` and `git ls-remote
git@github.com:arthurmaciel/ipe-lang.git` is **unreachable** from this box. The
push **parses the target once** (PARSE, DON'T VALIDATE) and **stops** if it can't
resolve a concrete reachable remote — it never force-pushes, never auto-creates a
repo, never guesses.

**Files consumed:** all Task 1-7 outputs (working tree).

**Interfaces**

- Consumes: a clean working tree with Tasks 1-7 committed on a **non-main**
  branch.
- Produces: commits pushed to the resolved public remote's default branch via a
  PR (never a direct push to a shared main/master), with in-progress non-tag CI
  runs cancelled first.

**Steps**

1. **Branch-first guard (project rule — never commit CI work straight to
   master).** If on `master`/`main`, branch:
   `git rev-parse --abbrev-ref HEAD`; if it is `master` or `main`, run
   `git switch -c ci/three-host-sweep-and-push`.
2. **Resolve the remote — fail-closed decision procedure:**
   ```bash
   if git ls-remote --exit-code git@github.com:arthurmaciel/ipe-lang.git >/dev/null 2>&1; then
     TARGET=git@github.com:arthurmaciel/ipe-lang.git
   elif git ls-remote --exit-code git@github.com:arthurmaciel/ipe.git >/dev/null 2>&1; then
     TARGET=git@github.com:arthurmaciel/ipe.git   # current origin
   else
     echo "STOP: neither ipe-lang nor ipe resolves — surface to user, do not guess"; exit 2
   fi
   echo "resolved push target: $TARGET"
   ```
   **Surface the discrepancy to the user** in the plan-execution summary: if
   `ipe-lang` is absent and we push to `ipe`, say so explicitly and ask whether a
   rename/new-repo is intended (this is spec ambiguity #1, below). Do **not**
   `gh repo create` without an explicit user ask.
3. **Cancel in-progress CI on main first (CLAUDE.md discipline) — never tag/release
   runs.** Against the resolved repo (`-R <owner/repo>`):
   ```bash
   REPO_SLUG="$(basename "$TARGET" .git)"; OWNER=arthurmaciel
   gh run list -R "$OWNER/$REPO_SLUG" --branch main --status in_progress \
       --workflow CI --json databaseId --jq '.[].databaseId' \
     | xargs -I{} gh run cancel -R "$OWNER/$REPO_SLUG" {} 2>/dev/null || true
   ```
   Only the `CI` workflow on `main` is cancelled; release/tag runs are never
   touched (the `--workflow CI --branch main` filter guarantees it).
4. **Push the branch + open a PR (no direct push to shared main):**
   ```bash
   git remote get-url ipe-public >/dev/null 2>&1 || git remote add ipe-public "$TARGET"
   git push -u ipe-public HEAD          # branch push; never --force
   gh pr create -R "$OWNER/$REPO_SLUG" --fill --base main   # or master, per repo default
   ```
   PR body: no Claude/AI attribution, no co-author trailer (project + user rules);
   summarize the three-host sweep + drift test + `.gitattributes`.
5. **Post-push verification (verification-before-completion):** re-list CI on the
   PR branch (`gh run list -R ... --branch ci/three-host-sweep-and-push`) and
   confirm both workflows were *triggered*; the sweep is informational so a RED
   sweep is expected and not a failure. **Commit:** nothing new — this task is the
   push itself.

---

## Task 9 — File the no-deferral follow-ups (tracked, not smuggled into this job)

**Goal.** `windows-ci-support.md` items 16-18 are runtime/verification work
**out of scope** for "add Windows CI". Per the no-deferral principle, file them so
they enter the pipeline rather than vanish.

**Files consumed:** none (task-tracker only).

**Steps**

1. Create three tracked tasks (via `TaskCreate`):
   - **D-D — `SKY_TUI_HEADLESS` one-shot render mode**: runtime change so a Tui
     example renders one frame via `element_to_cells` and exits 0 without
     crossterm — flips the sweep tui RUN green on all hosts and unlocks
     `equiv_tui_grid.py` cell-EQUIV. (Runtime, not CI.)
   - **D-C — webview feature-emission verification**: confirm skyc emits
     `--features webview` on `x86_64-pc-windows-msvc` and the crate links
     WebView2, *before* the Windows webview RUN row and the `msedgewebview2.exe
     //IM` reap superset are trusted (until then: UNPROVEN, not green).
   - **A5 — example oracle refresh tool**: the `refresh-example-oracle` sibling +
     `equiv_for()` cached-compare branch that lets EQUIV gate without a Go
     toolchain in CI (already tracked as #51 — link, don't duplicate).
2. **Commit:** none.

---

## Spec ambiguities resolved to make this mechanical

1. **`ipe` vs `ipe-lang` remote (highest-impact).** Live `origin` is
   `arthurmaciel/ipe.git`; `ipe-lang` is unreachable from this box. Resolution:
   Task 8 parses the target **once** via `git ls-remote --exit-code`, prefers
   `ipe-lang` if it resolves, falls back to the working `ipe`, and **STOPs +
   surfaces to the user** if neither resolves. No force-push, no silent repo
   creation. This is the one place the executor must confirm intent with the user
   before completing.

2. **`env.sh` sourced before `SKY_HOST_OS` exists (spec correction to
   `windows-ci-support.md` step 12).** The Windows doc gates the `.exe` SKYC_BIN
   candidates on `SKY_HOST_OS` inside `env.sh:73-85`, but `env.sh` is sourced at
   `examples-sweep.sh:45` — **before** `checks.sh:47` sets `SKY_HOST_OS`.
   Corrected: Task 2 **moves** host detection into `env.sh` (first-sourced) and
   guards `checks.sh`'s copy. Without this, the `.exe` gate is dead code.

3. **"Port `ci.yml`" when `ci.yml` already exists.** `../sky`'s `ci.yml` is a
   Haskell/Go pipeline (cabal test, Go build, console-drift, `sky fmt/check`) —
   not portable to a Rust workspace. ipê's `ci.yml` is already the Rust-native
   mirror of `mechcheck.sh`. Resolution: Task 7 treats "port" as **verify +
   drift-test + one missing lane**, not a rewrite. The mechcheck.sh↔ci.yml drift
   test is the durable artifact that keeps them mirrored.

4. **"cancel-in-progress-CI-on-main rule" = two distinct mechanisms.** (a) the
   workflow-level `concurrency: cancel-in-progress: true` (already in both yml —
   asserted by the Task 7 drift test), and (b) the **pre-push manual** `gh run
   cancel` discipline from CLAUDE.md (Task 8 step 3, CI-workflow + main-branch
   scoped so tag/release runs are never cancelled). Both are covered.

5. **"binmiss" is a new RED word, not present at HEAD.** The Windows doc names it
   but the harness has no such verdict today. Resolution: `resolve_bin` returns a
   distinct rc 3 (Task 3), and the row emitter + verdict loop map rc 3 → RED
   `binmiss` (Task 4). Made explicit so it's not left as a dangling term.

6. **No gate flip in this plan.** The task says "wire the mechcheck/sweep", which
   is *enable/verify*, not *make gating*. Per `sweep-and-parity-plan.md` the flips
   are staged and later; all three hosts stay `experimental: true`. Encoded in the
   Sequencing section and Task 6's commit note.
