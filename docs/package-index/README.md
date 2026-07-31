# The Ipê package index repository

This directory is the source scaffold for the **curated index repository** — the
separate, hosted git repository the Ipê CLI resolves packages from (the default
target of `ipe package publish` is `arthurmaciel/ipe-index`). It holds the pieces
that belong to the *index side* of the package-coordination design in
[`docs/adr/0044`](../adr/0044-package-coordination-manifest-index-gate.md):
the entry layout, the entry schema, an example entry, and the admission-CI trust
model.

The **client** side already ships in the CLI (`ipe package publish`): it runs the
audit gate, computes the entry, and opens a pull request against this index. It
holds no index credentials. The index repository is the authority — it re-runs the
gate on the submitted entry and merges only on a green result.

## What lives here vs. in the hosted repo

The hosted index repository is a *separate* repository; it cannot exist inside the
compiler repo. What is committed here is everything that can be authored and
tested in-repo, ready to copy into the hosted repo when it is stood up:

| Artifact | Path | Purpose |
| --- | --- | --- |
| Entry schema | [`SCHEMA.md`](SCHEMA.md) | The canonical `packages/<name>.toml` layout and per-field contract — the source of truth. |
| Example entry | [`packages/example-package.toml`](packages/example-package.toml) | A complete, valid entry the CI validator accepts. |
| Admission CI | [`workflows/admission.yml`](workflows/admission.yml) | The fail-closed trust boundary: validate → verify source pin → re-run the gate → merge only on green. |

What is **deferred to the hosted repo** (nothing in this compiler repo can supply
it):

- The repository itself, its `main` branch, and its branch-protection rule that
  makes the `admission` workflow a *required* status check (the merge gate is only
  fail-closed once merges are blocked without it).
- The real published entries under `packages/` (this directory ships one example
  only; the live index is populated by merged `ipe package publish` PRs).
- The GitHub App / token the merge step uses. The workflow uses the ambient
  `GITHUB_TOKEN` and never stores a credential of its own.

## The validator

The schema's Rust validator is not a separate program — it is the shipped `ipe`
CLI. `ipe package validate-entry packages/<name>.toml` parses an entry with the
resolver's own parser and exits non-zero on any malformed field, so the validator
and the resolver can never drift: a file the CI validates is exactly a file the
resolver will later read. The admission workflow installs a pinned `ipe` release
and calls it.
