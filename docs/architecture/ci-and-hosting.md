# CI & hosting plan

> **Status:** decision record. Written 2026-06-27; **PIVOTED 2026-06-29 → GitHub
> Actions, public repo** (see §0). Everything below §0 (Codeberg + hosted
> Woodpecker + self-hosted local heavy runner) is **SUPERSEDED**, kept for history.

## 0. Pivot (2026-06-29): public repo on GitHub, all CI on GitHub Actions

The repo is **public**, so GitHub-hosted runners are free and generous. That
dissolves the original constraint (no affordable cloud build box → split host
from build compute → self-host the heavy runner locally). GitHub gives us, with
**zero local setup or maintenance**:

- **4-core / 16 GB ephemeral runners** — one runner ≈ a decent build box, and the
  **matrix gives N of them in parallel** (the E2E sweep shards across 4 runners).
- **No disk hygiene** — runners are ephemeral, so the ENOSPC/go-build-bloat class
  simply doesn't exist in CI.
- **Warm builds** via `sccache` (GitHub Actions cache backend) + `Swatinem/rust-cache`.

This **replaces the self-hosted local heavy runner** entirely — no Forgejo/
Woodpecker agent, no systemd service, no "if the dev box is off, CI queues."

**Decisions:**
1. **Host + CI: GitHub** (public). Workflow: `.github/workflows/ci.yml`.
2. **Keep a Codeberg mirror** (one extra git remote) so we're never locked in —
   git is portable; we can move CI later if values/independence demand it.
3. **Jobs** (parallel): `fmt`, `clippy -D`, `test` (nextest + doctests), `miri`
   (compiler crates), `e2e` (sharded `nextest --partition`, `SKY_E2E=1`).
4. **Parity needs no Go toolchain in CI** — it rides the **cached Go oracle**
   (committed `expected_go.txt`; see `e2e-and-oracle-caching.md`). A Go panic is
   recorded as an `oracle_divergence`, never cached as "correct" (the Go oracle
   can panic / fail to produce a reference on a shape — see
   `repo-layout-and-mirroring.md`).
5. **E2E needs the Sky runtime.** Until the repo reorg vendors it
   (`vendor/upstream-sky`, see `repo-layout-and-mirroring.md`), the `e2e` shards
   **self-skip** (`::notice::`) and CI stays green; they auto-activate once the
   runtime is vendored as a submodule (`actions/checkout … submodules: recursive`).
6. **Issue-tracker / explain-page report URL** now points at the GitHub repo
   (resolves the `sky_diagnostics::ISSUE_TRACKER_URL` "OWNER" placeholder once the
   slug is known).
7. **Triggers:** push (default branch) + pull_request + nightly cron + manual
   `workflow_dispatch`. `concurrency` cancels superseded in-progress runs.

**Why this is correct, not a compromise:** the original split existed only because
no affordable cloud box could build our Rust CI. A public GitHub repo removes that
constraint outright — more compute, parallel, managed, ephemeral, free. The local
machine goes back to being just the dev box (its fast inner loop sped by the
shared cargo target + sccache + nextest); the comprehensive net moves to CI.

---

> _Below: original (SUPERSEDED) plan — Codeberg + hosted Woodpecker + self-hosted
> local heavy runner. Retained for history only._

> **Constraint that drove this:** OCI Always-Free **Ampere A1 is not practically
> obtainable** (perpetual "out of capacity"); only the **AMD `E2.1.Micro`
> (1 OCPU / 1 GB RAM)** is available, and **1 GB cannot build our Rust CI** (the
> emitted projects pull `tokio`/`rsa`/`aes-gcm`/`serde`-derive, whose link steps
> need 2–4+ GB). So **host** and **build compute** are split.

## Topology

| Role | Where | Why |
|---|---|---|
| Git host | **Codeberg** (hosted Forgejo) | free, more capable than the micro, community visibility + backups, zero ops |
| Light CI — `fmt --check`, `clippy -D warnings`, unit tests, Miri (compiler crates only) | **Codeberg-hosted Woodpecker** (`ci.codeberg.org`, opt-in) | fast feedback even when the local machine is off; light enough for fair-use |
| Heavy CI — example sweep, E2E build+run, **behavioural parity** (vs the Go `sky` oracle), perf-sweep | **self-hosted runner on the local dev machine** | only box with the RAM *and* the Go/`sky` oracle present |

The OCI `E2.1.Micro` is **not used as a build runner**. If self-hosted git is ever
wanted for ownership it can host Forgejo alone (SQLite + ~2 GB swap, small repo),
but it adds nothing over Codeberg for our needs — default is Codeberg.

## Local heavy runner — triggers (REQUIRED)

The local runner's heavy workflow MUST fire on **both**:

1. **push** — to the default branch and on PRs (catch regressions per change).
2. **nightly schedule** — a cron run (full sweep + parity + perf), so the
   expensive checks happen even on quiet days and catch upstream/dep drift.

### Forgejo Actions workflow (GitHub-Actions-compatible YAML)

`.forgejo/workflows/heavy.yml` — runs on the self-hosted runner label:

```yaml
name: heavy
on:
  push:
    branches: [main]
  pull_request:
  schedule:
    - cron: '0 6 * * *'   # 06:00 UTC ≈ 03:00 America/Sao_Paulo (nightly)
  workflow_dispatch: {}    # manual button too

jobs:
  sweep-and-parity:
    runs-on: [self-hosted, heavy]   # label set when registering the local runner
    steps:
      - uses: actions/checkout@v4
        with: { submodules: recursive }   # vendor/upstream-sky (Go `sky` oracle + runtime-go)
      - name: build + test + clippy + fmt
        run: |
          cargo fmt --all -- --check
          cargo clippy --all-targets -- -D warnings
          cargo build
          cargo test
      - name: miri (mutation-heavy crates)
        run: cargo +nightly miri test -p sky_intern -p sky_ir -p sky_types -p sky_lower -p sky_diagnostics
      - name: example sweep (Rust backend, byte/behaviour)
        run: ./scripts/example-sweep.sh        # to be authored in Phase F
      - name: behavioural parity (sky[Go] vs skyc[Rust])
        run: ./scripts/parity-sweep.sh         # needs the Go `sky` oracle on PATH
      - name: perf sweep
        run: ./scripts/perf-sweep.sh
```

> Forgejo Actions YAML ≈ GitHub Actions YAML, so the same file works at
> `.github/workflows/heavy.yml` if the repo is ever mirrored to GitHub. Codeberg
> cron is **UTC** — pick the cron with the São Paulo offset in mind.

### Light gates (hosted Woodpecker), `.woodpecker.yml`

```yaml
when:
  - event: [push, pull_request]
steps:
  check:
    image: rust:latest
    commands:
      - rustup component add clippy rustfmt
      - cargo fmt --all -- --check
      - cargo clippy --all-targets -- -D warnings
      - cargo test
```

(Miri needs nightly; either add a nightly toolchain step here or leave Miri to the
local heavy runner — the heavy workflow already runs it.)

## Registering the local runner

1. Install **Forgejo Runner** on the local machine (`forgejo-runner`), or a
   **Woodpecker agent** if standardising on Woodpecker.
2. Register it against the Codeberg repo/org with the label **`heavy`** (and
   `self-hosted`), using a registration token from the repo's Actions settings.
3. Run it as a service (systemd/launchd) so push-triggered jobs fire without
   manual start; the nightly cron fires regardless as long as the runner is up.
4. Cache discipline (the build host is your dev box): shared `target/` or
   `sccache`, and prune the go-build/cargo caches per the CLAUDE hygiene rules so
   a sweep doesn't fill the disk.

## Dependencies the heavy runner needs locally

- Rust stable + nightly (Miri), Go toolchain.
- The **Go `sky` oracle binary** for behavioural parity (from `vendor/upstream-sky`
  pinned tag, or a released `sky`). This is *the* reason parity can't run on a
  free cloud micro.
- Submodule `vendor/upstream-sky` checked out (oracle + `runtime-go` mirror source).

## Caveats

- OCI free Ampere unavailable; the micro can't build Rust CI — settled above.
- If the local machine is off, push-triggered heavy CI simply queues until the
  runner is back; the hosted light gates still run, so PRs get *some* signal.
- Behavioural parity needs an executable Go `sky` oracle on the runner's
  platform — already satisfied on the local x86 dev box.

## One-line summary

Code on Codeberg; light gates on hosted Woodpecker; **all heavy work (sweep, E2E,
parity, perf) on a self-hosted Forgejo/Woodpecker runner on the local machine,
triggered on push AND nightly cron**; the free OCI micro is not a build runner.
