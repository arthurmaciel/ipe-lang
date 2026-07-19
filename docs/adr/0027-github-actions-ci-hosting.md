Status: Accepted
Date: 2026-06-29

# 0027. GitHub Actions CI and public-repo hosting

## Context

The project needs continuous integration for a Rust compiler + runtime that has heavy
build times (tokio, reqwest, RustCrypto, syn). Self-hosted runners (Forgejo/Woodpecker
on a local dev box) were the first candidate, but they have operational costs: if the
dev box is off, CI queues; disk hygiene is manual; ephemeral environments must be
maintained. The original plan had a split host (compute on local) vs build trigger (CI
provider) that added complexity.

## Decision

Host the repository publicly on GitHub. Use GitHub-hosted runners exclusively — no
self-hosted runners, no local CI agents.

Key properties the public-repo + GHA combination provides:
- **4-core / 16 GB ephemeral runners** per job; the matrix provides N in parallel.
- **No disk hygiene**: runners are ephemeral, so the ENOSPC / go-build-bloat class of
  dev-box problems does not exist in CI.
- **Warm builds** via `sccache` (GitHub Actions cache backend) + `Swatinem/rust-cache`.
- **Free for public repos**: zero cost.

Keep a Codeberg mirror (one extra git remote) for portability — git is portable; CI can
move later if values or cost demand it.

Parallel jobs: `fmt`, `clippy -D warnings`, `nextest` + doctests, `miri` (compiler
crates), `e2e` (sharded nextest + `IPE_E2E=1`). Supply-chain security lives in a
separate `.github/workflows/security.yml` (`cargo-audit` + `cargo-deny`), so it cannot
gate routine PRs but still runs nightly.

## Consequences

- Every push to `main`/`master` or PR triggers the full matrix; nightly cron covers dep
  and upstream drift.
- The concurrency group (`ci-${{ github.ref }}`) cancels superseded in-progress runs
  on the same ref; release/tag runs are never cancelled.
- Self-hosted runner tooling, Forgejo/Woodpecker config, and local systemd services are
  permanently off the critical path.
- The two-tier gate (PRINCIPLES.md §The two-tier gate) maps onto CI: cheap `cargo check`
  + scoped nextest per lane; the full workspace gate on the nightly + release path.
- Adding a new workflow job requires a separate `.yml` file to avoid bloating the
  main CI workflow with concerns that have different failure modes (e.g. static linking,
  WASM floor probe).
