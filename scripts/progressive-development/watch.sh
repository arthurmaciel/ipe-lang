#!/usr/bin/env bash
# watch.sh - live monitor for progressive-development (run/orchestrate/autopilot).
# Follows the freshest live log, re-picks as work moves, self-terminates when the
# run ends.
#
# Liveness UX:
#  · Rendering is delegated to render-stream.sh (ONE source; un-clipped, indented,
#    non-json echoed, system/thinking events dropped). watch ALWAYS pipes through
#    it — no first-byte json guess that used to race an empty new log and then
#    print raw json for the whole file.
#  · A PINNED 2-line header (scroll-region) shows the current task / type /
#    attempt / phase / model (from docs/architecture/progdev-status.txt) with a
#    spinner + idle-seconds that advance every tick — so "working" is obvious and
#    the header never scrolls away. Set WATCH_PIN=0 to fall back to plain scroll.
set -uo pipefail
SELFDIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SELFDIR/../.."
b="$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo '?')"
STATUS="docs/architecture/progdev-status.txt"
RENDER="$SELFDIR/render-stream.sh"
PIN="${WATCH_PIN:-1}"; [ -t 1 ] || PIN=0     # pinned header needs a tty

if [ -t 1 ]; then
    C0=$'\033[0m'; TAGC=$'\033[36m'; TOOLC=$'\033[33m'
    RESC=$'\033[32m'; DIMC=$'\033[90m'; HDRC=$'\033[1;34m'; BANC=$'\033[1;36m'
else
    C0=; TAGC=; TOOLC=; RESC=; DIMC=; HDRC=; BANC=
fi

active_tool() { pgrep -f 'progressive-development/(autopilot|orchestrate|run)\.sh' >/dev/null 2>&1; }
now_hm()    { date +%H:%M; }
mtime()     { stat -c %Y "$1" 2>/dev/null || echo 0; }
term_rows() { tput lines 2>/dev/null || echo 40; }
term_cols() { tput cols  2>/dev/null || echo 120; }

# ── startup summary (scrolls; transient orientation) ──────────────────────────
tool="idle"
for t in autopilot.sh orchestrate.sh run.sh; do
    pgrep -f "progressive-development/$t" >/dev/null 2>&1 && { tool="$t"; break; }
done
printf '  %s-- progressive-development monitor --%s\n' "$HDRC" "$C0"
printf '  base=%s  active=%s\n' "$b" "$tool"
q="docs/architecture/progressive-development-queue.tsv"
[ -s "$q" ] && { printf '  queue (pending):\n'; awk -F'\t' '{st[$3]=$1;kd[$3]=$2}
    END{for(d in st) if(st[d]=="PENDING") printf "    [%s] %s\n", kd[d], substr(d,1,66)}' "$q"; }
printf '\n'

# ── pinned header machinery (scroll-region) ───────────────────────────────────
FRAMES='⠋ ⠙ ⠹ ⠸ ⠼ ⠴ ⠦ ⠧ ⠇ ⠏'; read -r -a SPIN <<< "$FRAMES"; spin_i=0
LINES_N="$(term_rows)"

setup_pane() {
    [ "$PIN" = 1 ] || return 0
    LINES_N="$(term_rows)"
    printf '\033[2J\033[H'              # clear
    printf '\033[3;%dr' "$LINES_N"      # scroll region = rows 3..bottom (2-line header)
    printf '\033[3;1H'                  # park cursor in the region
    printf '\033[?25l'                  # hide cursor (header redraw is cleaner)
}
draw_header() {
    [ "$PIN" = 1 ] || return 0
    local f="${SPIN[$((spin_i % ${#SPIN[@]}))]}"; spin_i=$((spin_i+1))
    local task type att phase model idle cols l1 l2
    if [ -s "$STATUS" ]; then
        task="$(awk '/^task/{ $1=""; sub(/^ +/,""); print; exit}' "$STATUS")"
        type="$(awk '/^type/{print $2; exit}' "$STATUS")"
        att="$(awk '/^type/{for(i=1;i<=NF;i++) if($i=="attempt"){print $(i+1); exit}}' "$STATUS")"
        phase="$(awk '/^phase/{print $2; exit}' "$STATUS")"
        model="$(awk '/^phase/{for(i=1;i<=NF;i++) if($i=="model"){print $(i+1); exit}}' "$STATUS")"
    fi
    idle='-'; [ -n "${cur:-}" ] && idle=$(( $(date +%s) - $(mtime "$cur") ))
    cols="$(term_cols)"
    l1="$f ${phase:-·}/${model:-·} · ${type:-·} · attempt ${att:-·} · idle ${idle}s · $(now_hm)"
    l2="task: ${task:-·}"
    printf '\0337'                                          # save cursor
    printf '\033[1;1H\033[2K%s%.*s%s' "$BANC" "$((cols-1))" "$l1" "$C0"
    printf '\033[2;1H\033[2K%s%.*s%s' "$DIMC" "$((cols-1))" "$l2" "$C0"
    printf '\0338'                                          # restore cursor
}
teardown() {
    stop_tail
    [ "$PIN" = 1 ] && printf '\033[r\033[?25h\033[%d;1H\n' "$LINES_N"   # reset region, show cursor
}

TAIL_PID=""
stop_tail() {
    [ -n "$TAIL_PID" ] && { kill "$TAIL_PID" 2>/dev/null; pkill -P "$TAIL_PID" 2>/dev/null; }
    [ -n "${cur:-}" ] && pkill -f "tail -n 20 -f ${cur}" 2>/dev/null
    TAIL_PID=""
}
trap 'teardown; exit 0' INT TERM EXIT
trap 'LINES_N="$(term_rows)"; setup_pane' WINCH

# ── freshest live log (broadened glob; skip stale >5min) ──────────────────────
newest_log() {
    local f
    f="$(ls -t docs/architecture/progressive-development-*.log \
              /tmp/autopilot-*.log 2>/dev/null | head -1)"
    [ -n "$f" ] && [ -n "$(find "$f" -mmin -5 2>/dev/null)" ] && echo "$f"
}
tag_for() {
    case "$1" in
        progressive-development-lane-*)          tag="lane${1//[^0-9]/}" ;;
        progressive-development-iter-*)          tag="iter${1//[^0-9]/}" ;;
        autopilot-guardian-design-*)             tag="design" ;;
        autopilot-guardian-review-*)             tag="review" ;;
        autopilot-guardian-*)                    tag="impl" ;;
        autopilot-triage-*)                      tag="triage" ;;
        autopilot-audit-*)                       tag="audit" ;;
        autopilot-fuzz-*)                        tag="fuzz" ;;
        autopilot-gate*)                         tag="gate" ;;
        autopilot-reconcile-*|orch-reconcile-*)  tag="reconcile" ;;
        *)                                       tag="${1%.log}" ;;
    esac
}
# ALWAYS render through render-stream.sh (handles json AND plain; no race).
follow() {
    local logf="$1" tag; tag_for "$(basename "$logf")"
    printf '  %s-- %s [%s] --%s\n' "$DIMC" "$(basename "$logf")" "$tag" "$C0"
    ( tail -n 20 -f "$logf" | "$RENDER" "$tag" ) &
    TAIL_PID=$!
}

setup_pane
printf '  %s-- monitoring (auto-follows newest log; exits when run ends; Ctrl-C) --%s\n' "$DIMC" "$C0"
cur=""; idle=0
while :; do
    if active_tool; then idle=0; else
        idle=$((idle+1))
        [ "$idle" -ge 3 ] && { teardown; printf '  %s-- run ended - monitor done --%s\n' "$DIMC" "$C0"; exit 0; }
    fi
    n="$(newest_log)"
    if [ -n "$n" ] && [ "$n" != "$cur" ]; then stop_tail; cur="$n"; follow "$cur"; fi
    draw_header
    sleep 1
done
