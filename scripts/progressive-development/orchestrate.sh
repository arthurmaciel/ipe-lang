#!/usr/bin/env bash
# orchestrate.sh — parallel, worktree-isolated progressive-development.
#
# Moves from  Σ(task times)  toward  max(author times) + Σ(gate times) + merge.
# Each lane authors ONE pinned item IN PARALLEL in its own git worktree (cheap —
# no per-lane build); then the orchestrator integrates the lane branches onto the
# base SEQUENTIALLY, gating on the single warm target after each merge, so a red
# lane is attributed + reverted and the tree only advances. A merge conflict
# (the norm for kernel lanes that all touch the shared registry files) dispatches
# an Opus 4.8 agent to UNION-reconcile, then re-gates.
#
# Why author-parallel / gate-serial: on one box the gate (cargo test --workspace)
# is CPU+RAM+disk-heavy and does NOT parallelize cheaply (N lanes = N cold builds
# on separate targets). Authoring is API/IO-bound and parallelizes for free. So
# we parallelize the cheap phase and serialize the expensive one on the warm
# shared target.
#
# Usage:
#   scripts/progressive-development/orchestrate.sh "item one" "item two" [...]
#     each argument is one lane, pinned to that item description. Capped at
#     PROGDEV_LANES (this box: 2). No args → nothing to do (won't auto-pick, to
#     avoid two lanes grabbing the same item).
#
# Config (env): PROGDEV_LANES (2) · PROGDEV_ITER_TIMEOUT (5400) ·
#   MASTER_GATE_TARGET (~/.cache/master-gate-target) · PROGDEV_RECONCILE_MODEL
#   (claude-opus-4-8)
set -uo pipefail
cd "$(dirname "$0")/../.."
REPO="$(pwd)"

LANES="${PROGDEV_LANES:-2}"
ITER_TIMEOUT="${PROGDEV_ITER_TIMEOUT:-5400}"
GATE_TARGET="${MASTER_GATE_TARGET:-$HOME/.cache/master-gate-target}"
RECONCILE_MODEL="${PROGDEV_RECONCILE_MODEL:-claude-opus-4-8}"
CONTEXT="$REPO/scripts/progressive-development/context.md"
PROMPT="$REPO/scripts/progressive-development/prompt.md"
TS="${PROGDEV_TS:-manual}"
WTROOT="$REPO/.progressive-development-wt"
LOGDIR="docs/architecture"

log() { printf '%s | orchestrate | %s\n' "$(date -Is)" "$*"; }
die() { log "ABORT: $*"; exit 1; }

# The lane's author-only flag: same flags as run.sh, minus the gate obligation.
lane_claude_args=(--safe-mode --permission-mode auto
    --allowedTools 'Bash(cargo *)' 'Bash(git *)' 'Bash(skyc *)' 'Bash(touch *)'
                   'Bash(cat *)' 'Bash(ls *)' 'Bash(rg *)' 'Bash(sed *)' 'Bash(awk *)'
                   'Bash(mkdir *)' 'Bash(cp *)' 'Bash(mv *)' 'Bash(rm *)' 'Bash(df *)'
                   Edit Write Read Grep Glob)

# ── preconditions ────────────────────────────────────────────────────────────
command -v claude >/dev/null || die "claude CLI not found"
[ -f "$CONTEXT" ] && [ -f "$PROMPT" ] || die "missing context/prompt"
git rev-parse --is-inside-work-tree >/dev/null 2>&1 || die "not a git repo"
[ -z "$(git status --porcelain --untracked-files=no)" ] || die "tracked changes present — commit or stash first"
pgrep -f mem-guard.sh >/dev/null || die "mem-guard.sh not running"
free_gb="$(df -BG --output=avail / | tail -1 | tr -dc '0-9')"
[ "${free_gb:-0}" -lt 15 ] && die "low disk (${free_gb}G)"

items=("$@")
[ "${#items[@]}" -eq 0 ] && die "no items — pass 1..$LANES item descriptions as arguments"
[ "${#items[@]}" -gt "$LANES" ] && { log "capping ${#items[@]} items at PROGDEV_LANES=$LANES"; items=("${items[@]:0:$LANES}"); }

BASE="$(git rev-parse --abbrev-ref HEAD)"
mkdir -p "$WTROOT"
declare -A L_BRANCH L_WT
log "start: base=$BASE lanes=${#items[@]} gate=$GATE_TARGET reconcile=$RECONCILE_MODEL"

# ── phase 1: parallel authoring, each lane in its own worktree ───────────────
pids=()
for idx in "${!items[@]}"; do
    item="${items[$idx]}"
    br="progressive-development/lane-$TS-$idx"
    wt="$WTROOT/lane-$idx"
    ilog="$LOGDIR/progressive-development-lane-$idx.log"
    rm -rf "$wt"; git worktree add --quiet -b "$br" "$wt" "$BASE" \
        || { log "lane $idx: worktree add failed — skipping"; continue; }
    L_BRANCH[$idx]="$br"; L_WT[$idx]="$wt"
    log "lane $idx → $br  (item: ${item:0:60})"
    (
        cd "$wt" || exit 1
        timeout "$ITER_TIMEOUT" claude "${lane_claude_args[@]}" \
            --append-system-prompt-file "$CONTEXT" \
            -p "$(cat "$PROMPT")

## ORCHESTRATED LANE — pinned
You are one PARALLEL lane. Work on EXACTLY this item and nothing else:

  → $item

Do the root-cause edits and \`git add -A && git commit\` them on THIS branch with
a clear message. Do NOT run the full workspace gate (cargo test --workspace) —
the orchestrator integrates + gates all lanes afterward on the shared target;
running it here would be a redundant cold build. Still obey every principle, the
boundary, and the seal. If the item is excluded/non-mechanical, write an
escalation and exit without committing. Final line: PROGDEV: LANDED/ESCALATED/FAILED." \
            > "$REPO/$ilog" 2>&1
    ) &
    pids+=("$!")
    while [ "$(jobs -rp | wc -l)" -ge "$LANES" ]; do wait -n 2>/dev/null || break; done
done
wait
log "authoring phase complete"

# ── phase 2: sequential integrate (merge → reconcile → gate → keep/revert) ───
run_gate() {
    ( cd "$REPO"
      touch runtime/tests/*.rs crates/skyc/tests/*.rs 2>/dev/null
      CARGO_TARGET_DIR="$GATE_TARGET" timeout 3000 cargo test --workspace >/tmp/orch-gate.log 2>&1 \
      && CARGO_TARGET_DIR="$GATE_TARGET" timeout 1200 cargo clippy --workspace --all-targets -- -D warnings >>/tmp/orch-gate.log 2>&1 )
}

git switch "$BASE" >/dev/null 2>&1
merged=0
for idx in "${!items[@]}"; do
    br="${L_BRANCH[$idx]:-}"; [ -z "$br" ] && continue
    ahead="$(git rev-list --count "$BASE..$br" 2>/dev/null || echo 0)"
    if [ "$ahead" -eq 0 ]; then log "lane $idx: nothing committed (escalated/failed) — skip"; continue; fi
    log "lane $idx: merging $br ($ahead commit(s))"
    if ! git merge --no-ff -m "orchestrate: merge lane $idx ($br)" "$br" >/dev/null 2>&1; then
        conflicts="$(git diff --name-only --diff-filter=U | tr '\n' ' ')"
        log "lane $idx: CONFLICT in [$conflicts] — dispatching $RECONCILE_MODEL to union-reconcile"
        claude --model "$RECONCILE_MODEL" --safe-mode --permission-mode auto \
            --allowedTools 'Bash(git *)' 'Bash(rg *)' 'Bash(cat *)' Edit Write Read Grep Glob \
            -p "You are resolving an in-progress git merge conflict from two PARALLEL compiler-kernel lanes in a Rust workspace. These conflicts are almost always a UNION: both lanes appended a variant to the same enum, an arm to the same match, or an entry to the same table/list. Resolve EVERY conflicted file by KEEPING BOTH sides' additions (union) — never drop either lane's work — and keep every match/enum exhaustive and alphabetically/ordinally consistent with the surroundings. Conflicted files: $conflicts . When done, \`git add -A && git commit --no-edit\`. If a conflict is NOT a clean union (genuine semantic clash), abort with \`git merge --abort\` and print RECONCILE: MANUAL <reason>." \
            >/tmp/orch-reconcile-$idx.log 2>&1
        if [ -n "$(git diff --name-only --diff-filter=U)" ]; then
            log "lane $idx: reconcile did NOT resolve all conflicts — aborting merge (see /tmp/orch-reconcile-$idx.log)"
            git merge --abort 2>/dev/null; continue
        fi
        log "lane $idx: reconcile succeeded"
    fi
    log "lane $idx: gating merged result on $GATE_TARGET …"
    if run_gate; then
        log "lane $idx: GATE GREEN — kept"; merged=$((merged+1))
    else
        log "lane $idx: GATE RED after merge — reverting (see /tmp/orch-gate.log)"
        git reset --hard "HEAD~1" >/dev/null 2>&1
    fi
done

# ── phase 3: cleanup ─────────────────────────────────────────────────────────
for idx in "${!items[@]}"; do
    [ -n "${L_WT[$idx]:-}" ] && git worktree remove --force "${L_WT[$idx]}" 2>/dev/null
    [ -n "${L_BRANCH[$idx]:-}" ] && git branch -D "${L_BRANCH[$idx]}" 2>/dev/null
done
git worktree prune
rmdir "$WTROOT" 2>/dev/null || true

log "done: $merged/${#items[@]} lanes merged onto $BASE (tree $(git rev-parse --short HEAD))"
