# Contributing to Ipê

We really appreciate your contribution! Ipê is a principled, security-first
compiler — every change is weighed against `PRINCIPLES.md` (the single source of
truth for the enforced rules) and the contributor map in `AGENTS.md`. Please read
both before opening a pull request, then follow the steps below:

1. **Clone the repo**

   ```bash
   git clone https://github.com/arthurmaciel/ipe-lang.git
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
     | xargs -0 tools/panic-scan/target/release/panic-scan
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

## CI for external contributors

An external PR (author association `CONTRIBUTOR`, `FIRST_TIME_CONTRIBUTOR`, or
`NONE`) runs the **full** gate at PR time, so a broken or hostile change is
surfaced before it can reach `main`. Two things gate it:

- **Approval to run.** No CI runs on an external contributor's PR until a
  maintainer approves the run, and each new push needs re-approval. Nothing is
  spent on an unreviewed PR.
- **Full gate once approved.** After approval the heavy suite (the full SEAL
  e2e, the sanitizer and miri passes, the Tier-2 sandbox/jail proofs, the
  registry-admission gate, and the browser e2e) runs on the PR, in addition to
  the fast gate.

A green CI is necessary but not sufficient: a maintainer review and approval are
still required to merge.
