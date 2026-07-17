#!/usr/bin/env bash
# scripts/guards/lane-guard.sh — stalled/dead build-lane detector for the Ipê
# (Sky->Rust) dev sessions, sibling to mem-guard.sh and disk-guard.sh.
#
# Background: a dispatched background Agent can die silently to
# infrastructure issues (observed 4 times in one session: the harness's own
# task tracker returns "no task found" for an agent that produced no final
# report and left no commit) with NO notification — the only way to find out
# was to ask "is this still running?" and manually check. This watchdog
# polls every git worktree lane on a fixed interval and flags one whose
# state hasn't moved AT ALL (no new commit, no file touched, no live
# rustc/cargo process) since the previous poll — the exact signature every
# dead agent this session left behind.
#
# This script CANNOT itself confirm a Claude Code Agent task is dead (that's
# harness-internal state, only queryable via the Agent tool's TaskOutput from
# inside a session) — it only detects the filesystem/process-level PROXY
# signal (staleness) and reports it. A flagged lane still needs a human/
# orchestrator follow-up (check TaskOutput for the real task ID, and if it's
# genuinely gone, relaunch into the same worktree rather than discarding it —
# see the CLAUDE.md "near-miss" precedent on never blindly resetting a
# worktree that might hold uncommitted in-progress work).
#
# Usage:
#   ./scripts/guards/lane-guard.sh                      # one poll pass, prints findings, exits
#   watch -n 900 ./scripts/guards/lane-guard.sh          # simple standalone repeat (no state dir needed
#                                                  # across runs if you don't care about the
#                                                  # "unchanged since LAST poll" comparison — see
#                                                  # LANE_GUARD_STATE_DIR below for that)
#   nohup ./scripts/guards/lane-guard.sh --loop >/tmp/lane-guard.out 2>&1 & disown   # background daemon
#
# Tunables (env vars, all optional):
#   LANE_GUARD_INTERVAL     poll interval when run with --loop (seconds).  default 1800 (30 min)
#   LANE_GUARD_LOG          log file path.                                 default /tmp/lane-guard.log
#   LANE_GUARD_STATE_DIR    per-lane last-seen state (commit hash + max     default /tmp/lane-guard-state
#                           source mtime), used to detect "nothing moved
#                           since the last poll" rather than an absolute
#                           staleness age (a lane can legitimately think
#                           for a while without touching files; "identical
#                           to last poll" is a tighter, less-noisy signal
#                           than "idle for N minutes").
#   LANE_GUARD_WORKTREE_DIR worktrees root.                                default .claude/worktrees
#                           (relative to repo root, auto-detected from
#                           this script's own location)
#
# Exit / output contract (for Monitor-tool wrapping): each poll pass prints
# ONE line per lane whose state is IDENTICAL to the previous poll AND has
# zero live build processes — `STALLED <lane> last_commit=<hash> idle_since=<ts>`.
# A quiet lane that's still building (live procs present) or that produced
# a new commit/file touch since the last poll is NEVER printed — silence
# means "still working," matching this project's Monitor-tool convention
# of only emitting on the events worth a human's attention.

set -euo pipefail

INTERVAL="${LANE_GUARD_INTERVAL:-1800}"
LOG="${LANE_GUARD_LOG:-/tmp/lane-guard.log}"
STATE_DIR="${LANE_GUARD_STATE_DIR:-/tmp/lane-guard-state}"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORKTREE_DIR="${LANE_GUARD_WORKTREE_DIR:-$REPO_ROOT/.claude/worktrees}"

mkdir -p "$STATE_DIR"

log() {
    printf '[%s] %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$*" | tee -a "$LOG" >&2
}

# True (0) if any process references this worktree's own source path OR its
# CARGO_TARGET_DIR naming convention (sky-rust-target-<lane>) in its argv —
# covers both a live cargo/rustc build AND an agent shell actively editing/
# reading files under the worktree path itself.
lane_is_live() {
    local worktree_path="$1" lane="$2"
    pgrep -f -- "$worktree_path" > /dev/null 2>&1 && return 0
    pgrep -f -- "sky-rust-target-${lane}" > /dev/null 2>&1 && return 0
    return 1
}

# A single content fingerprint for "has anything moved": the current commit
# hash (captures new commits) concatenated with the newest mtime among
# tracked-and-modified + untracked files (captures uncommitted edits a
# still-working agent is making but hasn't committed yet — the exact shape
# of the recovered ~1850-line diff from the salsa-p6-symrelo incident, so a
# lane mid-edit on real work is never mistaken for stalled).
lane_fingerprint() {
    local worktree_path="$1"
    local commit dirty_mtime
    commit="$(git -C "$worktree_path" rev-parse HEAD 2>/dev/null || echo "no-head")"
    # Newest mtime across anything git considers changed (modified + untracked,
    # excluding ignored build output) — falls back to 0 if the tree is clean.
    dirty_mtime="$(git -C "$worktree_path" status --porcelain 2>/dev/null \
        | awk '{print $2}' \
        | xargs -r -I{} stat -c '%Y' "$worktree_path/{}" 2>/dev/null \
        | sort -rn | head -1)"
    echo "${commit}:${dirty_mtime:-0}"
}

poll_once() {
    local found_any=0
    [[ -d "$WORKTREE_DIR" ]] || { log "no worktree dir at $WORKTREE_DIR — nothing to watch"; return 0; }

    local wt
    for wt in "$WORKTREE_DIR"/agent-*; do
        [[ -d "$wt" ]] || continue
        local lane; lane="$(basename "$wt")"
        lane="${lane#agent-}"
        local state_file="$STATE_DIR/${lane}.state"
        local fp; fp="$(lane_fingerprint "$wt")"

        if lane_is_live "$wt" "$lane"; then
            # Actively building or being edited right now — definitely alive,
            # update state and move on regardless of fingerprint comparison.
            echo "$fp" > "$state_file"
            continue
        fi

        if [[ -f "$state_file" ]]; then
            local prev; prev="$(cat "$state_file")"
            if [[ "$prev" == "$fp" ]]; then
                found_any=1
                local commit_hash="${fp%%:*}"
                echo "STALLED $lane last_commit=${commit_hash:0:12} unchanged_since_last_poll worktree=$wt"
                log "STALLED $lane last_commit=${commit_hash:0:12} unchanged_since_last_poll (no live process, no commit/edit since previous ${INTERVAL}s poll)"
            fi
        fi
        echo "$fp" > "$state_file"
    done

    return 0
}

# Remove state files for lanes whose worktree no longer exists (merged +
# reclaimed) so a future re-created lane with the same name doesn't get a
# false "unchanged" match against a stale fingerprint from a prior lane.
prune_state() {
    local f lane
    [[ -d "$STATE_DIR" ]] || return 0
    for f in "$STATE_DIR"/*.state; do
        [[ -f "$f" ]] || continue
        lane="$(basename "$f" .state)"
        [[ -d "$WORKTREE_DIR/agent-${lane}" ]] || rm -f "$f"
    done
}

trap 'log "stopping (signal)"; exit 0' INT TERM

if [[ "${1:-}" == "--loop" ]]; then
    log "starting loop mode (interval=${INTERVAL}s state_dir=$STATE_DIR worktree_dir=$WORKTREE_DIR)"
    while :; do
        prune_state
        poll_once
        sleep "$INTERVAL"
    done
else
    prune_state
    poll_once
fi
