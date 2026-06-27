# CI & hosting plan

> **Status:** decision record. Written 2026-06-27.
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
