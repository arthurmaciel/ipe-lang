---
name: examples-sweep
description: Run the ipê EXAMPLES sweep — the cornerstone correctness gate. ONE deterministic pass that BUILDS each in-scope example with `skyc build` + `cargo build`, RUNS it headless per shape, and (once a Go reference exists) asserts the Rust output matches Go per the example's equiv mode, emitting a per-example BUILD·RUN·EQUIV table. Use when the user asks to run the examples sweep, verify the vendored examples still build + run on skyc, or after a codegen/runtime change. First-iteration default is BUILD+RUN only (SKY_SWEEP_NO_EQUIV=1) — the Go≡Rust EQUIV column is phased in later. Trigger: /sky-compiler:examples-sweep.
---

# examples-sweep

The **cornerstone** of the ipê Rust-compiler dev cycle: one deterministic script —
`scripts/examples-sweep.sh` — that for every in-scope example (`build_set` from
`lib/examples.sh`: every candidate dir minus Go-FFI) does up to **three** things
and emits **one** table row:

| Column | What | Cells |
|---|---|---|
| **BUILD** | `skyc build <sky.toml\|src/Main.sky> --out sky-out/rust` + `cargo build --manifest-path sky-out/rust/Cargo.toml` | `ok` · `skyc-fail` · `cargo-fail` |
| **RUN** | run the emitted `sky-app` binary headless, per `example_shape` (via `exercise_*` in `lib/checks.sh`) | `ok` · `panic` · `hang` · `noserve` · `notty` · `skip` |
| **EQUIV** | build the Go reference + compare to Rust per the DERIVED equiv mode (**phased — off by default**) | `equiv-stdout` · `equiv-body N` · `equiv-serve` · `equiv-scenario` · `equiv-pty` · `n/a` · `DIFFER` · `go-ref-broken` · `—` |

A green Rust build is necessary but **NOT sufficient** — RUN catches the
runtime-regression class (panic / dead server / dead click). **Do NOT re-decide
the steps** — if a run reveals a better way, edit `examples-sweep.sh`,
`lib/checks.sh`, or the overrides manifest.

## skyc — how BUILD works here

Unlike the Haskell `sky` this was ported from, **`skyc` is Rust-only** — there is
NO `--backend rust` flag. `skyc build <entry>` emits a self-contained Cargo
project under `sky-out/rust/` (the runtime vendored into `src/sky_runtime`), whose
package/binary is `sky-app`. The sweep then runs `cargo build` on the emitted
`Cargo.toml`. The build target is `sky.toml` when present (multi-module project
discovery), else `src/Main.sky` (single-file). Verified against
`crates/skyc/src/lib.rs` `run_build`.

## Phased Go≡Rust parity

EQUIV needs a Go reference produced by the Haskell `sky` compiler, which this repo
does not ship. So the **first CI iteration runs BUILD+RUN only**
(`SKY_SWEEP_NO_EQUIV=1`). The EQUIV column, `build_go()`, `exercise_server_equiv`,
and the two Python normalisers are kept intact so parity flips on later — either by
putting a Haskell `sky` on PATH as `SKY_GO_BIN`, or by consuming vendored Go
reference outputs. See `docs/architecture/class2-tier1-sweep-fix-spec-2026-07-09.md` §1.

## Verdict

- **GREEN row** = BUILD `ok` AND RUN ∈ {`ok`, `skip`} AND EQUIV ∈ {`equiv-*`, `n/a`, `—`, `go-ref-broken`}.
- **RED row** = any `skyc-fail` / `cargo-fail` / `panic` / `hang` / `noserve` / `notty` / `DIFFER`.
- **AMBER** = `go-ref-broken` — the Go reference itself fails (upstream Go bug, not a Rust failure). Does NOT make the row red.
- **VERDICT PASS** iff no RED row AND no cargo warning leaks past the generated `#![allow]` (gated by `SKY_SWEEP_WARN_GATE`, default on; `=0` = report-only).

> **Expected first-run state.** skyc currently implements only `Sky.Core.*`;
> examples reaching for `Std.Ui` / `Std.Live` / `Std.Db` / server/tui runtimes will
> `skyc-fail` or `cargo-fail` until the compiler reaches parity. A largely-RED
> first sweep is HONEST, not a harness bug — the sweep exists to track that ramp.

## EQUIV modes — DERIVED from shape, overrides on top

`equiv_mode` (`lib/examples.sh`) derives the mode from `example_shape` so a new
example auto-classifies: cli→`stdout`, server→`body`, live→`scenario`, tui→`pty`,
webview/fyne/Go-FFI→`none`. `equiv-classification.tsv` is **overrides-only** (a
small file of exceptions + reasons) — a line wins over the derived mode.

## Flags

| Flag | Effect |
|---|---|
| `SKY_SWEEP_BUILD_ONLY=1` | BUILD column only (fast compile check; RUN + EQUIV = `—`). No `go`/`curl` needed. |
| `SKY_SWEEP_NO_EQUIV=1` | BUILD + RUN; EQUIV skipped (`—`). **The phase-1 default.** |
| `SKY_SWEEP_WARN_GATE=0` | downgrade the cargo-warning-past-`#![allow]` gate to report-only. |
| `SKY_SWEEP_NIGHT_GATE=1` | re-enable the OPT-IN local 22:00–08:00 BRT deferral window (off by default so CI never blocks). |
| `SKY_SWEEP_FORCE=1` | override the night gate. |
| `SKY_SWEEP_BUILD_TIMEOUT=N` | per-example skyc build ceiling (default 900 s — cold CI-safe). |
| `RUST_EXAMPLES="01-… 19-…"` | subset override (paths or basenames). |

## Preflight

The free-disk < 5G gate is a HARD abort (exit 2) — an ENOSPC mid-build leaves
corrupt artifacts. `mem-guard.sh`-not-running is a soft WARN only here (it never
blocks the sweep or CI).

## Workflow (every invocation)

1. **Build skyc once**, then run the sweep (phase-1 default = BUILD+RUN):
   ```bash
   cargo build --release -p skyc
   SKY_SWEEP_NO_EQUIV=1 bash scripts/examples-sweep.sh
   ```
   It self-resolves repo + env (`SKYC_BIN` from the cargo target dir), runs the
   sweep, and prints the table + summary + scoreboard path
   (`~/.cache/sky/examples-sweep/`).

2. **Relay the table + verdict** — quote the rendered BUILD·RUN·EQUIV table, the
   `N green · M red · K skipped · amber=A` summary, and the cargo-warning line.

3. **Triage RED rows.** A `skyc-fail`/`cargo-fail` is a real compiler/codegen gap
   (often a not-yet-ported `Std.*` module — file it against the parity backlog). A
   `panic`/`hang`/`noserve` is a real runtime regression. A `DIFFER` (once EQUIV is
   on) is a REAL Go≡Rust divergence to root-cause, not paper over. `go-ref-broken`
   is AMBER — an upstream Go bug.

## Shared `lib/`

`lib/env.sh` (REPO + `SKYC_BIN` + shared `CARGO_TARGET_DIR` + sccache),
`lib/examples.sh` (the Go-FFI exclusion / `build_set` / `equiv_mode` — SSOT),
`lib/checks.sh` (per-shape `exercise_*` + `PANIC_RE` + browser-stack probe), and
the two `equiv_*.py` normalisers are the SINGLE SOURCE OF TRUTH. RUN exercises the
Rust binary; EQUIV (phased) exercises BOTH backends and compares.

## Baked-in gotchas

- `SKYC_BIN` resolves from the **shared** `CARGO_TARGET_DIR`
  (`~/.cache/sky-rust-target`, pinned in this repo's `~/.cargo/config.toml`) —
  `target/release/skyc` first, then `target/debug/skyc`, then `$REPO/target`, then PATH.
- Go-FFI examples are ABSENT from `build_set` (imports of Go-package modules) and
  were never vendored — see `docs/architecture/class2-tier1-sweep-fix-spec-2026-07-09.md` §1 for the vendored-example state (the original copied-vs-excluded list is preserved in git history).
- `26-ui-showcase` has no `sky.toml` → single-file build target; it imports a local
  module, so it surfaces a real `skyc-fail` until a manifest is added upstream.
- Never edit runtime / crate files while this runs (concurrent copy → false build error).
