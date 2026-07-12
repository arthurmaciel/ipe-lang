# CI & hosting plan

> **Status:** decision record. Written 2026-06-27; **PIVOTED 2026-06-29 → GitHub
> Actions, public repo** (§0 is the whole current plan).

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
3.5. **Supply-chain security is a separate workflow** (`.github/workflows/
   security.yml`, added 2026-07-12): `cargo-audit` (RustSec advisory scan
   against `Cargo.lock`, config at `.cargo/audit.toml`) and `cargo-deny`
   (license compliance + dependency-source pinning + duplicate-version
   report, config at `deny.toml`, scanned against the `--features full`
   graph to match `runtime-full-features`). Both are lockfile/metadata-only
   — no compilation, seconds to run — and both **gate** (fail the workflow
   on a real finding), not advisory-only: PRINCIPLES.md ranks Security
   ahead of Correctness, and both tools support a documented, justified
   ignore-list for the unfixable case (see the one current entry,
   RUSTSEC-2023-0071, in `.cargo/audit.toml`), so gating doesn't risk a
   permanently-red CI over an advisory with no available fix. Miri stays in
   `ci.yml` (it's about undefined behaviour in code we write, not
   third-party dependency metadata) and deliberately stays scoped to the
   handful of crates where it's load-bearing (`sky_intern`, `sky_ir`,
   `sky_types`, `sky_lower`, `sky_diagnostics`, `sky_backend_rust` — all
   `#![forbid(unsafe_code)]`, so Miri's value there is catching UB
   triggered indirectly through a dependency's own `unsafe`, plus Stacked/
   Tree-Borrows aliasing violations in code using `Rc`/`RefCell`-style
   interior mutability, not scanning for a nonexistent `unsafe` block of
   ours). `sky_db` (the salsa incremental-compilation database) was
   evaluated and rejected as a Miri target: a scoped
   `cargo +nightly miri test -p sky_db` run timed out after 280s on the
   FIRST of 3 tests (salsa's internal machinery — thread pools / generation
   counters — makes Miri's interpretation overhead impractical for that
   crate), confirming the existing narrow scope is deliberate, not an
   oversight.
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

> The original pre-pivot plan (Codeberg + hosted Woodpecker light gates +
> self-hosted local heavy runner, driven by the 1 GB OCI-micro constraint) was
> superseded in full by the §0 pivot; it is preserved in git history.
