#!/usr/bin/env bash
# watch.sh - live monitor for progressive-development (run/orchestrate/autopilot).
# Follows the freshest live log, re-picks as work moves, self-terminates when the
# run ends. Flat 2-space margin, tmux-safe colors, no raw json, "> " marks tools.
#
# Liveness UX (increment-2):
#  · Rendering is delegated to render-stream.sh (ONE source; un-clipped + indented).
#  · A CURRENT-TASK banner reprints on every phase transition, sourced from
#    docs/architecture/progdev-status.txt (task / type / attempt / phase / model) —
#    so the thing you're reading near the bottom always says what's running now.
#  · A spinner + idle-seconds HEARTBEAT prints during silent gaps (e.g. a quiet
#    `cargo test`), so you can tell "working" from "stalled" at a glance.
set -uo pipefail
SELFDIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SELFDIR/../.."
b="$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo '?')"
STATUS="docs/architecture/progdev-status.txt"
HEARTBEAT_S="${WATCH_HEARTBEAT_S:-20}"   # emit a liveness line after this many idle seconds
RENDER="$SELFDIR/render-stream.sh"

# tmux-safe ANSI (basic colors; gated on a tty so redirects stay clean).
if [ -t 1 ]; then
    C0=$'\033[0m'; TAGC=$'\033[36m'; TOOLC=$'\033[33m'
    RESC=$'\033[32m'; DIMC=$'\033[90m'; HDRC=$'\033[1;34m'; BANC=$'\033[1;36m'
else
    C0=; TAGC=; TOOLC=; RESC=; DIMC=; HDRC=; BANC=
fi

active_tool() { pgrep -f 'progressive-development/(autopilot|orchestrate|run)\.sh' >/dev/null 2>&1; }
now_hm()      { date +%H:%M; }
mtime()       { stat -c %Y "$1" 2>/dev/null || echo 0; }
size_of()     { stat -c %s "$1" 2>/dev/null || echo 0; }

tool="idle"
for t in autopilot.sh orchestrate.sh run.sh; do
    pgrep -f "progressive-development/$t" >/dev/null 2>&1 && { tool="$t"; break; }
done
printf '  %s-- progressive-development monitor --%s\n' "$HDRC" "$C0"
printf '  base=%s  active=%s\n' "$b" "$tool"
mapfile -t brs < <(git branch --list 'progressive-development/*' | tr -d ' *')
if [ "${#brs[@]}" -gt 0 ]; then
    printf '  branches:\n'
    for br in "${brs[@]}"; do
        n="$(git rev-list --count "$b..$br" 2>/dev/null || echo 0)"
        printf '    %s (+%s)\n' "$br" "$n"
    done
fi
q="docs/architecture/progressive-development-queue.tsv"
[ -s "$q" ] && { printf '  queue:\n'; awk -F'\t' '{st[$3]=$1;kd[$3]=$2}
    END{for(d in st) if(st[d]=="PENDING") printf "    [%s] %s\n", kd[d], substr(d,1,70)}' "$q"; }
printf '\n'

# CURRENT-TASK banner — the "fixed header" content, reprinted on each phase change
# (near the bottom, where you're reading). Sourced from progdev-status.txt, which
# autopilot rewrites at every phase() transition.
banner() {
    [ -s "$STATUS" ] || return 0
    local task type att phase model
    task="$(awk -F'  +' '/^task/{ $1=""; sub(/^ +/,""); print; exit}' "$STATUS")"
    type="$(awk '/^type/{print $2; exit}' "$STATUS")"
    att="$(awk '/^type/{for(i=1;i<=NF;i++) if($i=="attempt"){print $(i+1); exit}}' "$STATUS")"
    phase="$(awk '/^phase/{print $2; exit}' "$STATUS")"
    model="$(awk '/^phase/{for(i=1;i<=NF;i++) if($i=="model"){print $(i+1); exit}}' "$STATUS")"
    printf '  %s── %s · %s · phase %s/%s · attempt %s · %s ──%s\n' \
        "$BANC" "${task:-?}" "${type:-?}" "${phase:-?}" "${model:-?}" "${att:-?}" "$(now_hm)" "$C0"
}

# spinner frames (braille; degrade gracefully — purely cosmetic)
FRAMES='⠋ ⠙ ⠹ ⠸ ⠼ ⠴ ⠦ ⠧ ⠇ ⠏'; read -r -a SPIN <<< "$FRAMES"; spin_i=0
heartbeat() {  # <idle-seconds> <phase>
    local f="${SPIN[$((spin_i % ${#SPIN[@]}))]}"; spin_i=$((spin_i+1))
    printf '  %s%s working · phase %s · idle %ss · %s%s\n' \
        "$DIMC" "$f" "${2:-?}" "$1" "$(now_hm)" "$C0"
}
cur_phase() { [ -s "$STATUS" ] && awk '/^phase/{print $2; exit}' "$STATUS" || echo '-'; }

TAIL_PID=""
# Kill the follow pipeline. The `tail` is a GRANDCHILD (watch → subshell → tail),
# so killing the subshell orphans it; also reap the tail by its exact logfile arg.
stop_tail() {
    [ -n "$TAIL_PID" ] && { kill "$TAIL_PID" 2>/dev/null; pkill -P "$TAIL_PID" 2>/dev/null; }
    [ -n "${cur:-}" ] && pkill -f "tail -n 20 -f ${cur}" 2>/dev/null
    TAIL_PID=""
}
trap 'stop_tail; exit 0' INT TERM EXIT

# Freshest live log; skip stale prior-run leftovers (>5 min old).
newest_log() {
    local f
    f="$(ls -t docs/architecture/progressive-development-iter-*.log \
              docs/architecture/progressive-development-lane-*.log \
              docs/architecture/progressive-development-*.log \
              /tmp/autopilot-*.log 2>/dev/null | head -1)"
    [ -n "$f" ] && [ -n "$(find "$f" -mmin -5 2>/dev/null)" ] && echo "$f"
}
tag_for() {
    case "$1" in
        progressive-development-lane-*)             tag="lane${1//[^0-9]/}" ;;
        progressive-development-iter-*)             tag="iter${1//[^0-9]/}" ;;
        autopilot-guardian-design-*)                tag="design" ;;
        autopilot-guardian-review-*)                tag="review" ;;
        autopilot-guardian-*)                       tag="impl" ;;
        autopilot-triage-*)                         tag="triage" ;;
        autopilot-audit-*)                          tag="audit" ;;
        autopilot-fuzz-*)                           tag="fuzz" ;;
        autopilot-gate*)                            tag="gate" ;;
        autopilot-reconcile-*|orch-reconcile-*)     tag="reconcile" ;;
        *)                                          tag="${1%.log}" ;;
    esac
}

# Follow one logfile. JSON → render-stream.sh (shared renderer); else plain sed.
follow() {
    local logf="$1" tag; tag_for "$(basename "$logf")"
    printf '  %s-- %s [%s] --%s\n' "$DIMC" "$(basename "$logf")" "$tag" "$C0"
    if command -v jq >/dev/null 2>&1 && [ "$(head -c1 "$logf" 2>/dev/null)" = "{" ]; then
        ( tail -n 20 -f "$logf" | "$RENDER" "$tag" ) &
    else
        ( tail -n 20 -f "$logf" | sed "s/^/  ${TAGC}[${tag}]${C0} /" ) &
    fi
    TAIL_PID=$!
}

printf '  %s-- monitoring (auto-follows newest log; exits when run ends; Ctrl-C) --%s\n' "$DIMC" "$C0"
cur=""; idle=0; last_status_mtime=0; last_size=0; last_growth=$(date +%s)
while :; do
    if active_tool; then idle=0; else
        idle=$((idle+1))
        [ "$idle" -ge 3 ] && { stop_tail; printf '  %s-- run ended - monitor done --%s\n' "$DIMC" "$C0"; exit 0; }
    fi

    # re-pick the freshest log as work moves between phases
    n="$(newest_log)"
    if [ -n "$n" ] && [ "$n" != "$cur" ]; then stop_tail; cur="$n"; follow "$cur"; last_size=0; last_growth=$(date +%s); fi

    # current-task banner on every phase transition
    sm="$(mtime "$STATUS")"
    if [ "$sm" != "$last_status_mtime" ]; then last_status_mtime="$sm"; banner; fi

    # liveness heartbeat during silent gaps (log not growing)
    if [ -n "$cur" ]; then
        sz="$(size_of "$cur")"
        if [ "$sz" != "$last_size" ]; then last_size="$sz"; last_growth=$(date +%s); else
            gap=$(( $(date +%s) - last_growth ))
            if [ "$gap" -ge "$HEARTBEAT_S" ]; then heartbeat "$gap" "$(cur_phase)"; last_growth=$(date +%s); fi
        fi
    fi

    sleep 2
done
