# Spec — the required-check set is one file that the workflows and the ruleset are checked against

Files: `.github/ci/required-set.json`, `.github/ci/verify-manifest.py`,
`.github/workflows/check-manifest.yml`, `.github/workflows/ci.yml`,
`.github/ci/RECONCILIATION.md`. Principles: SSOT, defend in depth. Lane tier:
standard.

## Class-closing property

`required-set.json` is the single list of required check names. A CI job
fails when (a) a name in it is produced by no workflow job, (b) a job marked
required is missing from it, or (c) a job in it can be skipped by an `if:`
that does not also skip on the ruleset side. The live ruleset is reconciled
from the file, never edited by hand.

## Why the instances exist (origin/main 983fdc404)

- `RECONCILIATION.md:54,78,80,85` and `check-manifest.yml:12` reference a
  `promotion-ready` check and ruleset 22326541; no workflow defines a
  `promotion-ready` job — a required name no job produces blocks or is
  silently ignored (#2954).
- `ci.yml:312` `registry-admission` has an `author_association` `if:`; a
  skipped required job reports success → the gate is bypassable by
  association (#2945).
- `verify-manifest.py` checks names but not skip conditions (#2934 item 1).

## Members

| Issue | Status | Verified by |
|---|---|---|
| #2954 `promotion-ready` referenced, not produced | real | `git grep promotion-ready .github` |
| #2945 conditional required job | real | `ci.yml:312` |
| #2934 item 1 manifest checks skip conditions | real | `verify-manifest.py` |

## Implementation plan

1. `verify-manifest.py` parses every workflow; asserts required ⊆ produced
   job names, and flags any required job with an `if:` unless it is on an
   explicit, reasoned allowlist in `required-set.json`.
2. Either add the `promotion-ready` aggregator job (needs: all required) or
   remove it from docs + ruleset — see LIMIT.
3. `registry-admission`: move the association check inside the job (the job
   always runs, fails closed for untrusted authors) so "skipped" is not
   reachable.
4. Rewrite `RECONCILIATION.md` in present tense (no archaeology).

## Prove the refusals

- fixture workflow set missing a required name → verify fails;
- required job with an `if:` not on the allowlist → verify fails;
- PR from a non-member → `registry-admission` runs and refuses (not skipped).

## LIMIT

- #2934 item 2 (backport policy for release branches): which branches the
  ruleset covers is a maintainer decision.
- #2945 freebsd leg flakes: whether it is required. Decision needed: required
  (fix the flake first) or advisory (listed as non-required in the file).
- `promotion-ready`: keep as aggregator or drop. Decision needed.

Lane tier: standard.
