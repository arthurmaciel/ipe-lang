# Contributing to Ipê

Ipê is a principled, security-first compiler. Every contribution is weighed
against `PRINCIPLES.md` (the single source of truth for the enforced rules) and
the contributor map in `AGENTS.md`. Read both before opening a pull request.

## Pull requests

Branch off `development`, keep one unit of work per PR, and make the fast gate
pass locally before you push:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --workspace -- -D warnings
cargo nextest run -p ipe
```

`development` is the integration branch; `main` advances only by the promotion
of an already-green `development` head.

## CI for external contributors

Maintainer-authored PRs run only the fast gate at PR time and defer the heavy
suite (the full SEAL e2e, the sanitizer and miri passes, the Tier-2 sandbox/jail
proofs, the registry-admission gate, and the browser e2e) to the
`development → main` promotion. That trust optimization does not extend to an
outside contribution.

An external PR (author association `CONTRIBUTOR`, `FIRST_TIME_CONTRIBUTOR`, or
`NONE`) runs the **full** gate at PR time, so a broken or hostile change is
surfaced before it can reach `development` rather than caught later at the
promotion backstop. Two things gate it:

- **Approval to run.** No CI runs on an external contributor's PR until a
  maintainer approves the run, and each new push needs re-approval. Nothing is
  spent on an unreviewed PR.
- **Full gate once approved.** After approval the heavy suite runs on the PR, in
  addition to the fast gate — the whole signal is on the PR before a maintainer
  merges.

A green CI is necessary but not sufficient: a maintainer review and approval are
still required to merge.
