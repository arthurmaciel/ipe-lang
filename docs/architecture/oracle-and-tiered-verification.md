# Equivalence oracle + tiered verification (task #51)

> **Purpose.** Replace "fresh-design + Opus-adversarial-review every slice" with
> "**faithful port** of `../sky`'s proven logic + **oracle-diff** verification".
> The oracle turns *"does skyc's emitted+run output behave like the reference?"*
> into an automatic byte-diff, so a **faithful-port slice needs no Opus review** —
> only the surfaces the oracle **cannot** cover (deliberate divergences, security
> seal, no-reference shapes) still get the guardian.
>
> Companion docs: `examples-sweep-port.md` (how the harness was ported),
> `go-oracle-fixture-corpus-plan.md` (the 65-fixture unit corpus + normalizer
> audit), `e2e-and-oracle-caching.md` (the cached golden oracle), and
> `divergence-policy.md` (sanctioned-divergence rules). This doc is the
> **reference-availability verdict + activation state + the tiering rule**.

---

## 1. Reference-availability verdict (measured on this host, 2026-07-04)

The oracle needs a **reference** to diff skyc against. There are two candidate
references and two oracle mechanisms. What is actually producible **on this
machine**:

| Reference | Toolchain needed | Available here? | Verdict |
|---|---|---|---|
| **Go, live** — Haskell `sky` builds each example's `sky-out/app` via its Go backend | `sky` on PATH + `go` | **YES** — `sky v0.16.29` at `/usr/local/bin/sky`; `go1.26.2` at `/usr/local/go/bin/go` | **ACTIVE** — this is `SKY_GO_BIN`; `build_go()` drives it. Proven `equivalence-stdout` below. |
| **Rust, via `../sky`** — `../sky`'s own Haskell compiler + Rust backend builds+runs the examples | built `sky` from `../sky` (needs GHC 9.4.8 + cabal) | **NO** — `../sky` has no `dist-newstyle`; host GHC is **8.8.4** (too old), **no `cabal`** | Blocked. Not needed — the live Go reference is sufficient and higher-value (byte-diff, not just "both boot"). |
| **Go, cached snapshot** — committed `expected_go.txt` per fixture | none at diff time | **YES (already built)** — `tools/oracle` + `refresh-oracle`, cache under `tests/golden/*/` | **ACTIVE** for the **unit** golden suite (see §5, Tier A). |

**Bottom line:** the **live Go oracle is producible and works on this host today**
(`sky v0.16.29` + `go1.26.2`). The `../sky` Rust reference is *not* buildable here
(GHC too old, no cabal) and is *not needed*: the Go backend is the authoritative
parity target the whole harness was designed around (`equivalence-classification.tsv`
already reasons about `sky v0.16.29`'s Go codegen quirks, e.g. the `undefined: T1`
regression on `00-standard-libs`).

### 1.1 The one blocker found and fixed — `SKY_RUNTIME_DIR` leak

The EQUIVALENCE machinery was **fully ported but dormant** (`SKY_SWEEP_NO_EQUIV=1`
default). Turning it on surfaced exactly **one** activation bug, now fixed:

- `scripts/lib/env.sh` exports `SKY_RUNTIME_DIR=$REPO/runtime/src/sky_runtime` so
  **skyc**'s `--runtime` auto-resolve is CWD-independent. That variable is a
  **skyc-only** knob.
- The Haskell `sky` (Go reference) **also honours `SKY_RUNTIME_DIR`** — and with
  it pointing at the repo's **Rust** runtime tree, `sky build` vendored the wrong
  runtime as its Go `rt/` package, so every `go build` died with
  `undefined: rt.SetPortDefault` / `rt.SkyADT` / `rt.RegisterAdtTag` … — reported
  by the sweep as the **AMBER `go-ref-broken`** cell (a false one).
- **Fix (in `build_go()`, `scripts/equivalence-checks/examples-sweep.sh`):** run the Go reference
  with the var scoped-unset — `env -u SKY_RUNTIME_DIR … "$go_bin" build …`. `sky`
  has its own TH-embedded Go runtime; the skyc knob must not leak to it.

This was a **scripts** fix — no compiler/runtime/crate change. After it,
`01-hello-world` moved `go-ref-broken → equivalence-stdout` (§3).

---

## 2. The oracle design (what a "diff" means per shape)

The harness is `scripts/equivalence-checks/examples-sweep.sh` + `scripts/lib/{env,examples,checks}.sh`
+ two Python normalizers, all ported from `../sky/runtime-rust/scripts/`. Per
in-scope example it emits one row with three cells — **BUILD · RUN · EQUIVALENCE** —
and the EQUIVALENCE cell is the oracle. The **mode** is *derived* from the example
shape (`equivalence_mode` in `lib/examples.sh`), overridable in
`scripts/equivalence-checks/equivalence-classification.tsv`:

| Shape | EQUIVALENCE mode | Oracle comparison (skyc-Rust **vs** Go reference) | Rigour today |
|---|---|---|---|
| cli | `stdout` | run both binaries, `norm()` (blank-line strip) each stdout, **byte-diff**. A 2-probe **determinism auto-gate** on the Go side downgrades a nondeterministic program to `n/a` rather than false-DIFFER. | **Real byte-diff oracle.** |
| server | `body` | boot both on isolated ports+cwds; for each comparable `Server.get` GET route, byte-compare bodies (per-route determinism + slow/streaming skips). | Real body-diff, **raw** (see §4 — HTML normalizer not yet wired). |
| live | `scenario` | browser round-trip via Playwright driver + `verify-scenarios.mjs`; **degrades to boot-both** when the browser stack is absent. | Boot-both floor here (no `node_modules/playwright`). |
| tui | `pty` | drive both under a pty; today asserts **both drive the runtime** (NOT cell-identical). | Boot-both floor (grid normalizer not wired — §4). |
| webview / fyne / Go-FFI / Rust-FFI | `none` | no comparable reference output → explicitly **`n/a`** with a reason. | Correctly **no-oracle** (→ Opus tier, §6). |

**Verdicts:** GREEN = BUILD ok ∧ RUN ok ∧ EQUIVALENCE ∈ {`equivalence-*`, `n/a`, `—`, amber
`go-ref-broken`}. RED = any `*-fail` / `panic` / `hang` / `noserve` / `notty` /
**`DIFFER`**. `go-ref-broken` is **AMBER** (the *reference* failed to build/serve —
not a skyc defect; it auto-flips back to `equivalence-*` the moment the reference is
fixed, no manual edit).

### 2.1 What I built / wired

- **Fixed the activation blocker** (`build_go()` `SKY_RUNTIME_DIR` unset) — §1.1.
- **Verified the live Go oracle end-to-end** on this host (§3) — the harness now
  emits real `equivalence-stdout`, not a dormant `—`.
- **This doc** — the reference verdict, activation state, run command, and the
  tiering rule (§6).

Everything else (the modes, `exercise_server_equiv`, `build_go`, the two
normalizers, `equivalence-classification.tsv`) was already ported intact — activation
was a **wiring fix, not a rebuild**, exactly as `examples-sweep-port.md` predicted.

---

## 3. Proof it works (sample run, this host)

```
$ SKY_GO_BIN=sky bash scripts/equivalence-checks/examples-sweep.sh          # RUST_EXAMPLES=01-hello-world
EXAMPLE                      BUILD      RUN       EQUIVALENCE            NOTE
-------                      -----      ---       -----            ----
01-hello-world               ok         ok        equivalence-stdout
  summary: 1 green · 0 red · 0 skipped (of 1) · amber go-ref-broken=0
  equivalence-mode breakdown: stdout=1 …
=== VERDICT: PASS ===
```

`equivalence-stdout` = skyc's emitted-Rust stdout **byte-matched** the Go reference
binary's stdout for `01-hello-world`. Before the §1.1 fix the same run reported a
false `go-ref-broken`. Provenance: `sky v0.16.29` (Go ref) · `go1.26.2` · `skyc`
debug build 2026-07-04.

A broader 4-example CLI sample shows the **honest pre-parity shape** — the oracle
is real where the Rust build succeeds; the RED rows are **skyc build-lane
failures, not oracle failures**:

```
01-hello-world               ok         ok        equivalence-stdout
20-cli-counter               skyc-fail  —         —            rust build failed
06-json                      skyc-fail  —         —            rust build failed
14-task-demo                 skyc-fail  —         —            rust build failed
  summary: 1 green · 3 red (all skyc-fail) · equivalence-mode: stdout=1
```

This matches `go-oracle-fixture-corpus-plan.md` §5: the corpus is a **burndown**
target, not a today-pass gate. Each example flips to `equivalence-stdout` (or a real
`DIFFER` to investigate) as its owning skyc phase lands. A `skyc-fail` / `DIFFER`
is Rust-side work; an `equivalence-*` is an Opus-retired faithful-port slice (§6).

> **Note on skyc binary choice:** `env.sh` prefers `$CARGO_TARGET_DIR/release/skyc`
> over `debug`. The stale **release** skyc (older) `cargo-fail`s with a workspace
> nesting error (`current package believes it's in a workspace`); the fresh
> **debug** skyc builds clean. Rebuild the release skyc (`cargo build --release -p
> skyc`) or pass `SKYC_BIN=…/debug/skyc` so the sweep uses a current compiler.
> This is a Rust build-lane item, not an oracle item.

---

## 4. Known oracle gaps (do NOT treat these as full oracles yet)

The oracle is **real for `stdout`**. Three surfaces are currently a *boot/raw*
floor, not a semantic diff — a slice touching them still needs Opus until wired:

1. **`body` uses a RAW byte-compare**, not `equivalence_normalize_html.py`. Go and Rust
   encode sky-ids differently (`r.1#div.15` vs `r_1_div_15`), sort attrs
   differently, and deliver pseudo/media/anim styles differently — all
   behaviourally identical but raw-DIFFER. Until the HTML normalizer is wired into
   `exercise_server_equiv`, `body` mode false-REDs on any non-trivial page, so it
   is effectively a **serve floor** for real UIs.
2. **`pty` is boot-both**, not cell-identical — `equivalence_tui_grid.py` (needs `pyte`,
   present here) is **not wired** into the `pty` branch.
3. **`scenario` degrades to boot-both** — no `node_modules/playwright` on this
   host (chromium + node ARE present; the driver `scripts/equivalence-checks/web-verify.mjs` +
   `verify-scenarios.mjs` and a `playwright` install are the missing pieces).

Wiring these (per `go-oracle-fixture-corpus-plan.md` §3) promotes `body`/`pty`/
`scenario` from **boot-floor (Opus-still-needed)** to **real oracle (Opus-retired)**.
The normalizer false-green/false-red audit in that doc (SVG-coord mask, event
collapse, charref canonicalisation, CRLF folding) MUST be honoured when wiring —
a normalizer that hides a real divergence is a correctness defect, not a
convenience.

---

## 5. Two oracle tiers (both live in this repo)

| Tier | Mechanism | Scope | Reference | State |
|---|---|---|---|---|
| **A — unit goldens** | `tools/oracle` (`check_parity`) + `refresh-oracle`; cached `expected_go.txt` + `oracle.meta` (sha256 staleness gate) per `tests/golden/<name>/` | small single-file programs (kernel/codegen slices) | **cached** Go stdout (no live Go at diff time) | **DONE** — wired into the golden suite; staleness/missing/divergence all hard-fail. |
| **B — example corpus** | `scripts/equivalence-checks/examples-sweep.sh` EQUIVALENCE column | the 33 in-scope example apps | **live** Go via `SKY_GO_BIN=sky` | **ACTIVE for `stdout`** (this doc); `body`/`pty`/`scenario` at boot-floor (§4). |

Tier A already encodes the **divergence discipline** the whole strategy needs:
`oracle_divergence=true` + a tagged `divergence_reason` (`sanctioned:` = ipê is
deliberately more correct, e.g. full-Unicode case mapping; `divergence:` = ipê
follows a different target, e.g. the float-exponent threshold; auto = Go itself
failed on the shape). A divergence is **recorded, never matched-to-Go, never
silently diffed**. Tier B inherits the same rule via `equivalence-classification.tsv`
overrides (mode `none` + reason).

---

## 6. The tiering rule — oracle-verifiable vs Opus-only

This is the ceremony-retirement contract. For any slice / example / aspect:

### 6.1 ORACLE-VERIFIABLE → **skip Opus**, trust the diff

A surface is oracle-covered when **all** hold:
- It is a **faithful port** of `../sky`'s proven logic (behaviour is *supposed* to
  match the reference — no intentional divergence).
- A **reference output is producible**: live Go (`SKY_GO_BIN`) for the example
  corpus, or a cached `expected_go.txt` for a golden.
- Its EQUIVALENCE mode is a **real diff today**: **`stdout`** (byte-diff) or a Tier-A
  golden. (`body`/`pty`/`scenario` qualify **only after** the §4 normalizers are
  wired; until then treat them as boot-floor, not oracle-covered.)

For these, a **green EQUIVALENCE / passing golden IS the review.** The guardian's
adversarial pass adds nothing a byte-diff against the proven reference does not
already prove. Faithful-port + green-oracle → **merge without Opus.**

### 6.2 NO-ORACLE → **Opus-only** (the diff cannot speak)

A surface needs the guardian when **any** hold:
- **Sanctioned / intentional divergence** — ipê deliberately differs from Go
  (typed errors instead of `Result String`, home-identity nominal typing,
  full-Unicode semantics, the float-exponent threshold, security gates that Go
  lacks). The oracle would false-RED; a human must rule the divergence sound and
  tag it (`oracle_divergence` / `equivalence-classification.tsv` mode `none`).
- **No producible reference** — Rust-FFI apps (no Go build), webview/fyne
  (no comparable output), anything the reference toolchain can't build here.
- **Security seal / soundness properties** that are **absolute, not
  Go-relative** — injection-safe emit (#47/F7 `70-style-injection`), auth-secret
  never stringified (`18-auth-signup`), panic-freedom, `parse-don't-validate`
  boundaries. These assert a property of *ipê's* output, not "== Go", so no diff
  can confirm them.
- **EQUIVALENCE mode still at boot-floor** (`body`/`pty`/`scenario` pre-normalizer) —
  "both boot" is not "behaves the same"; the semantic gap is Opus's until §4 lands.

### 6.3 Operating rule

> **Faithful port + green real-oracle ⇒ no Opus. Divergence, no-reference,
> security-seal, or boot-floor-only ⇒ Opus.** When unsure which bucket a slice is
> in, the tie-breaker is: *can a byte-diff against the proven reference fail if the
> slice is wrong?* If yes → oracle-verifiable. If the diff would be green even when
> the slice is wrong (masked, no reference, or an intended divergence) → Opus.

---

## 7. How to run the oracle

**Prerequisites (all present on this host):** `sky` (Go reference) on PATH,
`go`, `curl`, `python3`, `rg`, a current `skyc`.

```bash
# Full corpus, live Go oracle ON (drop SKY_SWEEP_NO_EQUIV to enable EQUIVALENCE):
cd /home/arthur/Documentos/comp/sky-rust
SKY_GO_BIN=sky bash scripts/equivalence-checks/examples-sweep.sh

# Single example / subset (paths or basenames):
SKY_GO_BIN=sky RUST_EXAMPLES="01-hello-world 20-cli-counter" bash scripts/equivalence-checks/examples-sweep.sh

# Force the fresh compiler if the release skyc is stale (see §3 note):
SKYC_BIN=~/.cache/sky-rust-target/debug/skyc SKY_GO_BIN=sky bash scripts/equivalence-checks/examples-sweep.sh

# Phase-1 (no oracle, BUILD+RUN only) — the CI default today:
SKY_SWEEP_NO_EQUIV=1 bash scripts/equivalence-checks/examples-sweep.sh
```

Output: an aligned table + `~/.cache/sky/examples-sweep/sweep-<stamp>.table`;
per-example logs (`<n>.<stamp>.skyc.log`, `<n>.<stamp>.cargo.log`,
`<n>.<stamp>.go.build.log`, `<n>.<stamp>.diff.txt`, …) under the same dir,
STAMP-suffixed (#35b) so two invocations sharing this cache dir never
interleave-corrupt each other's diagnostic files for the same example. Exit
0 = no RED row.

---

## 8. What remains to FULLY activate

Ordered by leverage:

1. **Wire the HTML normalizer into `body` mode** (`exercise_server_equiv` →
   pipe both bodies through `scripts/lib/equivalence_normalize_html.py` before diff).
   Promotes every server/live app from boot-floor to real render-diff. Honour the
   false-green audit (drop the blanket SVG-coord mask for ipê; canonicalise
   char-refs; fold CRLF). — the single biggest Opus-retirement win.
2. **Wire the tui-grid normalizer into `pty` mode** (`equivalence_tui_grid.py`; `pyte`
   already installed). Real cell-diff for the 2 tui examples.
3. **Install the browser stack** (`npm i playwright` + the `web-verify.mjs` /
   `verify-scenarios.mjs` drivers) so `scenario` mode does real round-trips
   instead of boot-both.
4. **Rebuild the release `skyc`** so `env.sh`'s release-first probe picks a
   current compiler (the stale release skyc `cargo-fail`s on the workspace-nesting
   error). Build-lane item.
5. **Flip CI to phase-2**: uncomment the `examples-sweep-equivalence` job stub in
   `.github/workflows/examples-sweep.yml`, set `SKY_GO_BIN`, drop
   `SKY_SWEEP_NO_EQUIV`. Two supported reference modes there: (a) build/install the
   Haskell `sky` on the runner (needs GHC ≥9.4.8 + cabal — **not** this host's
   8.8.4), or (b) the cached-snapshot oracle (no toolchain, à la Tier A) — mode
   (b) is the cheaper CI path and reuses `tools/oracle`'s format.
6. **Author the 65-fixture unit corpus** (`go-oracle-fixture-corpus-plan.md`) so
   the silent-divergence kernel classes (Dict/Set order, Money rounding, i64 wrap,
   json HTML-escape, float threshold) get a Tier-A oracle each — these are the
   dangerous *pass-skyc-mismatch-Go-silently* classes.

Until 1–3 land, the honest tiering is: **`stdout` examples + Tier-A goldens are
oracle-verified (Opus-retired); everything else stays Opus.** That already retires
the ceremony for the largest, cheapest class of faithful-port slices.
