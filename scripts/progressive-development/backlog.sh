#!/usr/bin/env bash
# backlog.sh — CRUD over the BACKLOG SSOT.
#
# scripts/progressive-development/backlog.jsonl is the ONLY source of truth
# (one JSON object per line). There is no generated BACKLOG.md anymore —
# root BACKLOG.md was deleted 2026-07-12 (explicit call: the JSONL is the
# working data, a markdown mirror added no value once the loop reads/writes
# JSONL directly). `show` prints a human-readable table to STDOUT on demand;
# it never writes a file.
#
# backlog_table.py still exists as the one-time migration tool + the pretty-
# printer `show` shells out to for table formatting — kept because its
# round-trip parsing is proven sound (see that file's docstring), not because
# anything renders to a committed .md file anymore.
#
# Usage:
#   backlog.sh list [--status pending|claimed|done] [--phase "P1;P2"]   # JSONL rows, one per line
#   backlog.sh show [--status pending|claimed|done] [--phase "P1;P2"]   # human-readable table, stdout only
#
# NOTE: --phase multi-value separator is `;`, NOT `,` — one canonical phase
# name ("CI, oracle & publish") contains a literal comma, so splitting on
# comma would silently break filtering on that exact phase (found + fixed
# 2026-07-12: an empty-but-no-error result on `--phase "CI, oracle & publish"`
# is exactly the silent-wrong-answer failure mode this project's principles
# forbid — caught while building the first status report off this tool).
#   backlog.sh add --priority P --phase PH --task T --notes N [--spec S]
#   backlog.sh claim <id> [<id>...]
#   backlog.sh unclaim <id> [<id>...]
#   backlog.sh close <id> --done-at YYYY-MM-DD
#
# KNOWN LIMITATION (flagged, not silently skipped): `close` marks the row
# done in the JSONL but does NOT yet stamp ROADMAP.md's matching per-section
# "Done at" column — that file has its own, separately-complex per-section
# table format not parsed/verified here. `close` prints an explicit reminder
# line instead of guessing at an unverified edit. Follow-up, not silently
# dropped.
set -euo pipefail
cd "$(dirname "$0")/../.."
HERE="scripts/progressive-development"
JSONL="$HERE/backlog.jsonl"
PY="$HERE/backlog_table.py"
LOCK="$HERE/.backlog.lock"

PRIORITIES="Critical High Medium Low"
PHASES="Sweep to green|Security hardening|CI, oracle & publish|Hardening follow-ups|FFI|Post-completion|Longer-horizon|Designed targets"

die() { echo "backlog.sh: $*" >&2; exit 1; }

[ -f "$JSONL" ] || die "missing $JSONL"
command -v jq >/dev/null || die "jq required"

valid_priority() { local p="$1" x; for x in $PRIORITIES; do [ "$x" = "$p" ] && return 0; done; return 1; }
valid_phase() {
    local p="$1"
    IFS='|' read -ra arr <<< "$PHASES"
    local x; for x in "${arr[@]}"; do [ "$x" = "$p" ] && return 0; done
    return 1
}

with_lock() { # <fn> [args...] — serializes JSONL mutation across concurrent invocations
    flock "$LOCK" -c "$(printf '%q ' "$@")"
}

_filtered() { # --status S --phase "P1,P2" -> jq rows on stdout
    local status="" phase=""
    while [ $# -gt 0 ]; do case "$1" in
        --status) status="$2"; shift 2 ;;
        --phase)  phase="$2"; shift 2 ;;
        *) die "unknown arg $1" ;;
    esac; done
    jq -c 'select(has("id"))' "$JSONL" \
        | if [ -n "$status" ]; then jq -c --arg s "$status" 'select(.status == $s)'; else cat; fi \
        | if [ -n "$phase" ]; then
              jq -c --arg phases "$phase" '
                  (.phase) as $p | ($phases | split(";")) as $want | select($want | index($p))'
          else cat; fi
}

cmd_list() { _filtered "$@"; }

cmd_show() {
    _filtered "$@" | jq -r '[.id, .status, .priority, .phase, (.task | .[0:70])] | @tsv' \
        | { printf 'ID\tSTATUS\tPRIORITY\tPHASE\tTASK\n'; cat; } | column -t -s $'\t'
}

cmd_add() {
    local priority="" phase="" task="" notes="" spec=""
    while [ $# -gt 0 ]; do case "$1" in
        --priority) priority="$2"; shift 2 ;;
        --phase)    phase="$2"; shift 2 ;;
        --task)     task="$2"; shift 2 ;;
        --notes)    notes="$2"; shift 2 ;;
        --spec)     spec="$2"; shift 2 ;;
        *) die "add: unknown arg $1" ;;
    esac; done
    [ -n "$priority" ] && [ -n "$phase" ] && [ -n "$task" ] || die "add: --priority --phase --task are required"
    valid_priority "$priority" || die "add: unknown priority '$priority' (one of: $PRIORITIES)"
    valid_phase "$phase" || die "add: unknown phase '$phase' (one of: $PHASES)"

    _add_locked() {
        local next
        next="$(jq -r 'select(has("id")) | .id' "$JSONL" | grep -E '^[0-9]+$' | sort -n | tail -1)"
        next="${next:-168}"; next=$((next + 1))
        local id="$next"
        jq -cn --arg id "$id" --arg priority "$priority" --arg phase "$phase" \
               --arg task "#$id $task" --arg notes "$notes" --arg spec "$spec" \
               '{id:$id, priority:$priority, phase:$phase, task:$task, notes:$notes, spec:$spec, status:"pending"}' \
            >> "$JSONL"
        echo "added #$id ($phase / $priority)" >&2
    }
    with_lock bash -c "$(declare -f _add_locked die); JSONL='$JSONL' priority='$priority' phase='$phase' task='$task' notes='$notes' spec='$spec' _add_locked"
}

cmd_claim() {
    [ $# -ge 1 ] || die "claim: need at least one id"
    local ids="$*" run_id="${PROGDEV_RUN_ID:-$(hostname)-$$}" now
    now="$(date -Is)"
    _claim_locked() {
        local tmp; tmp="$(mktemp)"
        jq -c --arg ids "$ids" --arg run_id "$run_id" --arg now "$now" '
            if has("id") and ((" " + $ids + " ") | contains(" " + .id + " ")) and .status == "pending"
            then .status = "claimed" | .claimed_by = $run_id | .claimed_at = $now
            else . end' "$JSONL" > "$tmp"
        mv "$tmp" "$JSONL"
    }
    with_lock bash -c "$(declare -f _claim_locked); JSONL='$JSONL' ids='$ids' run_id='$run_id' now='$now' _claim_locked"
}

cmd_unclaim() {
    [ $# -ge 1 ] || die "unclaim: need at least one id"
    local ids="$*"
    _unclaim_locked() {
        local tmp; tmp="$(mktemp)"
        jq -c --arg ids "$ids" '
            if has("id") and ((" " + $ids + " ") | contains(" " + .id + " ")) and .status == "claimed"
            then .status = "pending" | del(.claimed_by) | del(.claimed_at)
            else . end' "$JSONL" > "$tmp"
        mv "$tmp" "$JSONL"
    }
    with_lock bash -c "$(declare -f _unclaim_locked); JSONL='$JSONL' ids='$ids' _unclaim_locked"
}

cmd_close() {
    local id="" done_at=""
    id="${1:-}"; shift || true
    while [ $# -gt 0 ]; do case "$1" in
        --done-at) done_at="$2"; shift 2 ;;
        *) die "close: unknown arg $1" ;;
    esac; done
    [ -n "$id" ] && [ -n "$done_at" ] || die "close: usage: close <id> --done-at YYYY-MM-DD"

    local row; row="$(jq -c --arg id "$id" 'select(has("id")) | select(.id == $id)' "$JSONL")"
    [ -n "$row" ] || die "close: no row with id $id"

    _close_locked() {
        local tmp; tmp="$(mktemp)"
        jq -c --arg id "$id" --arg done_at "$done_at" \
            'if has("id") and .id == $id then .status = "done" | .done_at = $done_at else . end' "$JSONL" > "$tmp"
        mv "$tmp" "$JSONL"
    }
    with_lock bash -c "$(declare -f _close_locked); JSONL='$JSONL' id='$id' done_at='$done_at' _close_locked"

    local phase priority task
    phase="$(echo "$row" | jq -r .phase)"; priority="$(echo "$row" | jq -r .priority)"; task="$(echo "$row" | jq -r .task | cut -c1-80)"
    echo "closed #$id." >&2
    echo "REMINDER (not automated yet): stamp ROADMAP.md's '$phase' section, row starting '$task…', Done at = $done_at (priority $priority)." >&2
}

cmd="${1:-}"; shift || true
case "$cmd" in
    list)    cmd_list "$@" ;;
    show)    cmd_show "$@" ;;
    add)     cmd_add "$@" ;;
    claim)   cmd_claim "$@" ;;
    unclaim) cmd_unclaim "$@" ;;
    close)   cmd_close "$@" ;;
    *) die "unknown command '$cmd' — list|show|add|claim|unclaim|close" ;;
esac
