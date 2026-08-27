#!/usr/bin/env bash
# Enable branch protection on `main` so the branch is green by construction:
# every change lands through a PR whose FAST required gate is green, and merges
# are auto-merged the moment that gate passes. The slow checks (e2e, miri,
# feature-combos) run post-merge on push + nightly and do
# NOT gate the PR — see .github/workflows/{ci,security}.yml.
#
# ── RUN THIS AFTER IN-FLIGHT DIRECT-PUSH LANES DRAIN ─────────────────────────
# While direct-push build lanes are still landing on `main`, requiring PRs would
# block them. Flip protection on only once those lanes are done and every future
# change goes through a PR. This script is intentionally NOT executed by CI or
# by setup — a human/orchestrator runs it deliberately.
#
# Required status checks below are the FAST gate job names, exactly as they
# appear in the workflows:
#   fmt, clippy, test   — .github/workflows/ci.yml
#   seal-smoke          — .github/workflows/ci.yml (build+run one emitted example)
#   cargo-deny          — .github/workflows/security.yml (supply-chain)
# The slow jobs (e2e, miri, runtime-*, wasm-floor)
# are deliberately NOT listed — they self-skip on pull_request or run advisory.
#
# `strict: false`: PRs do NOT have to be up to date with `main` before merging.
# `strict: true` would require it, but GitHub only auto-updates a behind branch
# when a full merge QUEUE is configured — without one it just adds a manual
# "Update branch" button, so strict + many concurrent PRs means constant manual
# rebasing. The required checks still gate every PR and literal conflicts still
# block the merge; only semantic merge-skew is possible, which the post-merge
# push CI catches. Revisit strict only alongside a merge queue.
set -euo pipefail

REPO="arthurmaciel/ipe-lang"
BRANCH="main"

echo "Enabling branch protection on ${REPO}@${BRANCH} …"

# 1. Branch protection: required status checks (strict) + required PR review
#    + no direct pushes (enforced by requiring a PR). PUT to the branch-
#    protection endpoint with the full desired state.
gh api -X PUT "repos/${REPO}/branches/${BRANCH}/protection" \
  -H "Accept: application/vnd.github+json" \
  --input - <<'JSON'
{
  "required_status_checks": {
    "strict": false,
    "contexts": ["fmt", "clippy", "test", "seal-smoke", "cargo-deny"]
  },
  "enforce_admins": false,
  "required_pull_request_reviews": {
    "required_approving_review_count": 0,
    "dismiss_stale_reviews": false,
    "require_code_owner_reviews": false
  },
  "restrictions": null,
  "required_linear_history": false,
  "allow_force_pushes": false,
  "allow_deletions": false,
  "required_conversation_resolution": true
}
JSON

# 2. Enable repo-level auto-merge so a PR can be set to merge automatically once
#    the required gate is green and the branch is up to date.
gh api -X PATCH "repos/${REPO}" \
  -H "Accept: application/vnd.github+json" \
  -f allow_auto_merge=true \
  -f delete_branch_on_merge=true

echo "Branch protection enabled. PRs now merge only when the fast gate is green."
echo "Set a PR to auto-merge with: gh pr merge <N> --auto --squash"
