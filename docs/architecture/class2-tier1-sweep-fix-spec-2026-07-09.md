# Class 2 implementation spec — Tier-1 sweep/CI/push (#35, #110, #37)

> Classification: **MECHANICAL** (`campaign-classification-2026-07-09.md`, Class
> 2 — "Tier-1 sweep/CI/push infrastructure"). No open design question — this is
> a scripting/porting job. Where a decision was already made by a prior design
> pass, this spec cites the doc and does not re-derive it. Where NO reference
> exists in `../sky` (confirmed by direct search — see §3.2 and §3.3 below), the
> spec says so explicitly and gives a concrete implementation anyway (still
> mechanical: wiring two already-vendored pieces together, not a new design).
>
> Written from a live audit of this repo + `../sky` on 2026-07-09. Every "done"
> / "not done" claim below was verified by reading the actual file, not assumed
> from the backlog prose (the backlog text for #110 and #35 undersells how much
> is already landed — read §1 before doing any work here, or you will redo
> things that are already committed).

---

## 0. Precondition — DO NOT START #35's sweep-green run until Class 1 lands

Per `BACKLOG.md` Tier-1 ORDER: **sweep-green → seal → #110 →
#37 → #59 → push**. "sweep-green" depends on the inference-cluster fix tracked
in `docs/architecture/class1-inference-fix-spec-2026-07-09.md` ("Boundary
Scheme Promotion" — untyped top-level bindings sharing one monomorphic var
across module boundaries). That fix is a separate, already-fully-specified
guardian-design item with its own implementation doc. **This spec does not
re-derive it and does not require you to implement it.** Your job here is
scripts/CI/oracle infrastructure; the compiler fix is out of scope.

Concretely: before declaring #35's sweep "green" (§2.3's Definition of Done),
confirm the Class 1 fix has landed on `master` (check `git log --oneline
--grep="Boundary Scheme Promotion"` or read the top of
`class1-inference-fix-spec-2026-07-09.md` for its landed-status marker). If it
has not landed yet, you can still do ALL of the following without waiting:

- Every part of #110 that is pure harness/CI plumbing (§3.1–§3.6 below) — none
  of it depends on the compiler accepting more programs, it only depends on the
  sweep infrastructure being correct.
- Every part of #37 that is not "run the sweep and declare it green" (§4 below).

Only the **final** "run the full sweep, confirm zero RED rows, declare
sweep-green" step (§2.3) is gated on Class 1. Do the infrastructure work now;
run the gating sweep once Class 1's fix is confirmed landed.

---

## 1. Current-state audit (read this before touching anything)

The backlog entries for #35/#110/#37 read as if this is greenfield. It is not.
Here is what is **already committed on `master`** as of 2026-07-09, verified by
reading the files directly (not the backlog prose):

| Sub-item | State | Evidence |
|---|---|---|
| `scripts/examples-sweep.sh` ported to drive `skyc` | ✅ DONE | `scripts/examples-sweep.sh` (445 lines), `docs/architecture/examples-sweep-port.md` |
| `scripts/lib/{env,examples,checks}.sh` ported | ✅ DONE | present, `wc -l` 92/187/404 |
| `scripts/lib/equiv_normalize_html.py` + `equiv_tui_grid.py` vendored | ✅ DONE (verbatim) | present in `scripts/lib/` |
| `scripts/equiv-classification.tsv` ported | ✅ DONE | present, 32 lines |
| 35 example dirs vendored under `examples/` | ✅ DONE | `ls examples/ | wc -l` → 35 (33 in-scope per the Go-FFI filter + `simple`/`test_pkg`) |
| `.github/workflows/examples-sweep.yml` (ubuntu+macOS matrix, phase-1 BUILD+RUN) | ✅ DONE, phase-1 only | present, 206 lines, `SKY_SWEEP_NO_EQUIV: '1'` |
| `.github/workflows/ci.yml` (fmt/clippy/test/miri/e2e, Rust-native) | ✅ DONE, already good | present, 125 lines |
| Live Go-reference oracle (`SKY_GO_BIN`) activated + pinned to v0.17.3 | ✅ DONE | `tools/oracle/` (`bin/sky` pinned binary, `README.md`), `docs/architecture/oracle-and-tiered-verification.md` §1, §3 |
| `SKY_RUNTIME_DIR` leak bug in `build_go()` (false `go-ref-broken`) | ✅ FIXED | `oracle-and-tiered-verification.md` §1.1, verified in `scripts/examples-sweep.sh:146-161` (`env -u SKY_RUNTIME_DIR`) |
| HTML normalizer wired into `body`-mode EQUIV | ✅ DONE | commit `63f57b2` "sweep: wire HTML normalizer into body-mode oracle comparison (#110)"; `scripts/lib/checks.sh:283-320` calls `equiv_normalize_html.py` from `exercise_server_equiv` |
| tui-grid normalizer (`equiv_tui_grid.py`) wired into `pty`-mode EQUIV | ❌ NOT DONE | confirmed via `rg -n "equiv_tui_grid" scripts/examples-sweep.sh scripts/lib/checks.sh` → zero hits; `pty` mode still hardcodes `equiv-pty\tboth drive runtime (NOT cell-identical)` (`scripts/examples-sweep.sh:215-225`) |
| Playwright / browser stack for `scenario` mode | ❌ NOT DONE | no `node_modules/`, no `package.json`, no `scripts/web-verify.mjs` / `scripts/verify-scenarios.mjs` in this repo — `scenario` mode degrades to boot-both (§4 of `oracle-and-tiered-verification.md`) |
| Release `skyc` build (`$CARGO_TARGET_DIR/release/skyc`) | ❌ STALE/ABSENT | only `~/.cache/sky-rust-target/debug/skyc` exists on this host; `env.sh`'s release-first probe will silently fall back to debug (slower sweep) or pick a stale release binary if one existed |
| CI phase-2 (Go≡Rust EQUIV in the workflow) | ❌ NOT DONE | `examples-sweep.yml`'s `examples-sweep-equiv` job is still a commented-out stub (lines 173-207) |
| 65-fixture non-FFI divergence corpus | ❌ NOT AUTHORED | `crates/skyc/tests/` has no `sky/` fixture subdir at all (only `support/`); `go-oracle-fixture-corpus-plan.md` is a complete plan with zero fixtures ported |
| `scripts/equiv-corpus.sh` / `scripts/equiv-render.sh` drivers | ❌ NOT PORTED | confirmed absent in this repo; exist in `../sky/runtime-rust/scripts/{equiv-corpus.sh,equiv-render.sh}` (both read below, §3.2/§3.3) |
| `vendor/upstream-sky/` submodule | ❌ NOT DONE (tracked separately) | `repo-layout-and-mirroring.md` §5 step 2 still pending; `ci.yml`'s `e2e` job self-skips on its absence by design — **not this spec's job to add**, see §4.1 |
| Windows CI (`windows-latest` matrix entry) | ❌ NOT DONE, fully spec'd | `docs/architecture/windows-ci-support.md` is a complete, ready-to-execute 18-item change-set; not yet applied to `examples-sweep.yml` |
| CI example-patch-queue (`docs/divergences-from-sky.md#planned-future-divergences` §6.9) | N/A — **intentionally deferred** | disposition: "accepted (2026-07-05), **execute at Tier-3 start**" — see §4.3, do not build this now |
| Push to a public remote | ❌ NOT DONE | current `origin` = `git@github.com:arthurmaciel/ipe.git`; GitHub also has an **empty** `arthurmaciel/ipe-lang` repo (see §4.4) — name mismatch needs resolving before the real push, not before this spec's work |

Read this table again before starting: roughly 60% of #35 and 15% of #110 is
already merged. Your job is the ❌ rows.

---

## 2. #35 — port examples-sweep to skyc + run the full sweep

### 2.1 What's left to do

Nothing to *port* — §1 confirms the harness is fully ported and already
exercised the live Go oracle successfully (`oracle-and-tiered-verification.md`
§3's proof-of-work run). What's left is purely **operational**:

1. Rebuild a fresh **release** `skyc` (the stale/missing release binary is a
   build-lane issue, not a script issue — see `oracle-and-tiered-verification.md`
   §3's note: *"the stale release skyc `cargo-fail`s with a workspace nesting
   error; the fresh debug skyc builds clean"*).
2. Confirm the Class 1 fix (§0) has landed.
3. Run the full sweep with the live Go oracle on, across all 33 in-scope
   examples.
4. Triage every RED row per CLAUDE.md's no-deferral principle: each RED row
   becomes its own backlog entry (spotted = filed) — do not fold sweep-green
   fixes into this spec's scope; file them and let the mechanical/guardian
   swarm burn them down separately.
5. Once the sweep is all-GREEN (or all remaining REDs are separately filed and
   the sweep itself is confirmed to be running correctly, not silently
   swallowing failures), declare "sweep-green" — this unblocks #110's CI-phase-2
   flip and #37.

### 2.2 Exact commands

```bash
cd /home/arthur/Documentos/comp/sky-rust

# 0. Disk hygiene first (CLAUDE.md non-negotiable #6) — the sweep builds 33
#    Cargo projects into a shared target dir.
df -h /
# if free space < 15-20 GB:
go clean -cache 2>/dev/null || true
cargo clean --manifest-path Cargo.toml 2>/dev/null || true   # only if truly needed

# 1. Fresh release skyc (this also satisfies #110 item 4, §3.4 below — do it once).
cargo build --release -p skyc
ls -la ~/.cache/sky-rust-target/release/skyc     # confirm it exists and is fresh

# 2. Confirm Class 1 landed (adjust the grep to whatever commit message /
#    marker the Class 1 spec's implementer used):
git log --oneline --all | rg -i "boundary.scheme.promotion|class.?1.*untyped.*generaliz"
# If nothing found, STOP — do not run step 3 as a gating claim yet. You may
# still run it informationally to see the current shape of RED rows.

# 3. Full sweep, live Go oracle ON, every in-scope example:
SKY_GO_BIN="$PWD/tools/oracle/bin/sky" \
SKYC_BIN=~/.cache/sky-rust-target/release/skyc \
SKY_SWEEP_FORCE=1 \
bash scripts/examples-sweep.sh
echo "exit: $?"

# Table + logs land under:
ls -t ~/.cache/sky/examples-sweep/sweep-*.table | head -1
```

If the full run is too slow to keep in one shot, use `RUST_EXAMPLES` to shard
it (all shards must eventually be run and combined into one verdict):

```bash
SKY_GO_BIN="$PWD/tools/oracle/bin/sky" SKY_SWEEP_FORCE=1 \
RUST_EXAMPLES="00-standard-libs 01-hello-world 02-go-stdlib 04-local-pkg 06-json" \
bash scripts/examples-sweep.sh
```

### 2.3 Definition of done

- `bash scripts/examples-sweep.sh` (full corpus, `SKY_GO_BIN` set) exits 0
  ("VERDICT: PASS").
- Every non-GREEN row is one of: `skip` (no comparable RUN — correct, per
  `example_shape`), `n/a`/`—` EQUIV (correct per §2 of
  `oracle-and-tiered-verification.md`), or `go-ref-broken` (amber, Go reference
  itself failed to build — not a Rust defect).
- Zero `skyc-fail` / `cargo-fail` / `panic` / `hang` / `noserve` / `notty` /
  `DIFFER` rows remain unfiled. Any that appear are filed as their own backlog
  entries (per CLAUDE.md §4 no-deferral) **before** declaring sweep-green —
  filing is not the same as fixing, but an un-filed RED row is not allowed to
  vanish from view.
- `WARN_TOTAL` (cargo warnings leaking past the generated `#![allow]`) is 0, or
  every non-zero warning is filed (the sweep already hard-fails on this by
  default, `SKY_SWEEP_WARN_GATE=1`).

---

## 3. #110 — oracle full-activation

`docs/architecture/oracle-and-tiered-verification.md` §8 already enumerates the
exact ordered punch list ("What remains to FULLY activate"). This section turns
that punch list into file-level instructions. Items are numbered to match that
doc's §8 ordering.

### 3.1 Item 1 — HTML normalizer in `body` mode: verify + one hardening follow-up

**Status: landed** (commit `63f57b2`). Action here is verification + closing one
documented gap, not net-new wiring:

1. Verify: run the sweep against a `server`-shaped example with EQUIV on and
   confirm the normalizer actually fires:
   ```bash
   SKY_GO_BIN="$PWD/tools/oracle/bin/sky" SKY_SWEEP_FORCE=1 \
   RUST_EXAMPLES="15-http-server" bash scripts/examples-sweep.sh
   ```
   Check `~/.cache/sky/examples-sweep/15-http-server.equiv` — it should be a
   diff of *normalized* HTML (attribute-sorted, sky-id-collapsed), not raw
   server output. `scripts/lib/checks.sh:283-320` (`exercise_server_equiv`) is
   the call site if you need to confirm the wiring.
2. **Residual Rule-1 hardening (documented, not yet done):**
   `scripts/lib/equiv_normalize_html.py:104` still masks every SVG coordinate
   attr (`d`/`x`/`y`/`cx`/`cy`/`r`/…) to the literal string `'#'` inside any
   `<svg>` subtree (`SVG_COORD` set at line 45, mask applied at line 104,
   comment: *"mask known-divergent chart coords (Go bug, PR #136)"`). This was
   copied verbatim from `../sky` where it exists to hide a **known Go bug**. Per
   `go-oracle-fixture-corpus-plan.md` §3.3 and §6 ("HARDEN (Rule 1)"), this is a
   **false-green hole for skyc**: a genuine skyc SVG-coordinate regression would
   render as an empty diff. Fix: do not blanket-mask; instead compare SVG
   coordinates against a stored-correct snapshot (or drop the mask entirely and
   accept that this ONE fixture may show a legitimate `oracle_divergence` against
   the known-buggy Go output, tagged per `divergence-policy.md`). This is a
   small, scoped fix to `equiv_normalize_html.py`'s `SVG_COORD` handling — file
   it as its own follow-up if you don't have time to land it in this pass, but
   note it explicitly (do not silently leave the mask in place without a filed
   item, per CLAUDE.md §4).

### 3.2 Item 2 — wire the tui-grid normalizer

**No prior-art wiring exists anywhere** (confirmed: `../sky/runtime-rust`'s own
`examples-sweep.sh` also does NOT call `equiv_tui_grid.py` from its `pty` EQUIV
branch — that repo wires tui-grid comparison ONLY through the separate
`equiv-render.sh` driver, read in full below). So there is no "port this exact
diff" move here; the correct move — confirmed by reading the actual reference
files — is:

**Port `../sky/runtime-rust/scripts/equiv-render.sh` → `scripts/equiv-render.sh`**,
adapted the same way `examples-sweep.sh` was adapted (per
`examples-sweep-port.md`'s table): the reference drives ONE compiler binary
with `--backend go` / `--backend rust`; this repo has TWO binaries (the pinned
Go oracle `tools/oracle/bin/sky`, and `skyc` for Rust). Concrete adaptation:

```bash
# Reference (../sky/runtime-rust/scripts/equiv-render.sh) build_both():
#   "$SKY" build --backend go src/Main.sky    → cp sky-out/app  → $GO_BIN
#   "$SKY" build --backend rust src/Main.sky  → find …/target/debug/sky-app → $RUST_BIN
#
# Adapted build_both() for this repo:
#   ( cd "$d" && env -u SKY_RUNTIME_DIR timeout 300 "$GO_ORACLE" build src/Main.sky )
#     → cp "$d/sky-out/app" "$GO_BIN"                         (Go reference)
#   ( cd "$d" && timeout 600 "$SKYC_BIN" build sky.toml --out sky-out/rust )
#     → resolve_bin "$d"  (reuse checks.sh's resolve_bin)     → $RUST_BIN
```

Concrete steps:

1. Copy `../sky/runtime-rust/scripts/equiv-render.sh` to
   `scripts/equiv-render.sh` verbatim first, then apply these edits:
   - Replace the `SKY="${SKY_BIN:-$REPO/sky-out/sky}"` line with two
     resolutions: `GO_ORACLE="${SKY_GO_BIN:-$REPO/tools/oracle/bin/sky}"` and
     `SKYC_BIN` (source `lib/env.sh`, which already resolves it).
   - In `build_both()`: replace the two `"$SKY" build --backend {go,rust}`
     invocations with the adapted forms above. Keep the `rm -rf sky-out
     .skycache .skydeps` between the two builds (same reason as the reference:
     the rust build's `sky-out` wipe would otherwise clobber a copied Go binary
     if you don't `cp` it out first — the reference already does this, keep the
     ordering).
   - `capture_tui()` (the `pty.fork()` + `TIOCSWINSZ` Python helper) is
     backend-agnostic — port verbatim, no changes needed.
   - `equiv_tui()`: unchanged logic (build both, capture both at a fixed
     80×`rows` winsize, run each capture through
     `scripts/lib/equiv_tui_grid.py <capture> <rows>`, diff the two grid
     outputs). Update `NORM_TUI="$_dir/lib/equiv_tui_grid.py"` path (already
     correct if you copy into `scripts/`).
   - `TUI_EXAMPLES=(24-tui-kitchen-sink)` — confirm this example exists in this
     repo's `examples/` (it does — see §1's vendored list). Also add
     `21-tui-stopwatch` / `22-tui-stopwatch-ui` / `23-tui-todo` if you want
     broader tui coverage; `24-tui-kitchen-sink` alone is the minimum per the
     reference.
2. Run it once pyte is confirmed installed (it already is on this host per
   `oracle-and-tiered-verification.md` §4's "pyte already installed" note —
   verify with `python3 -c "import pyte"`; if missing, `pip install --user
   pyte`).
   ```bash
   bash scripts/equiv-render.sh tui 24-tui-kitchen-sink
   ```
   Expected output: `✓ equiv (styled cell grid identical)` or a `DIFFER` with a
   line-count and a path to the diff file.
3. This closes `oracle-and-tiered-verification.md` §4 item 2 ("pty is
   boot-both, not cell-identical") — `tui` fixtures now qualify as
   ORACLE-VERIFIABLE per §6.1 of that doc once this lands, for the examples
   listed in `TUI_EXAMPLES`. The `pty` mode inside `scripts/examples-sweep.sh`
   itself (the main sweep, not this standalone driver) stays at the boot-both
   floor — that is intentional per the reference's own architecture (tui-grid
   equivalence lives in the separate `equiv-render.sh` driver, not inline in
   the per-example sweep loop; do not try to inline it into
   `examples-sweep.sh`'s `equiv_for()` "pty" case — that would duplicate the
   build-both logic `equiv-render.sh` already owns).

### 3.3 Item 3 — install the browser stack for `scenario` mode

Three missing pieces, confirmed absent (§1):

1. **`package.json` + Playwright.** This repo has no `package.json` at all.
   Create one at the repo root:
   ```bash
   cd /home/arthur/Documentos/comp/sky-rust
   npm init -y
   npm install --save-dev playwright
   npx playwright install chromium --with-deps
   ```
   (`--with-deps` installs the OS-level chromium dependencies; on a dev box
   without root, drop `--with-deps` and `apt install` the deps manually — see
   `../sky`'s CI step for the exact apt package list if needed, or just install
   via `npx playwright install-deps chromium`.)
2. **The browser driver + scenario library.** Port these two files verbatim
   from `../sky/scripts/` (they already exist there and are referenced by name
   in `scripts/lib/checks.sh:88` (`DRIVER="${DRIVER:-$REPO/scripts/web-verify.mjs}"`)
   and `checks.sh:89` (`SCENARIOS="${SCENARIOS:-$REPO/scripts/verify-scenarios.mjs}"`)
   — those exact filenames are ALREADY the expected paths, they're just not
   present yet):
   - `../sky/scripts/verify-scenarios.mjs` → `scripts/verify-scenarios.mjs`
     (13.5 KB — the per-example Playwright scenario library `scenario_for()`
     keys into, per `checks.sh`'s `scenario_for()` function).
   - There is no `web-verify.mjs` under that exact name in `../sky/scripts/` —
     grep for the closest match and adapt (`../sky/scripts/verify-examples.mjs`
     is the nearest analogue: a Playwright driver that takes an example
     name/port/scenario and drives a real browser against it). Confirm the
     CLI contract `checks.sh`'s `exercise_live()` expects:
     `node "$DRIVER" "$ex" "$port" "$scen" "$abin"` (4 positional args: example
     name, port, scenario key, absolute binary path — the driver itself boots
     the binary, so it must accept a binary path, not assume it's already
     running). Adapt `verify-examples.mjs`'s CLI surface to match this exact
     signature; save as `scripts/web-verify.mjs`.
3. **Verify `WEB_OK` flips to 1** after installing:
   ```bash
   source scripts/lib/env.sh; source scripts/lib/checks.sh; echo "WEB_OK=$WEB_OK"
   ```
   All four preconditions in `checks.sh:91-95` must hold: `node` on PATH,
   `$SKY_CHROMIUM` executable (defaults to `/usr/bin/chromium` — on a dev box
   without system chromium, either symlink Playwright's downloaded chromium
   binary there or export `SKY_CHROMIUM=$(find ~/.cache/ms-playwright -name
   chrome -o -name headless_shell | head -1)`), `$DRIVER` file present,
   `node_modules/playwright` dir present.
4. Once `WEB_OK=1`, re-run the sweep on a `live`-shaped example and confirm the
   EQUIV cell reads `equiv-scenario` (real browser round-trip) instead of the
   `equiv-serve` boot-floor fallback:
   ```bash
   SKY_GO_BIN="$PWD/tools/oracle/bin/sky" SKY_SWEEP_FORCE=1 \
   RUST_EXAMPLES="09-live-counter" bash scripts/examples-sweep.sh
   ```

This closes `oracle-and-tiered-verification.md` §4 item 3.

### 3.4 Item 4 — rebuild the release `skyc`

Already covered in §2.2 step 1 (`cargo build --release -p skyc`). No separate
action needed here beyond confirming `scripts/lib/env.sh`'s probe order (`
$CARGO_TARGET_DIR/release/skyc` first) picks it up:

```bash
source scripts/lib/env.sh
echo "$SKYC_BIN"   # must print .../release/skyc, not .../debug/skyc
```

### 3.5 Item 5 — flip CI to phase 2

Edit `.github/workflows/examples-sweep.yml`. Two changes:

1. **Uncomment and adapt the `examples-sweep-equiv` job stub** (lines
   173-207). It currently sketches "build the Haskell `sky` on the runner" —
   that is the WRONG path for this repo (we don't build Haskell `sky` from
   source; we use the **pinned release binary** at `tools/oracle/bin/sky`, per
   `tools/oracle/README.md`). Replace the stub with:

   ```yaml
     examples-sweep-equiv:
       name: examples-sweep-equiv (ubuntu, Go≡Rust)
       runs-on: ubuntu-latest
       env:
         CARGO_TARGET_DIR: ${{ github.workspace }}/.cache/sky-rust-target
         SKY_NO_SCCACHE: '1'
         SKY_SWEEP_FORCE: '1'
         SKY_AUTH_TOKEN_SECRET: sky-ci-sweep-test-secret-0123456789-abcdef
         SKY_SWEEP_BUILD_TIMEOUT: '900'
       steps:
         - uses: actions/checkout@v4
         - uses: dtolnay/rust-toolchain@stable
         - name: Cache cargo target
           uses: actions/cache@v4
           with:
             path: |
               ~/.cargo/registry
               ${{ github.workspace }}/.cache/sky-rust-target
             key: ubuntu-sweep-equiv-cargo-${{ hashFiles('Cargo.lock') }}
             restore-keys: ubuntu-sweep-equiv-cargo-
         - name: Install ripgrep + xvfb + go (Go reference needs `go build`)
           run: |
             sudo apt-get update
             sudo apt-get install -y ripgrep xvfb
         - uses: actions/setup-go@v5
           with: { go-version: '1.26', cache: true }
         - name: Fetch the pinned Go-reference `sky` binary
           run: |
             mkdir -p tools/oracle/bin
             # Same release asset tools/oracle/README.md documents (v0.17.3,
             # sky-linux-x64). Pin the URL to that exact asset; do not fetch
             # "latest" (version skew vs the port target reads as a false
             # parity failure — see tools/oracle/README.md "Why pin").
             curl -fsSL -o /tmp/sky-linux-x64.tar.gz \
               "https://github.com/<upstream-org>/sky/releases/download/v0.17.3/sky-linux-x64.tar.gz"
             tar -xzf /tmp/sky-linux-x64.tar.gz -C tools/oracle/bin
             chmod +x tools/oracle/bin/sky
             tools/oracle/bin/sky --version
         - name: Build skyc (release)
           run: cargo build --release -p skyc
         - name: Run examples sweep (BUILD + RUN + EQUIV)
           env:
             SKY_GO_BIN: ${{ github.workspace }}/tools/oracle/bin/sky
           run: bash scripts/examples-sweep.sh   # SKY_SWEEP_NO_EQUIV unset → EQUIV on
         - name: Job summary
           if: always()
           shell: bash
           run: |
             set +e +o pipefail
             HIST="$HOME/.cache/sky/examples-sweep"
             table="$(ls -t "$HIST"/sweep-*.table 2>/dev/null | head -1)"
             { echo "## examples-sweep-equiv (Go≡Rust, ubuntu)"; echo;
               [ -f "$table" ] && { echo '```'; cat "$table"; echo '```'; }
             } >> "$GITHUB_STEP_SUMMARY"
             exit 0
         - uses: actions/upload-artifact@v4
           if: always()
           with:
             name: examples-sweep-equiv
             path: |
               ~/.cache/sky/examples-sweep/sweep-*.table
               ~/.cache/sky/examples-sweep/*.log
             if-no-files-found: warn
   ```

   Replace `<upstream-org>` with the real GitHub org/repo the `v0.17.3
   sky-linux-x64.tar.gz` release asset lives under (check
   `tools/oracle/README.md` / whoever fetched the currently-committed pinned
   binary for the exact release URL they used — do not guess a URL that
   doesn't exist).

2. **Keep this job non-gating (`continue-on-error: true`) until #35's
   sweep-green is confirmed** (§2.3), then flip it to gating alongside the main
   `examples-sweep` job's `continue-on-error` flag, per
   `examples-sweep-port.md`'s own instruction: *"FLIP `continue-on-error` to
   false … once skyc reaches example parity."*

3. **Windows stays EQUIV-OFF** (per `windows-ci-support.md` §Q4 architectural
   rule: *"keep the Go≡Rust oracle ubuntu-only … Windows' distinct value is
   cross-platform MSVC build+run soundness, not re-checking Go parity."*). Do
   not add a Windows leg to this new job.

### 3.6 Item 6 — author the 65-fixture non-FFI divergence corpus

`docs/architecture/go-oracle-fixture-corpus-plan.md` is the complete manifest
(65 fixtures, Tier 0→4 priority order, exact divergence class per fixture). Do
not re-derive the list — follow it. Concrete porting mechanics:

1. **Port the two standalone drivers** from `../sky/runtime-rust/scripts/`:
   - `equiv-corpus.sh` → `scripts/equiv-corpus.sh` (pure-stdlib deterministic-
     stdout driver, Tier 0 fixtures).
   - `equiv-render.sh` → `scripts/equiv-render.sh` (already covered in §3.2 for
     the tui half; the same file also has the `live`/HTML half — port both
     halves together, you already need this file for §3.2).

   Adapt `equiv-corpus.sh` the same way as `equiv-render.sh` (§3.2): replace
   the single `"$SKY" build --backend {go,rust}` calls with the two-binary
   form (`tools/oracle/bin/sky` for Go, `$SKYC_BIN` for Rust). Also change:
   - `FIXROOT="runtime-rust/tests/sky"` → `FIXROOT="crates/skyc/tests/sky"`
     (this repo's fixture home — see step 2).
   - The `build_one()` Go-path invocation needs `env -u SKY_RUNTIME_DIR` in
     front of it (the same `SKY_RUNTIME_DIR`-leak fix already applied in
     `examples-sweep.sh`'s `build_go()` — port that fix here too, or this
     driver will hit the exact false `go-ref-broken` bug §1.1 of
     `oracle-and-tiered-verification.md` already diagnosed and fixed once).
   - The Rust-path invocation: `"$SKYC_BIN" build sky.toml --out
     sky-out/rust` (or `src/Main.sky` if the fixture has no `sky.toml`, mirror
     `skyc_build_target()` from `examples-sweep.sh`), then `resolve_bin
     "$d"` (source `checks.sh` for this — it's already sourced at the top of
     the reference file).
   - `CORPUS_DEFAULT` list: leave it as a **starting seed** but expand it to
     the full Tier-0 list from `go-oracle-fixture-corpus-plan.md` §2 as each
     fixture is authored (see step 2's priority order).

2. **Author fixtures under `crates/skyc/tests/sky/<name>/`** (mirroring
   `src/Main.sky` + `sky.toml`), in this exact priority order (Tier 0 first —
   `go-oracle-fixture-corpus-plan.md` §2 and §1a-e give the full 65-row table;
   reproduced here as the execution order, do not reorder without cause):

   **Tier 0 (10 fixtures, port first — pure-stdlib, no Go oracle/normalizer
   needed beyond `norm()`'s blank-line+timestamp strip):**
   `kernel-parity-probe`, `kernel-parity-probe-set`,
   `kernel-parity-probe-money`, `kernel-parity-probe-dbdec`,
   `kernel-parity-probe-dbdec2`, `63-int-overflow-wrap`, `alloc-stress`,
   `65-crypto-random-encoding`, `67-random-float-bounds`, `64-log-with-attrs`,
   plus the already-corpus-seeded `23-char`, `60-errortostring-string`,
   `54-discard-task-effect`, `56-list-sort`, `31-system-env-chain`,
   `37-cache-cli`.

   **Tier 1 (13 fixtures, ERROR-class, cheap):** `44-curried-return`,
   `57-record-alias-any`, `51-let-lambda-param-infer`, `52-task-fn-capture`,
   `53-cons-pattern-tuple`, `59-result-passthrough-nosig`,
   `62-nonclone-capture`, `45-usermod-kernel-collision`, `101-task-rethunk`,
   `102-task-rethunk-free-tvar`, `103-task-rethunk-discard`,
   `codegen-generic-recursive-adt`, `codegen-record-destructure-param`,
   `71-panic-classifier`, `49-bytes-core`.

   **Tier 2 (7 fixtures, render-silent — needs §3.1/§3.2's normalizers, which
   land before this tier):** `69-html-render-parity`, `70-style-injection`,
   `71-style-merge`, `40-live-ui`, `34-live-pubsub-dict`, `38-tui-ui`,
   `41-tui-input`. `70-style-injection`/`69-html-render-parity`/`71-style-merge`
   can land as **stored-HTML security snapshots with no Go oracle at all**
   (`go-oracle-fixture-corpus-plan.md` §4) — do these AS SOON AS the fixtures
   exist, independent of the oracle wiring, since they assert an absolute
   property of skyc's own output.

   **Tier 3 (18 fixtures, structural — server/live/db/auth, needs the
   server-body + live-scenario EQUIV paths, i.e. §3.1 + §3.3 above):**
   `21-sse-server`, `22-sse-relay`, `24-http-api`, `43-ws-server-capturing`,
   `68-server-413`, `28-live-counter`, `29-live-form`, `30-live-routing`,
   `31-live-req`, `27-live-static`, `32-live-sessions`, `33-live-pubsub`,
   `35-live-db-startup`, `50-event-handler-arc`, `17-db-todo-cli`,
   `18-auth-signup`, `19-config`, `20-email`, `68-db-migrate-cli`,
   `66-db-postgres-compile`, `67-db-sqlvalue-params`.

   **Tier 4 (2 fixtures, build-only):** `39-webview`, `66-db-postgres-compile`
   (listed twice in the source plan under different tiers — it is both a
   structural CLI fixture and a build-only compile gate; author once, exercise
   both ways).

   For each fixture: **do not invent new Sky programs from scratch** — the
   fixture names + divergence classes come from `../sky/runtime-rust/tests/sky/`
   (140 fixtures total there, 65 non-FFI). Locate the matching fixture there
   (`find ../sky/runtime-rust/tests/sky -maxdepth 1 -iname '<name>*'`) and port
   its `src/Main.sky` + `sky.toml`, adapting only what
   `go-oracle-fixture-corpus-plan.md` explicitly calls out as needing adaptation
   (mostly: none — these are meant to be Sky programs that build identically on
   both backends; if a fixture references Rust-only FFI or a Go-only kernel,
   that is itself the divergence class the fixture exists to pin, not a mistake
   to fix).

3. **CRLF hardening** (`go-oracle-fixture-corpus-plan.md` §3.2) — do this
   NOW while authoring, not as an afterthought:
   - Add `.gitattributes` at repo root (if it doesn't exist — confirmed absent
     in §1's Windows-CI audit too, so this single file serves both #110 and
     #37's Windows work, see §4.2):
     ```
     *.golden          text eol=lf
     crates/skyc/tests/sky/**/*.sky   text eol=lf
     crates/skyc/tests/sky/**/*.toml  text eol=lf
     ```
   - `equiv-corpus.sh`'s `norm()` already strips blank lines + normalizes the
     timestamp; per the plan's §3.2 add `| sed 's/\r$//'` to its output pipe
     too (cheap, future-proofs cross-OS runs even though this driver is
     ubuntu-only today).

4. **Rule 1/Rule 2 hardening while wiring** — apply
   `go-oracle-fixture-corpus-plan.md` §3.3's audit findings as you wire each
   normalizer: the SVG-mask fix (§3.1 above), and canonicalizing char-refs
   (`&#34;` vs `&quot;`) in `equiv_normalize_html.py` before diffing (currently
   `convert_charrefs=False` — verify whether this already canonicalizes; if
   not, add a canonicalization pass mapping both forms to one before compare).
   Tag the two KNOWN sanctioned divergences called out in that doc (the float
   stringify threshold and the JSON HTML-escape non-behaviour) with
   `sanctioned.divergence` markers per `divergence-policy.md`'s exact recipe —
   do not "fix" skyc to match Go on these two, they are settled decisions.

5. **Track as a burndown, not a today-pass gate** — per
   `go-oracle-fixture-corpus-plan.md` §5: most of Tiers 2-4 will be RED on
   first authoring (skyc doesn't yet implement every surface they exercise).
   That is expected. File each first-RED fixture's underlying gap as its own
   backlog item (no-deferral) rather than skipping the fixture.

---

## 4. #37 — fix CI + push

### 4.1 `ci.yml` — already good, do not rewrite

Per `examples-sweep-port.md`'s own note (§"ci.yml — deliberately untouched"):
the reference `../sky/.github/workflows/ci.yml` is a Haskell+Go pipeline with
**no Rust fmt/clippy/test/miri jobs to port** — this repo's existing `ci.yml`
already has superior Rust-native gates. **Do not port the reference `ci.yml`
wholesale; it would regress this repo's CI.** The only two things worth
checking in the existing `ci.yml`:

1. The `e2e` job's `vendor/upstream-sky/runtime-rust/src/sky_runtime` detection
   step (lines 111-119) currently self-skips because that submodule doesn't
   exist yet (`repo-layout-and-mirroring.md` §5 step 2, tracked separately —
   **not this spec's job to add the submodule**). Confirm this is still
   working as an honest self-skip (not a silent failure) by running the
   workflow once (`gh workflow run ci.yml` or push a no-op commit) and checking
   the `e2e` job's summary shows `"Sky runtime not vendored yet — E2E shard
   … skipped"` rather than a red X.
2. Nothing else needs changing in `ci.yml` for #37.

### 4.2 `examples-sweep.yml` — flip phase-2 (§3.5) + add Windows

Two independent changes to this one file:

1. **Phase-2 EQUIV job** — §3.5 above, verbatim.
2. **Windows-latest matrix entry** — `docs/architecture/windows-ci-support.md`
   is a complete, ready-to-execute spec (not a design doc with open questions;
   its "Top open decisions" section resolves every D-A through D-D item with a
   concrete recommendation). Execute its **"Concrete change-set (spec, no
   code)"** section verbatim — 18 numbered items split across the workflow
   file, `scripts/examples-sweep.sh`, `scripts/lib/env.sh`,
   `scripts/lib/checks.sh`, `scripts/lib/equiv_normalize_html.py` +
   `equiv_tui_grid.py`, and a root `.gitattributes`. Do not re-derive any of
   these — the doc already resolved the tradeoffs (Git-Bash reuse over a native
   PowerShell port, `taskkill //F //T` handle-lock reap, per-OS
   `experimental` gating flag, MSYS path-conversion left ON). Key excerpts to
   action directly:
   - Matrix → `strategy.matrix.include` with `{ os, experimental }` triples
     (ubuntu: true, macos: true, windows: true), `continue-on-error: ${{
     matrix.experimental }}`.
   - `_win_reap_app` (`taskkill //F //T //IM sky-app.exe`, `msedgewebview2.exe`,
     `winpty.exe`, `winpty-agent.exe`) called pre-build in `build_rust()`, plus
     an os-error-5 retry arm — **use the escaped `//F //T` form, and copy the
     alternation regex correctly** (`windows-ci-support.md` B3 explicitly flags
     that the reference's own `grep -E` alternation has an unescaped-pipe bug
     that silently defeats the retry — port `../sky`'s ACTUAL working
     `examples-sweep.sh:147` regex, not a re-typed version).
   - `.exe` candidates ahead of the `find` fallback in BOTH `resolve_bin`
     (`checks.sh:115`) and the `SKYC_BIN` probe loop (`env.sh:73-85`); a miss
     becomes a counted RED `binmiss`, never the freshest-file guess (B2).
   - `SKY_PYTHON` resolution in `env.sh` (`python3` vs `python`, B4).
   - `.gitattributes` at repo root, `eol=lf` on `*.sh`/`*.py`/`*.sky`/`*.mjs` —
     **this is the same file §3.6 step 3 already introduces**; merge the two
     sets of patterns into one `.gitattributes` (don't create two).
   - Root cause every item in the doc's "Trap ledger" table — it's a checklist
     of what must be foreclosed, use it as your own PR self-review checklist.
   - Gating stays `experimental: true` for Windows even after ubuntu/macOS flip
     to gating (§Q6) — Windows flips independently once it reaches one
     all-green Windows BUILD+RUN sweep on its own.

### 4.3 CI example-patch-queue — do NOT build this now

The filed idea's own disposition line
(`docs/divergences-from-sky.md#planned-future-divergences` §6.9): *"accepted
(2026-07-05), **execute at Tier-3 start** — first consumer is #128/#133; wire
`--patched` mode into the sweep + CI (#37) **when the first departure
lands**."* No Sky-surface departure (#128 drop-`Task.run`, #133 margin
stripping) has landed yet — Tier-3 has not started (Tier-1 is still in
progress, per the ORDER in §0). **Action for #37 today: none.** Leave a single
comment pointer in `scripts/examples-sweep.sh`'s header (if not already
present) noting the `--patched` mode is designed but intentionally not yet
implemented, citing `docs/divergences-from-sky.md#planned-future-divergences`. Do not scaffold an
empty patch-queue directory or a no-op `--patched` flag — that adds
maintenance surface with zero present consumer, contrary to the doc's own
"queue is empty, sweep runs pristine" until-then state.

### 4.4 Push mechanics — resolve the remote-name question, do not push

Verified live on GitHub (2026-07-09, via `gh repo view`):

- `origin` (this checkout) = `git@github.com:arthurmaciel/ipe.git` — an
  **empty** private repo (`gh api repos/arthurmaciel/ipe/commits` → "Git
  Repository is empty").
- `arthurmaciel/ipe-lang` — also exists, also **empty**, same description
  ("A programming language inspired by Elm and Sky that compiles to Rust").
- The backlog's #37 line says push to `git@github.com:arthurmaciel/ipe-lang`,
  which does not match the currently configured `origin`.

Both repos are empty, so nothing is at risk either way, but **do not guess**
which one is the intended final target — confirm with the user before the
actual `git push` (which in any case is explicitly out of scope for this spec
— see the task framing). Record for whoever does the eventual push:

1. Per the Tier-1 ORDER (`sweep-green → seal → #110 → #37 → #59 → push`), the
   literal `git push` is the LAST step, gated on **#59 (the Sky→Ipê rename)**
   landing first — not something #37 does on its own, despite #37's backlog
   one-liner mentioning "push to ipe-lang" in the same breath. Treat #37's
   scope as: fix/port the CI workflows (§4.1-§4.2) + confirm they would pass
   against the current tree. The remote reconciliation (`git remote set-url
   origin git@github.com:arthurmaciel/ipe-lang.git` or renaming/deleting the
   `ipe` repo) and the actual `git push -u origin main` happen as their own
   step, after #59, with the user's explicit go-ahead on which of `ipe` /
   `ipe-lang` is authoritative.
2. Before that eventual push, run through `docs/architecture/repo-layout-and-
   mirroring.md`'s §5 migration checklist for anything still pending (item 2,
   vendoring `vendor/upstream-sky`, is explicitly tracked separately and is
   NOT a precondition for #37/#59/push per that doc's own step numbering — it's
   a parallel, longer-running effort).

---

## 5. Sequencing summary (what to actually run, in order)

1. `cargo build --release -p skyc` (§2.2 step 1 / §3.4).
2. Confirm Class 1 fix status (§0). If not landed, proceed with 3-6 anyway
   (they don't depend on it); hold off on the final "declare sweep-green" claim.
3. §3.1 — verify HTML-normalizer wiring; file the SVG-mask hardening follow-up.
4. §3.2 — port `equiv-render.sh`, wire tui-grid, verify on
   `24-tui-kitchen-sink`.
5. §3.3 — install Playwright + port `web-verify.mjs`/`verify-scenarios.mjs`,
   verify `WEB_OK=1`, verify `equiv-scenario` on `09-live-counter`.
6. §3.5 — flip CI phase-2 stub to a real job (non-gating for now).
7. §4.2 — add the Windows matrix leg to `examples-sweep.yml` per
   `windows-ci-support.md`'s change-set (non-gating for now); create/merge the
   root `.gitattributes`.
8. §3.6 — author the 65-fixture corpus, Tier 0 → Tier 4, filing each new RED as
   its own backlog item.
9. Once Class 1 is confirmed landed: run the full gating sweep (§2.2 step 3),
   triage to zero unfiled REDs, declare sweep-green (§2.3).
10. Flip `examples-sweep.yml`'s `continue-on-error` to `false` for ubuntu/macOS
    (Windows stays `experimental: true` until its own independent all-green run
    per §4.2's last bullet).
11. Stop here for this spec's scope. #59 (rename) and the actual push are
    separate, later steps per §0/§4.4.

---

## 6. Definition of done for this spec (Class 2 as a whole)

- [ ] `scripts/equiv-render.sh` exists, ported + adapted, `bash
      scripts/equiv-render.sh tui 24-tui-kitchen-sink` runs to completion
      (pass or a real, inspectable DIFFER — not a setup error).
- [ ] `scripts/equiv-render.sh live <example>` likewise runs to completion once
      Playwright is installed.
- [ ] `scripts/equiv-corpus.sh` exists, ported + adapted, runs against the
      Tier-0 fixture set once fixtures exist under `crates/skyc/tests/sky/`.
- [ ] `crates/skyc/tests/sky/` contains fixtures ported in Tier 0→4 order; each
      fixture's first-run RED (if any) is filed as its own backlog item, not
      silently left red.
- [ ] `node_modules/playwright` installed; `WEB_OK=1` when `scripts/lib/checks.sh`
      is sourced.
- [ ] `.github/workflows/examples-sweep.yml` has: (a) an uncommented, working
      `examples-sweep-equiv` job pointed at `tools/oracle/bin/sky`; (b) a
      `windows-latest` matrix entry per `windows-ci-support.md`'s change-set;
      (c) both still `continue-on-error: true` until their respective
      gating criteria are met.
- [ ] Root `.gitattributes` exists, covering both the CRLF fixture-corpus need
      (§3.6) and the Windows CI need (§4.2) in one file.
- [ ] `~/.cache/sky-rust-target/release/skyc` is fresh (rebuilt after the
      latest master commit used for the sweep-green run).
- [ ] Full `scripts/examples-sweep.sh` run (§2.2) exits 0 with zero unfiled RED
      rows — **gated on Class 1 landing** (§0); do not claim this box checked
      until that precondition is verifiably true.
- [ ] Remote-name discrepancy (`ipe` vs `ipe-lang`, §4.4) flagged to the user;
      no `git push` executed by this work (out of scope, gated on #59 per the
      Tier-1 ORDER).

## 7. Explicit non-goals (do not do these under this spec)

- Do not implement the Class 1 inference fix (separate spec, separate owner).
- Do not build the CI example-patch-queue mechanism (§4.3 — Tier-3, not now).
- Do not add `vendor/upstream-sky` as a submodule (tracked in
  `repo-layout-and-mirroring.md`, separate multi-step migration, not a
  precondition for #35/#110/#37).
- Do not perform the Sky→Ipê rename (#59) or the final `git push` — both are
  later steps in the Tier-1 ORDER, explicitly out of scope for this
  spec-writing task per the task framing.
- Do not rewrite `ci.yml` wholesale from the `../sky` reference (§4.1) — it
  would regress this repo's already-superior Rust-native gates.
