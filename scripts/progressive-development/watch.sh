#!/usr/bin/env bash
# watch.sh — live monitor for ANY progressive-development activity:
# run.sh (single/loop), orchestrate.sh (parallel lanes), or autopilot.sh.
#
#   scripts/progressive-development/watch.sh
#
# Prints a header (which tool is running, all progressive-development/* branches +
# their commits, the autopilot queue, escalations/digest), then follows the
# NEWEST log across every family — iteration (run), lane (orchestrate), and
# autopilot (triage/audit/guardian) — pretty-printing stream-json steps via jq
# when present, else raw-tailing. Run it in a second terminal.
set -uo pipefail
cd "$(dirname "$0")/../.."
base="$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo '?')"

# Which tool is live?
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

# Newest log across ALL families: iter (run), lane (orchestrate), autopilot (triage/audit/guardian).
newest_log() {
    ls -t docs/architecture/progressive-development-iter-*.log \
          docs/architecture/progressive-development-lane-*.log \
          /tmp/autopilot-*.log 2>/dev/null | head -1
}
logf=""
for _ in $(seq 1 30); do logf="$(newest_log)"; [ -n "$logf" ] && break; sleep 1; done
if [ -z "$logf" ]; then
    echo "(no activity log yet — is a tool running?  pgrep -f progressive-development/)"; exit 0
fi

# Label + INDENT the stream by its source, so the process tree is visible: the
# master (autopilot/run heartbeat) is on terminal 1; here each line is tagged +
# indented by depth — guardian/triage/audit are subordinates (autopilot agents),
# and a lane (orchestrate → lane, itself under autopilot) is deeper still.
base="$(basename "$logf")"
case "$base" in
    progressive-development-lane-*) tag="lane ${base//[^0-9]/}"; ind="        " ;;  # sub-sub
    progressive-development-iter-*) tag="iter ${base//[^0-9]/}"; ind="    "     ;;  # sub (run.sh)
    autopilot-guardian-*)           tag="guardian";              ind="    "     ;;  # sub (autopilot agent)
    autopilot-triage-*)             tag="triage";                ind="    "     ;;
    autopilot-audit-*)              tag="audit";                 ind="    "     ;;
    autopilot-reconcile-*)          tag="reconcile";             ind="        " ;;
    *)                              tag="${base%.log}";          ind="    "     ;;
esac
printf '── following %s  as ↳[%s]  (indent = subordinate depth; Ctrl-C to stop) ──\n' "$base" "$tag"
render() { sed "s/^/${ind}↳[${tag}] /"; }
if command -v jq >/dev/null 2>&1 && head -c1 "$logf" 2>/dev/null | grep -q '{'; then
    tail -n +1 -f "$logf" | jq -Rr 'fromjson?
      | if .type=="assistant" then (.message.content[]? |
            if .type=="text"      then "💬 " + .text
            elif .type=="tool_use" then "🔧 " + .name + ": " + ((.input|tostring)[0:160])
            else empty end)
        elif .type=="user" then (.message.content[]? |
            if .type=="tool_result" then "   ↳ " + (( .content
                | if type=="array" then (map(.text // (.|tostring)) | join(" ")) else tostring end)[0:160])
            else empty end)
        elif .type=="result" then "✅ " + ((.result // .subtype // "done") | tostring)
        else empty end' | render
else
    tail -n +1 -f "$logf" | render
fi
