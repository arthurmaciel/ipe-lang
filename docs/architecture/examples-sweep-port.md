# Examples-sweep port — from `../sky` to this repo

This note records the port of the Sky **examples-sweep** harness (the cornerstone
BUILD·RUN·EQUIV correctness gate) from the Haskell-compiler repo
(`../sky/runtime-rust/scripts/`) into this ipê Rust-compiler repo. Scope of the
port was **docs / scripts / YAML + example vendoring only** — no compiler, runtime,
or stdlib code was touched.

## What was ported

| Source (`../sky`) | Here | Change |
|---|---|---|
| `runtime-rust/scripts/examples-sweep.sh` | `scripts/examples-sweep.sh` | adapted BUILD step to drive `skyc`; phased-off EQUIV; night gate opt-in |
| `runtime-rust/scripts/lib/env.sh` | `scripts/lib/env.sh` | `SKY_BIN`→`SKYC_BIN` (cargo target dir); dropped GHC/cabal; kept shared `CARGO_TARGET_DIR` + sccache |
| `runtime-rust/scripts/lib/examples.sh` | `scripts/lib/examples.sh` | Go-FFI exclusion logic **verbatim**; stdlib-index scan repointed to `crates/skyc/stdlib`; trimmed skydex-only `changed_examples` helpers |
| `runtime-rust/scripts/lib/checks.sh` | `scripts/lib/checks.sh` | `night_guard` made opt-in; browser-driver paths repointed; `resolve_bin` targets skyc's `sky-app` |
| `runtime-rust/scripts/lib/equiv_normalize_html.py` | `scripts/lib/equiv_normalize_html.py` | **verbatim** (backend-agnostic) |
| `runtime-rust/scripts/lib/equiv_tui_grid.py` | `scripts/lib/equiv_tui_grid.py` | **verbatim** (backend-agnostic) |
| `runtime-rust/scripts/equiv-classification.tsv` | `scripts/equiv-classification.tsv` | **verbatim** (overrides-only manifest) |
| `plugins/sky-rust-backend/skills/examples-sweep/SKILL.md` | `plugins/sky-compiler/skills/examples-sweep/SKILL.md` | rewrote paths + skyc invocation + phased-parity section |
| `.github/workflows/examples-sweep.yml` | `.github/workflows/examples-sweep.yml` | ubuntu+macOS matrix; cargo-build skyc; BUILD+RUN sweep; artifact + summary; phase-2 EQUIV stub commented |

## Key adaptation — `skyc`, not the Haskell `sky`

The compiler here is **`skyc`**, a Rust cargo workspace, Rust-only. It has **no
`--backend rust` flag** (the upstream sweep drove `sky build … --backend rust`).

The BUILD step invokes, verified against `crates/skyc/src/lib.rs` `run_build`
(usage string: `skyc build <entry.sky|project-dir|sky.toml> [--out <dir>]
[--runtime <dir>] [--emit-ir] [--fix]`):

```bash
( cd <example> && skyc build <sky.toml | src/Main.sky> --out sky-out/rust )
cargo build --manifest-path <example>/sky-out/rust/Cargo.toml
```

Facts confirmed from `crates/skyc/src/lib.rs` (`run_build` / `build` /
`build_project`) and its E2E test:

- `skyc build` emits a **self-contained Cargo project** under `sky-out/rust/`
  (default `--out`), with the runtime **vendored into `src/sky_runtime`** — so the
  emitted crate needs no external runtime path at `cargo build` time.
- The emitted package/binary is **`sky-app`** (E2E test runs
  `sky-out/…/target/debug/sky-app`). `resolve_bin` in `lib/checks.sh` looks for
  `sky-app` in the shared cargo target dir.
- `--runtime` is optional: skyc auto-resolves `<repo>/runtime/src/sky_runtime`
  (`resolve_runtime()`). `env.sh` exports `SKY_RUNTIME_DIR` as a belt-and-braces;
  the sweep leaves the flag off and relies on auto-resolve.
- Build target selection: **`sky.toml` when present** (multi-module discovery via
  `build_project`), else **`src/Main.sky`** (single-file `build`).

`SKYC_BIN` resolution (`env.sh`) probes, in order: `$CARGO_TARGET_DIR/release/skyc`,
`$CARGO_TARGET_DIR/debug/skyc`, `$REPO/target/{release,debug}/skyc`, then PATH.
This matters because this repo's `~/.cargo/config.toml` pins a **global**
`target-dir = ~/.cache/sky-rust-target`, so a bare `cargo build -p skyc` lands the
binary there, not in `$REPO/target/`.

## Night gate

Upstream gated this heavy sweep to 22:00–08:00 America/Sao_Paulo and had CI bypass
it with `SKY_SWEEP_FORCE=1`. Here `night_guard` is **opt-in**
(`SKY_SWEEP_NIGHT_GATE=1`) and a **no-op by default**, so it can never block GitHub
CI. The function is preserved for local low-load-window use; `SKY_SWEEP_FORCE=1`
still overrides it.

## Go-FFI exclusion set — copied vs excluded

The exclusion rule is `lib/examples.sh`'s `is_out_of_scope`, ported **verbatim**:
an example is excluded **iff** a `.sky` file imports a **Go-package module** — a
dotted `import` that resolves to neither a Sky-stdlib module (`Sky.*` / `Std.*` /
bare stdlib names) nor a local project `.sky`. `[go.dependencies]` is NOT consulted
(it over-excludes stdlib-transitive deps). The set below was computed by running
the source repo's own `build_set` / `is_out_of_scope` over `../sky/examples`.

**Vendored (in scope — 33 dirs):** `00-standard-libs`, `01-hello-world`,
`02-go-stdlib`, `04-local-pkg`, `06-json`, `09-live-counter`, `10-live-component`,
`12-skyvote`, `14-task-demo`, `15-http-server`, `16-skychess`, `17-skymon`,
`18-job-queue`, `19-skyforum`, `20-cli-counter`, `21-tui-stopwatch`,
`22-tui-stopwatch-ui`, `23-tui-todo`, `24-tui-kitchen-sink`, `25-sky-console`,
`26-ui-showcase`, `27-multi-session-chat`, `28-streaming-chat`,
`29-webview-threejs-spike`, `30-sse-server-demo`, `31-webview-stopwatch-ui`,
`32-sse-relay`, `33-websocket-echo`, `34-multi-tier-console`,
`37-composite-live-shop`, `38-composite-ui-multibackend`, `simple`, `test_pkg`.

**Excluded (Go-FFI — NOT vendored — 9 dirs):**

| Dir | Shape | Why excluded (Go-package import) |
|---|---|---|
| `03-tea-external` | cli | imports an external Go-package module |
| `05-mux-server` | cli | Go `net/http` mux (Go-package import) |
| `07-todo-cli` | cli | Go-package import outside the stdlib |
| `08-notes-app` | server | Go-package import (hidden in a Lib submodule) |
| `11-fyne-stopwatch` | fyne | `Fyne.…` Go GUI toolkit |
| `13-skyshop` | live | Stripe/Go-SDK-scale FFI (`Github.Com.…`) |
| `35-composite-generics` | cli | Go-package import |
| `36-composite-server` | server | Go-package import |
| `rust/skyshop-rs` | live | special-cased out (heavyweight Rust-FFI proof; verified separately) |

Notes:

- `02-go-stdlib` is **in scope** despite the name: its imports resolve to
  `Sky.Core.Http`/`Time`/`Crypto` (stdlib-transitive), not Go-package modules, so
  it builds on the Rust backend. It carries an equiv override to `none`
  (non-deterministic wall-clock + live HTTP).
- The gitignored `examples/26-ui-showcase/.sky/console-token` secret dir was
  **stripped** from the vendored copy.

## Phased Go≡Rust parity plan

EQUIV compares Rust output against a **Go reference** produced by the Haskell `sky`
compiler — which this repo does not ship. Therefore:

1. **Phase 1 (now):** BUILD + RUN only (`SKY_SWEEP_NO_EQUIV=1`). The EQUIV column
   renders `—`. The `examples-sweep.yml` job sets this env.
2. **Phase 2 (later):** turn EQUIV on. Two supported paths, both stubbed in the
   commented `examples-sweep-equiv` job at the bottom of `examples-sweep.yml`:
   - **(a) Live Go reference** — build the Haskell `sky` in the job, put it on PATH
     as `SKY_GO_BIN` (the sweep's `build_go()` already honours it), add `setup-go`
     for the reference `go build`, and drop `SKY_SWEEP_NO_EQUIV`.
   - **(b) Cached Go reference** — vendor `expected_go.txt`-style outputs per
     example (à la `e2e-and-oracle-caching.md`) and extend `equiv_for()` to diff
     against the cached reference, needing no Go toolchain on CI.

The whole EQUIV machinery (`equiv_for`, `exercise_server_equiv`, `build_go`, the
two Python normalisers, `equiv-classification.tsv`) is ported intact so phase 2 is
a wiring change, not a rebuild.

## Gating posture (expected first-run state)

skyc currently implements only `Sky.Core.*`. Examples reaching for `Std.Ui` /
`Std.Live` / `Std.Db` / server / tui / webview runtimes will `skyc-fail` or
`cargo-fail` until the compiler reaches parity — so a **largely-RED first sweep is
the honest state**, not a harness bug. Accordingly the `examples-sweep.yml` job
runs `continue-on-error: true` (informational: prints the table, uploads the
artifact, surfaces the verdict, but does not fail the workflow). **Flip
`continue-on-error` to false once skyc reaches example parity** so a RED row gates
CI, matching upstream.

## `ci.yml` — deliberately untouched

The source `../sky/.github/workflows/ci.yml` is a **Haskell (cabal) + Go** pipeline
(build `exe:sky`, cabal test, Go example sweep, console drift check). It has **no
Rust `fmt`/`clippy`/`test` jobs to port**. This repo's existing
`.github/workflows/ci.yml` already provides superior Rust gates — `fmt`, `clippy
-D warnings`, `nextest` + doctests, `miri`, and a sharded `e2e`. Merging the
Haskell pipeline in would add nothing and risk clobbering good jobs, so `ci.yml`
was left **unchanged**. The examples-sweep lives in its own workflow.

## `TODO(verify)` — open items for skyc-flag / behaviour uncertainty

- **`TODO(verify)` — `26-ui-showcase` has no `sky.toml`** but is multi-module (its
  `Main.sky` imports a local `RegressionGates`). The sweep falls back to
  `skyc build src/Main.sky` (single-file), which cannot resolve the local import →
  a genuine `skyc-fail` (SKY-N0020) until a `sky.toml` is added upstream OR skyc's
  single-file build learns sibling-module discovery. Vendored verbatim (no manifest
  synthesised) per the "copy dirs verbatim" constraint.
- **`TODO(verify)` — `--out sky-out/rust` is passed explicitly** even though it is
  skyc's default, so the sweep is robust if the default ever changes. Confirm this
  stays the emitted layout (`sky-out/rust/Cargo.toml`, binary `sky-app`).
- **`TODO(verify)` — `cargo build --manifest-path sky-out/rust/Cargo.toml`** assumes
  the emitted crate is self-contained (runtime vendored). Confirmed from `build()`
  in `lib.rs` (it `copy_dir(runtime_dir, …/sky_runtime)`), but re-verify if skyc
  moves to a path/workspace-dependency model for the runtime.
- **Pre-warm step** uses `cargo build -p sky-runtime-rust` (the runtime crate's
  package name, confirmed from `runtime/Cargo.toml`) with a `|| cargo build
  --workspace || true` fallback so the warm-up populates the dep cache the emitted
  crates reuse. Note: the emitted crate **vendors** the runtime source rather than
  depending on this crate by path, so the warm-up helps mainly by pre-compiling the
  shared third-party deps (tokio/axum/serde/…) into the shared target dir.
