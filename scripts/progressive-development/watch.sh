#!/usr/bin/env bash
# watch.sh — live monitor for a progressive-development run.
#
#   scripts/progressive-development/watch.sh
#
# Prints a header (current run branch + landed commits + escalations), then
# follows the newest iteration log live. When the run used PROGDEV_STREAM=1 the
# iter-log is stream-json, so this pretty-prints each step (reasoning / tool
# calls / result) via jq; otherwise it raw-tails (text mode only fills the log
# when the iteration ends). Run this in a second terminal while the loop runs.
set -uo pipefail
cd "$(dirname "$0")/../.."

base="$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo '?')"
branch="$(git branch --list 'progressive-development/run-*' | tr -d ' *' | tail -1)"

printf '── progressive-development monitor ──\n'
printf 'base=%s  branch=%s  loop=%s\n' \
    "$base" "${branch:-<none yet>}" \
    "$(pgrep -f 'progressive-development/run.sh' >/dev/null && echo running || echo 'not running')"

if [ -n "${branch:-}" ]; then
    n="$(git log --oneline "master..$branch" 2>/dev/null | wc -l | tr -d ' ')"
    printf 'landed commits (%s):\n' "$n"
    git log --oneline "master..$branch" 2>/dev/null | sed 's/^/  • /'
fi

esc="docs/architecture/progressive-development-escalations.md"
[ -s "$esc" ] && { printf 'escalations:\n'; sed 's/^/  ! /' "$esc"; }
printf '\n'

# Newest iteration log — wait briefly for it to appear if a run just started.
iterlog=""
for _ in $(seq 1 30); do
    iterlog="$(ls -t docs/architecture/progressive-development-iter-*.log 2>/dev/null | head -1)"
    [ -n "$iterlog" ] && break
    sleep 1
done
if [ -z "$iterlog" ]; then
    echo "(no iteration log yet — is a run active?  pgrep -f progressive-development/run.sh)"
    exit 0
fi

printf '── following %s  (Ctrl-C to stop) ──\n' "$iterlog"
if command -v jq >/dev/null 2>&1 && head -c1 "$iterlog" 2>/dev/null | grep -q '{'; then
    # stream-json → pretty-print reasoning / tool calls / result
    tail -n +1 -f "$iterlog" | jq -Rr 'fromjson?
      | if .type=="assistant" then (.message.content[]? |
            if .type=="text"      then "💬 " + .text
            elif .type=="tool_use" then "🔧 " + .name + ": " + ((.input|tostring)[0:160])
            else empty end)
        elif .type=="user" then (.message.content[]? |
            if .type=="tool_result" then "   ↳ " + (( .content
                | if type=="array" then (map(.text // (.|tostring)) | join(" ")) else tostring end)[0:160])
            else empty end)
        elif .type=="result" then "✅ " + ((.result // .subtype // "done") | tostring)
        else empty end'
else
    tail -n +1 -f "$iterlog"
fi
