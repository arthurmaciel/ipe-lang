#!/usr/bin/env bash
# watch.sh — live monitor for ANY progressive-development activity:
# run.sh (single/loop), orchestrate.sh (parallel lanes), or autopilot.sh.
#
#   scripts/progressive-development/watch.sh
#
# Prints a header (which tool is running, all progressive-development/* branches +
# their commits, the autopilot queue, escalations/digest), then FOLLOWS THE LIVE
# RUN: it re-picks the freshest log every couple of seconds (so it tracks work as
# it moves triage→lane→guardian→audit, instead of freezing on one file), ignores
# stale logs left by a previous run, pretty-prints stream-json via jq when present,
# and EXITS on its own once no progressive-development tool is running (so it never
# orphans). Run it in a second terminal, or let autopilot auto-launch it.
set -uo pipefail
cd "$(dirname "$0")/../.."
base="$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo '?')"

# Which tool is live?
active_tool() { pgrep -f 'progressive-development/(autopilot|orchestrate|run)\.sh' >/dev/null 2>&1; }
tool="idle"
for t in autopilot.sh orchestrate.sh run.sh; do
    pgrep -f "progressive-development/$t" >/dev/null 2>&1 && { tool="$t"; break; }
done
printf '── progressive-development monitor ──\n'
printf 'base=%s   active=%s\n' "$base" "$tool"

# All in-flight branches (run-* / lane-* / guardian-*) + their commits.
mapfile -t brs < <(git branch --list 'progressive-development/*' | tr -d ' *')
if [ "${#brs[@]}" -gt 0 ]; then
    printf 'branches:\n'
    for b in "${brs[@]}"; do
        n="$(git rev-list --count "$base..$b" 2>/dev/null || echo 0)"
        printf '  • %s (+%s)\n' "$b" "$n"
        git log --oneline "$base..$b" 2>/dev/null | sed 's/^/      /'
    done
fi

# Autopilot queue (pending items) + digest + escalations, if present.
q="docs/architecture/progressive-development-queue.tsv"
[ -s "$q" ] && { printf 'queue (pending):\n'; awk -F'\t' '{st[$3]=$1;kd[$3]=$2}
    END{for(d in st) if(st[d]=="PENDING") printf "  ? [%s] %s\n", kd[d], substr(d,1,80)}' "$q"; }
for f in docs/architecture/progressive-development-escalations.md docs/architecture/progressive-development-digest.md; do
    [ -s "$f" ] && { printf '%s:\n' "$(basename "$f")"; tail -8 "$f" | sed 's/^/  /'; }
done
printf '\n'

# Newest log across ALL families: iter (run), lane (orchestrate), autopilot
# (triage/audit/fuzz/guardian). Freshness-gated: a log NOT modified in the last
# 5 min is treated as stale (a previous run's leftover) and skipped, so we never
# freeze on an old file the way a one-shot `tail -f` used to.
newest_log() {
    local f
    f="$(ls -t docs/architecture/progressive-development-iter-*.log \
              docs/architecture/progressive-development-lane-*.log \
              /tmp/autopilot-*.log 2>/dev/null | head -1)"
    [ -n "$f" ] && [ -n "$(find "$f" -mmin -5 2>/dev/null)" ] && echo "$f"
}

# tag + indent by source, so the process TREE is visible: the master
# (autopilot/run heartbeat) is on terminal 1; here each line is tagged + indented
# by depth — guardian/triage/audit/fuzz are subordinates (autopilot agents), a
# lane (orchestrate → lane, itself under autopilot) is deeper still.
label_for() {
    case "$1" in
        progressive-development-lane-*)          tag="lane ${1//[^0-9]/}"; ind="        " ;;
        progressive-development-iter-*)          tag="iter ${1//[^0-9]/}"; ind="    "     ;;
        autopilot-guardian-*)                    tag="guardian";           ind="    "     ;;
        autopilot-triage-*)                      tag="triage";             ind="    "     ;;
        autopilot-audit-*)                       tag="audit";              ind="    "     ;;
        autopilot-fuzz-*)                        tag="fuzz";               ind="    "     ;;
        autopilot-reconcile-*|orch-reconcile-*)  tag="reconcile";          ind="        " ;;
        *)                                       tag="${1%.log}";          ind="    "     ;;
    esac
}

# Follow a log in the background; pretty-print stream-json via jq, else raw-tail.
TAIL_PID=""
stop_tail() { [ -n "$TAIL_PID" ] && { kill "$TAIL_PID" 2>/dev/null; pkill -P "$TAIL_PID" 2>/dev/null; }; TAIL_PID=""; }
trap 'stop_tail; exit 0' INT TERM EXIT
follow() {
    local logf="$1" tag ind; label_for "$(basename "$logf")"
    printf '\n── following %s  as ↳[%s]  (indent = subordinate depth; Ctrl-C to stop) ──\n' "$(basename "$logf")" "$tag"
    if command -v jq >/dev/null 2>&1 && head -c1 "$logf" 2>/dev/null | grep -q '{'; then
        ( tail -n +1 -f "$logf" | jq -Rr 'fromjson?
          | if .type=="assistant" then (.message.content[]? |
                if .type=="text"      then "💬 " + .text
                elif .type=="tool_use" then "🔧 " + .name + ": " + ((.input|tostring)[0:160])
                else empty end)
            elif .type=="user" then (.message.content[]? |
                if .type=="tool_result" then "   ↳ " + (( .content
                    | if type=="array" then (map(.text // (.|tostring)) | join(" ")) else tostring end)[0:160])
                else empty end)
            elif .type=="result" then "✅ " + ((.result // .subtype // "done") | tostring)
            else empty end' | sed "s/^/${ind}↳[${tag}] /" ) &
    else
        ( tail -n +1 -f "$logf" | sed "s/^/${ind}↳[${tag}] /" ) &
    fi
    TAIL_PID=$!
}

# Supervisor loop: re-pick the freshest live log; switch when work moves; exit
# when the run is over (a couple of grace passes to bridge between-phase gaps).
echo "── monitoring the live run (auto-follows the newest log; exits when the run ends) ──"
cur=""; idle=0
while :; do
    if active_tool; then idle=0; else
        idle=$((idle+1))
        [ "$idle" -ge 3 ] && { stop_tail; echo "── no progressive-development tool running — monitor done ──"; exit 0; }
    fi
    n="$(newest_log)"
    if [ -n "$n" ] && [ "$n" != "$cur" ]; then stop_tail; cur="$n"; follow "$cur"; fi
    sleep 2
done
