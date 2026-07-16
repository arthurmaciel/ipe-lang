# Repo layout restructure — spec

> **Status: Accepted (design) — not implemented.** Precondition: sweep is a real
> 36/36 (green tree) AND no agent/autopilot is mid-run. Becomes an ADR once
> landed. Pairs with `docs/rename/` (Sky→Ipê) and
> `namespace-imports-and-packaging-spec.md` (Ipe. namespace) — this is **Step A**
> of that combined endgame.

## Why one campaign, three steps

The layout move, the Sky→Ipê rename, and the `Sky.Core`+`Std`→`Ipe.` flatten
all rewrite the same files. Run as ONE campaign, sequenced so the tree stays
green between steps (each ends on a full §6 gate + example sweep):

- **Step A — pure relocation (THIS spec).** `git mv` only; NO renames, NO import
  changes. Rewire Cargo + scripts to the new paths. Fully bisectable; no semantic
  change. Land first.
- **Step B — rename** (`docs/rename/`): `sky_*`→`ipe_*`, `SKY-`→`IPE-`, `SKY_`→
  `IPE_`, `.sky`→`.ipe`, etc. Golden regen.
- **Step C — namespace flatten** (`namespace-imports-and-packaging-spec.md`):
  merge the relocated `Sky/Core`+`Std` stdlib into `Ipe/`, rewrite imports.

Rationale: A is safe pure-motion; doing renames/flatten before the move would
churn paths twice. Keeping `Sky/Core`+`Std` subdirs intact through Step A means
no import touches until Step C.

## Target tree (Step A end-state)

```
src/
  ipe-cli/        <- crates/skyc/{Cargo.toml, src, tests}   (the CLI/driver; binary `ipe`)
  compiler/       <- crates/* EXCEPT skyc  (sky_parse, sky_canon, sky_syntax, sky_types,
                     sky_lower, sky_ir, sky_backend, sky_backend_rust, sky_kernels,
                     sky_diagnostics, sky_db, sky_intern, sky_watch)
  runtime/rust/   <- runtime/*
  stdlib/         <- crates/skyc/stdlib/*  (KEEP Sky/Core + Std subdirs in Step A;
                     Step C merges them into stdlib/Ipe/)
```
`crates/` and `runtime/` are removed after their contents move.

### Naming decisions (confirmed 2026-07-16)
- **`src/ipe-cli`** (was `src/app`) — explicit + unequivocal; it is the `ipe`
  CLI binary.
- **`src/runtime/rust/`** — the `/rust/` level is intentional: other backends +
  runtimes are planned (e.g. wasm), so `src/runtime/<target>/` is the shape.

## `crates/skyc/tests/` — moves with `src/ipe-cli/`

skyc's tests (golden_*, server_e2e, watch_*, tui/webview_e2e, msg_admissibility,
stdlib-seal…) drive the `skyc` binary end-to-end → they are the app's integration
tests → move wholesale to `src/ipe-cli/tests/`. After the move, reassess whether any
pure compiler-unit test belongs under `src/compiler/<crate>/tests/` instead.

## Cargo + path rewiring (Step A)

1. **Workspace root `Cargo.toml`** — update `members`/`exclude` to
   `src/ipe-cli`, `src/compiler/*`, `src/runtime/rust`, plus `tools/*` (unchanged).
2. **Inter-crate deps** — every `path = "../<crate>"` in all Cargo.tomls
   re-anchored to the new relative layout (compiler crates are now siblings under
   `src/compiler/`; app depends on `../compiler/<crate>` and `../runtime/rust`;
   runtime dep path updated).
3. **Package names unchanged in Step A** (still `sky_*`, `sky-runtime-rust`,
   `skyc`) — renames are Step B. Only *paths* move here.
4. **Stdlib embed path** — skyc locates its stdlib (currently
   `crates/skyc/stdlib`); update the resolver to `src/stdlib` (const/env in
   `src/ipe-cli/src/…`; check `stdlib.rs`).
5. **`SKY_RUNTIME_DIR`** — default resolution + `scripts/lib/env.sh` +
   `skyc-runtime-dir` memory move from `runtime/src/sky_runtime` to
   `src/runtime/rust/src/sky_runtime`.

## Scripts + CI rewire (Step A)

- `scripts/lib/env.sh` — `SKYC_BIN` candidate paths, `SKY_RUNTIME_DIR`,
  `REPO`-relative anchors.
- `scripts/examples-sweep.sh` + `scripts/lib/examples.sh` — any `crates/`/
  `runtime/` path assumptions.
- Golden tests — path helpers that reach `crates/…`/`runtime/…` (e.g.
  `support::repo_root()` joins).
- `.github/workflows/*` — build/test paths, `runtime-full-features` job.
- `tools/*` (ipe-index, refresh-oracle, oracle, parity-matrix) — any indexed
  roots pointing at `crates/`/`runtime/`.
- CLAUDE.md / DEVELOPMENT.md §0b infra reference — path updates.

## Stdlib flatten (deferred to Step C — recorded here)

Verified **collision-free**: Core's 23 modules {Basics, Bytes, Char, Crypto,
Dict, File, Http, Io, List, Math, Maybe, Path, Pure, Random, Regex, Result, Set,
String, System, Task, Time, ToString, WebSocket} and Std's 12 {Cache,
Compression, Config, Css, Csv, Email, Live, Money, Palette, PubSub, Trace, Ui}
are disjoint → merging into `Ipe/` yields no name clash. Nested modules
(`Core.Http` vs `Http.Server`; `Std.Live.*`, `Std.Ui.*`) nest, not clash. Step C
owns the import rewrite + the `Ipe.` prefix decisions from the namespace spec.

## Verification — tiered per-step gate (NOT the full §6 gate ×3)

`git mv` preserves history. `cargo check` + `clippy` alone are NOT sufficient
between steps: they prove the COMPILER compiles, not that it RESOLVES
paths/stdlib/runtime and EMITS working code — exactly what these steps threaten,
and those failures are invisible to `check` (which also links no `skyc` binary to
run the sweep). Match the gate to what each step can break:

- **Fast pre-filter (every step):** `cargo build --workspace` (produces the
  `skyc`/`ipe` binary) + `cargo clippy --workspace`. Seconds; catches
  compile/lint breaks before the expensive run.
- **Load-bearing (every step):** `cargo nextest run --workspace` (golden + E2E +
  skyc-resolution tests — the layer a move/rename/flatten actually breaks) + the
  **example sweep** (the ultimate skyc-driven check). Steps B + C regenerate
  goldens via `refresh-oracle` first.
- **Deferrable to the END:** `cargo test --doc --workspace` + `cargo nextest run
  -p <runtime> --features full` + `clippy --workspace --all-targets -D warnings`
  — EXCEPT Step B (rename) touches runtime identifiers + feature-gated code, so
  run `--features full` there too.
- **End of all three:** the complete §6 gate + example sweep 36/36 +
  cross-platform CI.

A red that appears is a rewiring/rename/import miss, not a logic regression → fix
the path/name/import, never the code (§0).

## Rollback

Tag `pre-restructure <HEAD>` before Step A. Each step's red gate →
`git reset --hard` that step. `git mv`-only Step A is trivially revertible.

## Execution note

Do NOT start while agents/autopilot run (this moves the crates/dirs they touch)
or while the tree is red (can't distinguish rewiring misses from the open sweep
reds #217/#218). This is the pre-push endgame, on green, single-writer.
