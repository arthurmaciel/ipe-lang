# Contributing to Ipê

We really appreciate your contribution! Ipê is a principled, security-first
compiler — every change is weighed against `PRINCIPLES.md` (the single source of
truth for the enforced rules) and the contributor map in `AGENTS.md`. Please read
both before opening a pull request, then follow the steps below:

1. **Clone the repo**

   ```bash
   git clone https://github.com/ipe-lang/compiler.git
   cd ipe-lang
   ```

2. **Create a branch** off `main` (one unit of work per PR)

   ```bash
   git switch -c <new-branch-name>
   ```

3. **Edit the code.**

4. **Format & lint** — the deny-set is the SSOT in the root `Cargo.toml`; never
   relax it on the command line.

   ```bash
   cargo fmt --all -- --check
   cargo clippy --all-targets --workspace -- -D warnings
   ```

5. **Panic-scan** (soundness — bans `unwrap`/`expect`/`panic!`/`assert!` in
   production; it takes a FILE LIST, so no args = false pass).

   ```bash
   cargo build --release --manifest-path tools/panic-scan/Cargo.toml
   find src -name '*.rs' -not -path '*/tests/*' -not -path '*/templates/*' -print0 \
     | xargs -0 target/release/panic-scan
   ```

6. **Test — including the SEAL** (ipe-accepts ⇒ cargo-builds). Add `-p <crate>`
   for each crate you touched; `--profile ci` gives the 600s timeout slow emit
   tests need; `IPE_E2E=1` builds and runs the emitted project.

   ```bash
   cargo nextest run --profile ci -p ipe          # + -p <crate> per crate changed
   IPE_E2E=1 cargo nextest run --profile ci -p ipe
   ```

7. **Regenerate derived artifacts if you changed their source** — goldens are
   byte-exact emitted Rust (never hand-edit); `cargo deny` guards dependency
   changes. After running, `git diff` must be clean.

   ```bash
   cargo run -p regen-goldens        # only if you changed emit
   cargo deny check                  # only if you changed Cargo.toml / Cargo.lock
   ```

8. **Stage your changes**

   ```bash
   git add .
   ```

9. **Commit**

   ```bash
   git commit -m "<your commit message>"
   ```

   Use [Conventional Commits](https://www.conventionalcommits.org/) (`feat:`,
   `fix:`, `docs:`, …) — versions and `CHANGELOG.md` are release-please automated
   from them, so never bump a version by hand.

10. **Push your branch**

    ```bash
    git push -u origin <new-branch-name>
    ```

11. **Open the pull request** against `main`

    ```bash
    gh pr create --base main --fill
    ```

## CI phases

Every CI job runs in exactly one phase, so no check runs twice for one change:

| Phase | Runs on | What runs |
|-------|---------|-----------|
| **cheap** | pull request, merge queue | format, lint, quick-check, panic-scan, the lock/artifact/ruleset guards, the docs-drift and example gates, the first-party floors, the Windows/wasm/editor builds, and the security, grammar and manifest checks |
| **tests** | merge queue | nextest, the SEAL e2e shards and their coverage proof, the SEAL smoke and vendored-emit gates, the runtime feature builds, registry admission, the Linux and macOS sandbox/jail proofs, the playground, and the browser e2e |
| **post-merge** | push to `main`, schedule | the sanitizers, the arm64 Tier-2 proof, the Windows and FreeBSD jail proofs, the docs deploy, the release, and the nightly full gate |

A manual dispatch runs every phase. A pull request therefore reports only the
cheap phase; the tests phase runs once, in the merge queue, on the combined tree
that is about to land, and a red there drops the change from the queue.

GitHub reports a skipped required check as passing, so a required context over
a tests or post-merge job is never that job itself. It is a verdict job that
runs on every event (`.github/actions/phase-verdict`): outside its phase or its
path scope it passes vacuously; inside them it passes only when every job it
aggregates succeeded, so a failed, cancelled or wrongly skipped job fails it.
`.github/ci/verify-manifest.py` check 11 holds every job to one phase, every
required context to that verdict, and every tests-phase job to a required
verdict that aggregates it, so a tests-phase red always blocks the merge.

## CI for external contributors

No CI runs on an external contributor's PR (author association `CONTRIBUTOR`,
`FIRST_TIME_CONTRIBUTOR`, or `NONE`) until a maintainer approves the run, and
each new push needs re-approval, so nothing is spent on an unreviewed PR. Once
approved it runs the cheap phase like any PR; the tests phase runs in the merge
queue, which only a maintainer can enqueue.

A green CI is necessary but not sufficient: a maintainer review and approval are
still required to merge.
