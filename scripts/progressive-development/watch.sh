#!/usr/bin/env bash
# watch.sh - live monitor for progressive-development (run/orchestrate/autopilot).
# Follows the freshest live log, re-picks as work moves, self-terminates when the
# run ends. Flat 2-space margin, tmux-safe colors, no raw json, "> " marks tools.
set -uo pipefail
cd "$(dirname "$0")/../.."
b="$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo '?')"

# tmux-safe ANSI (basic colors; gated on a tty so redirects stay clean).
if [ -t 1 ]; then
    C0=$'\033[0m'; TAGC=$'\033[36m'; TOOLC=$'\033[33m'
    RESC=$'\033[32m'; DIMC=$'\033[90m'; HDRC=$'\033[1;34m'
else
    C0=; TAGC=; TOOLC=; RESC=; DIMC=; HDRC=
fi

active_tool() { pgrep -f 'progressive-development/(autopilot|orchestrate|run)\.sh' >/dev/null 2>&1; }

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

# Freshest live log; skip stale prior-run leftovers (>5 min old).
newest_log() {
    local f
    f="$(ls -t docs/architecture/progressive-development-iter-*.log \
              docs/architecture/progressive-development-lane-*.log \
              /tmp/autopilot-*.log 2>/dev/null | head -1)"
    [ -n "$f" ] && [ -n "$(find "$f" -mmin -5 2>/dev/null)" ] && echo "$f"
}
tag_for() {
    case "$1" in
        progressive-development-lane-*)          tag="lane${1//[^0-9]/}" ;;
        progressive-development-iter-*)          tag="iter${1//[^0-9]/}" ;;
        autopilot-guardian-*)                    tag="guardian" ;;
        autopilot-triage-*)                      tag="triage" ;;
        autopilot-audit-*)                       tag="audit" ;;
        autopilot-fuzz-*)                        tag="fuzz" ;;
        autopilot-reconcile-*|orch-reconcile-*)  tag="reconcile" ;;
        *)                                       tag="${1%.log}" ;;
    esac
}

TAIL_PID=""
stop_tail() { [ -n "$TAIL_PID" ] && { kill "$TAIL_PID" 2>/dev/null; pkill -P "$TAIL_PID" 2>/dev/null; }; TAIL_PID=""; }
trap 'stop_tail; exit 0' INT TERM EXIT

follow() {
    local logf="$1" tag; tag_for "$(basename "$logf")"
    printf '  %s-- %s [%s] --%s\n' "$DIMC" "$(basename "$logf")" "$tag" "$C0"
    if command -v jq >/dev/null 2>&1 && head -c1 "$logf" 2>/dev/null | grep -q '{'; then
        # stream-json -> one compact line per step; unhandled/raw lines drop to empty.
        ( tail -n 20 -f "$logf" | jq -R -j --unbuffered \
            --arg tag "$tag" --arg c0 "$C0" --arg tagc "$TAGC" \
            --arg toolc "$TOOLC" --arg resc "$RESC" --arg dimc "$DIMC" '
          ( fromjson? | objects
            | if .type=="assistant" then ( .message.content[]? |
                  if   .type=="text" and (.text|length>0) then ((.text|gsub("\n";" "))[0:400])
                  elif .type=="tool_use" then $toolc + "> " + .name + $c0 + " " + (((.input.command // .input.file_path // .input.path // .input.pattern // .input.description // (.input|tostring))|tostring|gsub("\n";" "))[0:120])
                  else empty end )
              elif .type=="user" then ( .message.content[]? |
                  if .type=="tool_result" then $dimc + "  " + (((.content | if type=="string" then . else (map(.text? // "")|join(" ")) end)|gsub("\n";" "))[0:100]) + $c0
                  else empty end )
              elif .type=="result" then $resc + "* " + ((.result // .subtype // "done")|tostring|gsub("\n";" "))[0:200] + $c0
              else empty end )
          | "  " + $tagc + "[" + $tag + "]" + $c0 + " " + . + "\n" ' ) &
    else
        ( tail -n 20 -f "$logf" | sed "s/^/  ${TAGC}[${tag}]${C0} /" ) &
    fi
    TAIL_PID=$!
}

printf '  %s-- monitoring (auto-follows newest log; exits when run ends; Ctrl-C) --%s\n' "$DIMC" "$C0"
cur=""; idle=0
while :; do
    if active_tool; then idle=0; else
        idle=$((idle+1))
        [ "$idle" -ge 3 ] && { stop_tail; printf '  %s-- run ended - monitor done --%s\n' "$DIMC" "$C0"; exit 0; }
    fi
    n="$(newest_log)"
    if [ -n "$n" ] && [ "$n" != "$cur" ]; then stop_tail; cur="$n"; follow "$cur"; fi
    sleep 2
done
