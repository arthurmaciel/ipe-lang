# A4→A5 execution plan — the examples sweep on CI + establishing Go-parity

> Design-ahead (doc-only). This note specifies **how the ipê examples sweep
> runs on GitHub CI (A4)** and **how Go≡Rust parity is established there and
> its red rows triaged into fixes (A5)** — the next critical-path phase after
> the exit-0 registry pass. It touches no code; it fixes the plan the sweep
> harness (`scripts/equivalence-checks/examples-sweep.sh`, already ported) will execute.

Grounding. The whole sweep exists to serve two fundamental rules and the
correctness principle:

- **Rule 1 — "if it compiles, it works."** A well-typed Sky program must never
  skyc-succeed and then cargo-fail, panic, or hang. The sweep's BUILD+RUN
  columns make any *exit-0-then-cargo-fail* loud. That silent shape is the
  single worst outcome and the sweep's primary quarry.
- **Rule 2 — Go-parity is the default** (PRINCIPLES §2 Correctness). For the
  same well-typed program and input, Rust output matches the Go reference's
  observable behaviour, ideally byte-for-byte. Any deviation is either a bug to
  fix or a **documented** divergence — never silent. The EQUIVALENCE column is the
  enforcement of Rule 2.

A largely-RED first sweep is therefore the honest measurement, not a harness
failure: it is the to-do generator that drives the compiler to parity.

---

## Current state (investigated)

### The sweep harness — ported, phased, non-gating

`scripts/equivalence-checks/examples-sweep.sh` emits one row per in-scope example with three
columns, and one VERDICT:

| Column | Values | Meaning |
|---|---|---|
| BUILD | `ok` / `skyc-fail` / `cargo-fail` | `skyc build` then `cargo build` of the emitted crate |
| RUN | `ok` / `panic` / `hang` / `noserve` / `notty` / `skip` | headless drive per shape |
| EQUIVALENCE | `equivalence-*` / `n/a` / `DIFFER` / `go-ref-broken` | Go≡Rust comparison (PHASED OFF now) |

- **Scope** = `build_set` (every candidate dir minus Go-FFI, filtered by
  `is_out_of_scope` in `lib/examples.sh`) — **33 vendored in-scope dirs**; 9
  Go-FFI dirs excluded (`03,05,07,08,11,13,35,36`, `rust/skyshop-rs`).
- **Shape** (`example_shape`) derives EQUIVALENCE **mode** (`equivalence_mode`):
  `cli→stdout`, `server→body`, `live→scenario`, `tui→pty`,
  `webview/fyne→none`. `equivalence-classification.tsv` holds only the *exceptions*
  (non-deterministic clis, Rust-FFI apps): `00`(finding), `02`, `07`, `35`,
  `skyshop-rs` are pinned `none`.
- **Verdict** — RED = any `*-fail` / `panic` / `hang` / `noserve` / `notty` /
  `DIFFER`; **AMBER** = `go-ref-broken` (upstream, not counted RED);
  `skip` = neutral. **PASS iff no RED row.**
- **CI** — `.github/workflows/examples-sweep.yml`, ubuntu + macOS matrix,
  `continue-on-error: true`, env `IPE_SWEEP_NO_EQUIV=1` (BUILD+RUN only). It is
  **informational**: prints the table into the job summary, uploads the
  scoreboard artifact, surfaces the verdict, but does not fail the workflow.
- `ci.yml` is untouched and independent: `fmt` / `clippy -D warnings` /
  `test` (nextest + doctests) / `miri` / sharded `e2e`. The **golden parity
  suite runs inside `test`/`e2e`** (below).

### The oracle mechanism — two DIFFERENT things, only one is wired

1. **Golden suite (wired, proven).** `tests/golden/<name>/` — **174 registered
   goldens** each carrying committed `expected_go.txt` + `oracle.meta`
   (`main_sky_sha256` + `go_sky_version` + `exit_code` + `oracle_divergence`
   [+ `divergence_reason`]). The `oracle` crate (`tools/oracle/src/lib.rs`)
   read path `check_parity` **NEVER invokes Go**: it re-hashes `Main.sky`, hard-
   fails on a stale hash, and diffs skyc's stdout against the cached bytes.
   `tools/refresh-oracle` (re)captures the cache by running the **real Go `sky`**
   (`IPE_GO_ORACLE`, default `../sky/sky-out/sky`) **once, locally** — Go
   success → cache Go bytes; Go failure / non-zero → cache skyc bytes with
   `oracle_divergence=true`. Rigour invariants (match / stale / missing /
   divergence) are unit-pinned. **This is already option (a), battle-tested —
   but at golden granularity, not example granularity.**

2. **Examples-sweep EQUIVALENCE (ported but dormant).** A **separate** path:
   `equivalence_for()` builds the Go reference **live** via `build_go()` /
   `IPE_GO_BIN` and compares per shape (stdout diff / route-body diff via
   `equivalence_normalize_html.py` / boot floor / pty via `equivalence_tui_grid.py`). This
   is option (b), and it is **phased off** (`IPE_SWEEP_NO_EQUIV=1`) because the
   Haskell `sky` compiler is not present in this repo or in CI. **Examples
   carry NO oracle files today** (`find examples -name expected_go.txt` → none).

So: the cached-oracle model is *already the accepted answer for goldens*; A5's
undesigned piece is **extending that same model to the examples sweep**, so the
examples EQUIVALENCE path also needs no Go toolchain in CI.

---

## A4 — run the sweep on GitHub (BUILD+RUN, no EQUIVALENCE)

**Goal:** turn on the honest measurement. The first CI runs of
`examples-sweep.yml` execute BUILD+RUN over the 33 in-scope examples on
ubuntu + macOS, non-gating.

**What it proves (Rule 1):**

- Every green BUILD row = skyc emitted a crate that *actually cargo-builds* —
  no exit-0-then-cargo-fail on that example.
- Every green RUN row = the binary boots / serves / drives its shape headless
  without panic/hang.
- The `WARN_CELL` gate (cargo warnings that leak past the emitted
  `#![allow(unused, non_snake_case)]`) catches codegen defects even on a
  building crate.

**Which rows are RED today (expected, honest):** skyc currently implements only
`Sky.Core.*`. Every example reaching for `Std.Ui` / `Std.Html` / `Std.Css` /
`Std.Live` / `Std.Db` / server / tui / webview runtimes will `skyc-fail` (unbound
name at canon) or `cargo-fail` (emitted crate references an unbuilt kernel) until
the compiler reaches breadth. Concretely, expect RED across the Live/UI/TUI/
webview cluster (`09,10,12,16,17,19,22,24,25,26,27,28,29,31,34,37,38`, and the
server/live rows) and green only on the thin `Sky.Core`-only clis
(`01`, `14`, `20`, `simple`, and `00` build-wise). `26-ui-showcase` is a known
`skyc-fail` (no `sky.toml`, local `RegressionGates` import unresolved single-file).

**This is not a gate.** `continue-on-error: true` keeps the workflow green while
the table is red — the RED set *is* the compiler to-do list, regenerated every
push, and cross-referenced against the exit-0 registry pass and the
Std.Html/Css breadth work (#46/#47). **Flip `continue-on-error → false` only
after the first all-green BUILD+RUN sweep** (see DONE gate).

---

## A5 — establishing Go-parity on CI (the core design)

The constraint: **the Haskell `../sky` compiler (Go backend) is NOT present on
GitHub CI**, and we do not want to build it there. Three options:

| Option | Mechanism | Go/Haskell in CI? | Verdict |
|---|---|---|---|
| **(a) Vendored cached reference** | commit `expected_go.txt` per example, generated once locally via the real Sky-Go / an example-level `refresh-oracle`; sweep EQUIVALENCE diffs Rust output against it | **No** | **Recommended (gating path)** |
| (b) Live Go reference in CI | a dedicated job builds GHC/cabal `sky` + Go toolchain, generates references live, drops `IPE_SWEEP_NO_EQUIV` | Yes (heavy) | Rejected as the gate; kept as an audit |
| (c) Hybrid | (a) gates every PR; a nightly non-gating (b) job **re-refreshes/audits** the cache against a live Go build to catch drift | Nightly only | **Recommended overall** |

### Recommendation: (c) — cached-gating + nightly live-refresh audit

This is exactly the posture that already works for the 174 goldens, lifted to
examples. Rationale against the three lenses:

- **Reproducibility / hermeticity.** The gating path (a) is a pure function of
  committed bytes: `check_parity`-style diff, no network, no toolchain, no
  non-determinism. A PR's EQUIVALENCE result is identical on every runner and every
  replay. Option (b) as the gate couples ipê CI to upstream Haskell repo
  availability + GHC build flakiness + Go toolchain — a foreign failure domain
  on the critical path.
- **Security (guardian lens).** (a) builds **no untrusted external toolchain**
  in the PR path — smaller attack surface, no fetch-and-build of a second
  compiler on untrusted PR code. The one place Go *is* built (the local/nightly
  refresh) is trusted and out of the PR path.
- **Maintenance / freshness.** (a)'s one weakness is staleness — a cached
  reference can silently rot if the example changes. The **staleness gate**
  closes this exactly as the golden suite does: the oracle meta pins
  `sha256(<example source>)`; if the current source hash ≠ cached hash,
  `check_parity` **hard-fails "oracle stale — run refresh"** rather than
  diffing against a stale expectation. The nightly (c) audit job rebuilds the
  Go reference and re-runs refresh; any drift (upstream Go behaviour change,
  un-refreshed edit) surfaces as a nightly failure, off the PR critical path.

### How the cached model maps onto example shapes

Examples are not all stdout — the cache granularity follows `equivalence_mode`:

| Shape / mode | What is byte-comparable | Cache artifact | Normalization |
|---|---|---|---|
| `cli` → `stdout` | full program stdout | `expected_go.txt` (verbatim, as goldens) | determinism auto-probe (2-run diff); non-det → `.tsv` `none` |
| `server` → `body` | each comparable GET route's response body | `expected_go/<route>.html` | `equivalence_normalize_html.py` (strip sky-id / csrf / session / timestamps) |
| `tui` → `pty` | terminal cell grid snapshot | `expected_go/tui.grid` (optional) | `equivalence_tui_grid.py` (grid-normalize) |
| `live` → `scenario` | *behaviour*, not bytes (server-driven patches) | none — **boot/scenario floor only** | n/a (both-drive-runtime check) |
| `serve` | binds + serves | none — boot floor | n/a |
| `webview` / `fyne` / rust-ffi | no Go reference exists | none | `equivalence_mode none` |

The two Python normalizers are already ported verbatim and backend-agnostic —
they are the mechanism that lets "non-deterministic-but-equivalent" output
(sky-ids, CSRF tokens, timestamps, session cookies) compare equal. For pure-
stdout non-determinism that a normalizer can't reach (wall-clock, live HTTP,
Dict iteration order) the escape hatch is the `equivalence-classification.tsv` `none`
override — **already applied** to `02`, `07`, `35`, `skyshop-rs` — plus the
built-in 2-run determinism auto-probe that auto-reports `n/a` when Go's own
stdout is non-reproducible.

### The one new piece to build (later — not now)

An **example-level refresh** analogous to `tools/refresh-oracle`, differing from
the golden tool in three ways: (1) staleness key hashes the **whole example
source tree** (`**/*.sky` + `sky.toml`), not a single `Main.sky`; (2) it drives
`server`/`tui` shapes to capture normalized route-body / grid snapshots, not
only stdout; (3) it reuses the identical `oracle::Meta` format + divergence
tags (`sanctioned:` / `divergence:` / auto Go-failure) so the divergence policy
is one mechanism across goldens and examples. The sweep's `equivalence_for()` then
gains a cached-compare branch (diff Rust output vs the committed reference)
selected when `IPE_GO_BIN` is absent — mirroring `check_parity`. **Until that
lands the EQUIVALENCE column stays `—` (phase 1).** The port already keeps
`build_go()`/`equivalence_for()` intact so this is a wiring change, not a rebuild.

---

## The red-row TRIAGE → fix loop

Every non-green row is classified by its failing column and routed. The loop
iterates **through GitHub CI** — this dev box is disk-limited and cannot run the
full sweep (the harness itself hard-gates on `< 5 GB free`); per-fixture local
builds are fine, the full measurement is CI's job.

| Row state | Classification | Route |
|---|---|---|
| `skyc-fail` | compiler breadth gap (unbound `Std.*` at canon, unimplemented kernel) | exit-0 registry pass + `/implement-parity-gap`; Std.Html/Css breadth #46/#47 |
| `cargo-fail` | **codegen defect** — emitted Rust ill-typed / references an unbuilt kernel. Higher severity: this is the Rule-1 *exit-0-then-cargo-fail* class (soundness) | same pipeline, prioritized; guardian review of the emission |
| RUN `panic`/`hang` | runtime defect (reachable panic from well-typed code, non-termination) | file + fix; add regression |
| RUN `noserve`/`notty` | server didn't bind / no TTY — env or runtime defect | fix (or confirm harness-env, not a Rust defect) |
| EQUIVALENCE `DIFFER` | **a Go-parity correctness bug (Rule 2 / principle #2)** | **fix to byte-parity**; OR, iff Rust is deliberately more correct / Go is buggy, record a `sanctioned:` / `divergence:` marker (divergence-policy.md) + registry row — never silent |
| EQUIVALENCE `go-ref-broken` | **AMBER** — upstream Go reference doesn't build/serve (e.g. the v0.16.29 `undefined: T1` regression). NOT a Rust regression | leave amber; auto-clears when upstream fixes (finding #2 in the `.tsv`) |

`DIFFER` is the load-bearing case: the default response is *fix the parity bug*.
A divergence marker is allowed **only** when the difference is a genuine
correctness win (Rust more correct) or a Go bug — and then it is loud, reviewed,
source-pinned, and diffed exactly against Rust's own committed output. "If in
doubt, fix the parity bug instead."

Each iteration: push → CI sweep → read the table artifact → the RED set is the
next batch of compiler/runtime work → fix → repeat. The set shrinks
monotonically as breadth lands.

---

## The DONE gate

**VERDICT PASS iff no RED row** across the 33 non-Go-FFI examples, i.e. every
row satisfies:

```
BUILD == ok
AND  RUN   ∈ { ok, skip }
AND  EQUIVALENCE ∈ { equivalence-stdout, equivalence-body, equivalence-scenario, equivalence-serve,
               equivalence-pty, n/a, —, go-ref-broken(AMBER) }
```

Staged gating:

1. **A4 green** — first all-green **BUILD+RUN** sweep. Flip
   `examples-sweep.yml` `continue-on-error → false` for the BUILD+RUN columns:
   a RED build/run row now fails the workflow (matching upstream).
2. **A5 green** — vendor the cached example references, wire the `equivalence_for()`
   cached-compare branch, drop `IPE_SWEEP_NO_EQUIV`. EQUIVALENCE goes live and gates:
   a `DIFFER` fails CI; `go-ref-broken` stays AMBER.
3. **A6 — DONE** — push to `arthurmaciel/ipe-lang`; CI runs the full gating
   sweep; **green everywhere = the example-parity milestone is complete.** FFI
   examples (the 9 excluded dirs) are the following week's separate track.

---

## Gaps / OPEN DECISIONS (need a user call)

1. **Parity mechanism: confirm (c).** Recommended: cached references gate every
   PR (option a); a **nightly non-gating** job builds the Haskell `sky` + Go and
   re-refreshes to audit drift (option b). Alternative: pure (a) with a manual
   refresh discipline (no nightly Go build at all). *Decision: (c) vs pure-(a).*

2. **When to gate CI on VERDICT.** Recommended staged flip: BUILD+RUN gating
   after the first all-green BUILD+RUN sweep (step 1); EQUIVALENCE gating only after
   cached oracles are vendored and green (step 2). Alternative: keep
   `continue-on-error: true` until full parity, then flip once. *Decision:
   staged vs single flip.*

3. **Oracle-refresh policy + Go pin.** Which Sky-Go version/commit is the
   authoritative reference (pinned in `oracle.meta go_sky_version`), who runs
   the example refresh and on what trigger (example source change / Go pin bump),
   and the staleness-key granularity (recommend: sha256 over the full example
   source tree, hard-fail on mismatch as the golden suite does). *Decision:
   pin + trigger + who owns refresh.*

4. **Example oracle artifact location + tool reuse.** Where committed cached
   references live (recommend `examples/<name>/oracle/` beside the example, or a
   parallel `tests/example-golden/<name>/`) and whether to extend
   `tools/refresh-oracle` in place or add a sibling `refresh-example-oracle`
   that reuses the `oracle` crate's `Meta` + divergence machinery. *Decision:
   layout + one-tool-vs-two.*

5. **macOS matrix gating.** Whether both ubuntu **and** macOS gate the verdict,
   or ubuntu gates while macOS stays informational (macOS adapts tui via BSD
   `script`, webview via a real display — more environmental noise). *Decision:
   gate both vs ubuntu-gate + macOS-informational.*

6. **Server/live byte-parity depth.** For `body`/`scenario` shapes, how far to
   push byte comparison vs boot/behaviour floor — `live` is server-driven
   (SSE patches, session state) and may resist a stable cached snapshot even
   after HTML normalization. Recommend: cache normalized **initial GET** bodies
   for `server`/`live`, keep the scenario round-trip as a behaviour floor.
   *Decision: how much of the live surface to pin as bytes.*
