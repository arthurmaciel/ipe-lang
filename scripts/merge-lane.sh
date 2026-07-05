#!/usr/bin/env bash
# merge-lane.sh — orchestrator merge protocol for worktree build lanes.
#
# Usage: scripts/merge-lane.sh <agent-id-or-branch> <short-name> "<merge message>" [more-branches...]
#   scripts/merge-lane.sh worktree-agent-abc123 batch-foo "Merge batch-foo: ..." \
#       [worktree-agent-def456:batch-bar]
#
# Protocol (memorized invariants):
#   1. rename worktree branch -> batch name; remove worktree; prune
#   2. merge --no-ff (STOPS on conflict — resolve by union, then rerun gate step)
#   3. blanket-touch path-baking test files (CARGO_MANIFEST_DIR stale-binary hazard)
#   4. full test + clippy gate on the ISOLATED master-gate target
#      (never the shared lane target — running lanes repoison rlibs)
set -euo pipefail
cd "$(dirname "$0")/.."

GATE_TARGET="${MASTER_GATE_TARGET:-$HOME/.cache/master-gate-target}"

if [ "$#" -lt 3 ]; then
    echo "usage: $0 <worktree-branch> <batch-name> <merge-msg> [extra worktree-branch:batch-name pairs...]" >&2
    exit 2
fi

wt_branch="$1"; batch="$2"; msg="$3"; shift 3

merge_one() {
    local from="$1" to="$2"
    git branch -m "$from" "$to"
    # worktree dir name matches the agent id suffix of the branch
    local dir=".claude/worktrees/${from#worktree-}"
    [ -d "$dir" ] && git worktree remove --force "$dir" || true
    git worktree prune
}

merge_one "$wt_branch" "$batch"
branches=("$batch")
for pair in "$@"; do
    from="${pair%%:*}"; to="${pair##*:}"
    merge_one "$from" "$to"
    branches+=("$to")
done

for b in "${branches[@]}"; do
    git merge --no-ff "$b" -m "$msg"   # conflict -> exits 1; resolve + rerun gate manually
done

# Stale-binary hazard: force-rebuild every path-baking test binary.
touch runtime/tests/*.rs crates/skyc/tests/*.rs

mkdir -p "$GATE_TARGET"
CARGO_TARGET_DIR="$GATE_TARGET" timeout 3000 cargo test --workspace
CARGO_TARGET_DIR="$GATE_TARGET" timeout 1200 cargo clippy --workspace --all-targets -- -D warnings

for b in "${branches[@]}"; do
    git branch -d "$b"
done
echo "MERGE GATE GREEN: ${branches[*]}"
