#!/usr/bin/env bash
# release-only — classify a pull request as a pure release-please version bump.
#
# Emits `release_only=true` to "$GITHUB_OUTPUT" only when EVERY condition holds:
#   • the event is a pull_request whose head repository is this repository (a
#     fork can never qualify);
#   • the PR changes `.github/.release-please-manifest.json`;
#   • every changed file is in RELEASE_FILES below.
# Such a PR's tree is the already-gated `main` plus version metadata, so the heavy
# emitted-build/SEAL/sandbox tiers that a version string cannot affect may skip;
# the version-sensitive gates (quick-check, clippy, test, manifest-lock-
# consistency, drift gates) still run. The classification is by the exact
# changed-file set, never the branch name: a branch name is chosen by whoever
# opens the PR, the diff is not. Any error, an empty or truncated file list, or
# one file outside the set yields `release_only=false` (fail closed).
#
# Inputs (env): EVENT_NAME, PR_NUMBER, HEAD_REPO, REPO, GH_TOKEN.

set -euo pipefail

RELEASE_FILES=(
  .github/.release-please-manifest.json
  CHANGELOG.md
  Cargo.toml
  Cargo.lock
)
MANIFEST=.github/.release-please-manifest.json
# The pulls/files API lists at most this many files; a PR at the cap may be
# truncated, so it cannot be proven release-only.
API_FILE_CAP=3000

verdict=false
emit() {
  echo "release_only=$verdict" >>"${GITHUB_OUTPUT:-/dev/stdout}"
  echo "release-only: $verdict — $1" >&2
}

if [[ "${EVENT_NAME:-}" != pull_request ]]; then
  emit "not a pull_request event"
  exit 0
fi
if [[ -z "${HEAD_REPO:-}" || "$HEAD_REPO" != "${REPO:-}" ]]; then
  emit "head repository '${HEAD_REPO:-}' is not '${REPO:-}'"
  exit 0
fi

if ! files=$(gh api --paginate "repos/$REPO/pulls/$PR_NUMBER/files" -q '.[].filename'); then
  emit "could not list the PR's changed files"
  exit 0
fi

count=0
saw_manifest=false
while IFS= read -r f; do
  [[ -z "$f" ]] && continue
  count=$((count + 1))
  [[ "$f" == "$MANIFEST" ]] && saw_manifest=true
  allowed=false
  for r in "${RELEASE_FILES[@]}"; do
    [[ "$f" == "$r" ]] && allowed=true && break
  done
  if [[ "$allowed" != true ]]; then
    emit "changes '$f', outside the release file set"
    exit 0
  fi
done <<<"$files"

if ((count == 0 || count >= API_FILE_CAP)); then
  emit "file list is empty or at the API cap ($count)"
  exit 0
fi
if [[ "$saw_manifest" != true ]]; then
  emit "does not change $MANIFEST"
  exit 0
fi

verdict=true
emit "only release files changed ($count)"
