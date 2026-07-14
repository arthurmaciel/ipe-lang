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
#  · A PINNED header (scroll-region) shows, top to bottom: the current task /
#    type / attempt / phase / model (from docs/architecture/progdev-status.txt)
#    with a spinner + idle-seconds that advance every tick; a situational-
#    awareness strip (daemon guards + backlog counts, active worktrees/lanes,
#    live log files, lane-guard STALLED warnings) — so "working" and "healthy"
#    are both obvious and the header never scrolls away. Set WATCH_PIN=0 to
#    fall back to plain scroll.
#  · The situational-awareness strip is the SAME data on both paths: pinned
#    (WATCH_PIN=1, redrawn header rows) or plain (WATCH_PIN=0 / non-tty,
#    periodic scrolling snapshot blocks) — one set of compute_status_lines()
#    globals feeds both renderers so they can't drift apart.
set -uo pipefail
SELFDIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SELFDIR/../.."
b="$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo '?')"
STATUS="docs/architecture/progdev-status.txt"
BACKLOG_JSONL="scripts/progressive-development/backlog.jsonl"
LANE_GUARD_LOG_FILE="${LANE_GUARD_LOG:-/tmp/lane-guard.log}"
RENDER="$SELFDIR/render-stream.sh"
PIN="${WATCH_PIN:-1}"; [ -t 1 ] || PIN=0     # pinned header needs a tty

if [ -t 1 ]; then
    C0=$'\033[0m'; TAGC=$'\033[36m'; TOOLC=$'\033[33m'
    RESC=$'\033[32m'; DIMC=$'\033[90m'; HDRC=$'\033[1;34m'; BANC=$'\033[1;36m'
    WARNC=$'\033[1;31m'
else
    C0=; TAGC=; TOOLC=; RESC=; DIMC=; HDRC=; BANC=; WARNC=
fi

LOCK=".autopilot.lock"   # autopilot writes its live PID here + removes it on EXIT — authoritative run marker
active_tool() {
    # Primary signal: is the autopilot that owns the lock still alive? This is
    # immune to the cmdline-prefix fragility of matching the script by path — a
    # `bash autopilot.sh` launch (from inside the dir) has no 'progressive-
    # development/' in argv, so the old pgrep saw NOTHING and the monitor quit
    # in 3s while autopilot was mid-8-minute design agent. The lock PID cannot lie.
    local pid
    if pid="$(cat "$LOCK" 2>/dev/null)" && [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
        return 0
    fi
    # Fallback for orchestrate.sh / run.sh (they hold no autopilot lock) or a
    # path-launched autopilot whose lock we somehow can't read.
    pgrep -f 'progressive-development/(autopilot|orchestrate|run)\.sh' >/dev/null 2>&1
}
now_hm()    { date +%H:%M; }
mtime()     { stat -c %Y "$1" 2>/dev/null || echo 0; }
term_rows() { tput lines 2>/dev/null || echo 40; }
term_cols() { tput cols  2>/dev/null || echo 120; }

# ── situational-awareness helpers ──────────────────────────────────────────
# Every external call below is timeout-bounded (non-negotiable #3: a hung
# subprocess must never wedge the monitor). Empirically load-bearing: a
# `git worktree list`-reported "locked" worktree observed `git log -1`
# taking ~3.8s vs ~20ms for a healthy one — one bad lane must not stall the
# spinner/redraw for everyone.
SUBPROC_TIMEOUT="${WATCH_SUBPROC_TIMEOUT:-2}"

# Guard against pgrep matching itself: pgrep -f already excludes its own PID,
# so these are safe to call every refresh tick without self-matching noise.
guard_state() { timeout "$SUBPROC_TIMEOUT" pgrep -f "$1" >/dev/null 2>&1 && echo UP || echo DOWN; }
free_disk_gb() { timeout "$SUBPROC_TIMEOUT" df -BG --output=avail / 2>/dev/null | tail -1 | tr -dc '0-9'; }

human_ago() {   # <epoch-seconds> -> "Ns"/"Nm"/"Nh"
    local t="${1:-0}" now diff
    now="$(date +%s)"; diff=$(( now - t ))
    [ "$diff" -lt 0 ] && diff=0
    if   [ "$diff" -lt 60 ];   then echo "${diff}s"
    elif [ "$diff" -lt 3600 ]; then echo "$((diff/60))m"
    else                            echo "$((diff/3600))h"
    fi
}

# backlog.jsonl: counts by status + which ids are currently claimed (those
# correspond to in-flight lanes). Degrades to "n/a" when jq or the file is
# missing rather than erroring the whole monitor.
backlog_line() {
    if ! { [ -s "$BACKLOG_JSONL" ] && command -v jq >/dev/null 2>&1; }; then
        printf 'backlog: n/a'; return 0
    fi
    local counts claimed
    counts="$(timeout "$SUBPROC_TIMEOUT" jq -r 'select(has("id")) | .status // "unknown"' "$BACKLOG_JSONL" 2>/dev/null \
        | sort | uniq -c | awk '{printf "%s=%s ", $2, $1}')"
    claimed="$(timeout "$SUBPROC_TIMEOUT" jq -r 'select(has("id")) | select(.status=="claimed") | .id' "$BACKLOG_JSONL" 2>/dev/null \
        | paste -sd, - 2>/dev/null)"
    if [ -n "$claimed" ]; then
        printf 'backlog: %sclaimed:[%s]' "$counts" "$claimed"
    else
        printf 'backlog: %s' "${counts:-?}"
    fi
}

# git worktree list --porcelain -> "<lane-name>(<branch>,<last-commit-ago>) ..."
# for every worktree under .claude/worktrees (skips the main checkout).
worktree_line() {
    local wt br name rel out=""
    while IFS=$'\t' read -r wt br; do
        [ -n "$wt" ] || continue
        case "$wt" in *".claude/worktrees/"*) ;; *) continue ;; esac
        name="$(basename "$wt")"; name="${name#agent-}"
        rel="$(timeout "$SUBPROC_TIMEOUT" git -C "$wt" log -1 --format=%cr 2>/dev/null)"
        [ -n "$rel" ] || rel="?(slow/locked)"
        out+="${name}(${br:-?},${rel}) "
    done < <(timeout "$SUBPROC_TIMEOUT" git worktree list --porcelain 2>/dev/null | awk '
        /^worktree /{ if (w!="") print w"\t"(b==""?"detached":b); w=$2; b="" }
        /^branch /{ b=$2; sub("refs/heads/","",b) }
        END{ if (w!="") print w"\t"(b==""?"detached":b) }')
    printf '%s' "$out"
}

# Every currently-live log file (matching the SAME globs newest_log() uses,
# so "live" here means the same freshness contract), one entry per lane:
# "<tag>[*]:<age-ago>" — '*' marks the log currently full-tailed below.
live_logs_summary() {
    local f base age mark out=()
    while IFS= read -r f; do
        [ -n "$f" ] || continue
        [ -n "$(find "$f" -mmin -10 2>/dev/null)" ] || continue
        base="$(basename "$f")"
        tag_for "$base"          # sets global $tag
        age="$(human_ago "$(mtime "$f")")"
        mark=""; [ "$f" = "${cur:-}" ] && mark="*"
        out+=("${tag}${mark}:${age}")
    done < <(timeout "$SUBPROC_TIMEOUT" ls -t docs/architecture/progressive-development-*.log /tmp/autopilot-*.log 2>/dev/null)
    printf '%s ' "${out[@]:-}"
}

# Recent lane-guard STALLED lines (raw, timestamp-prefixed), newest last —
# lane-guard.sh logs one `STALLED <lane> ...` line per poll when a lane's
# git-state fingerprint hasn't moved AND no live process references it.
stalled_lines_multi() {
    [ -s "$LANE_GUARD_LOG_FILE" ] || return 0
    timeout "$SUBPROC_TIMEOUT" tail -n 200 "$LANE_GUARD_LOG_FILE" 2>/dev/null | grep -F 'STALLED' | tail -n 3
}

# Populates GL_L3 (guards + backlog), GL_L4 (worktrees), GL_L5 (live logs),
# GL_L6 (condensed stalled-warning, single line), GL_STALLED_RAW (multi-line
# raw form for the plain/scrolling renderer), STALL_ACTIVE (0/1, color gate).
compute_status_lines() {
    local mem_state disk_state lane_state free
    mem_state="$(guard_state 'scripts/mem-guard\.sh')"
    disk_state="$(guard_state 'scripts/disk-guard\.sh')"
    lane_state="$(guard_state 'scripts/lane-guard\.sh')"
    free="$(free_disk_gb)"
    GL_L3="guards: mem-guard=${mem_state} disk-guard=${disk_state}(free=${free:-?}G) lane-guard=${lane_state}   $(backlog_line)"
    GL_L4="worktrees: $(worktree_line)"
    GL_L5="logs: $(live_logs_summary)"
    GL_STALLED_RAW="$(stalled_lines_multi)"
    if [ -n "$GL_STALLED_RAW" ]; then
        STALL_ACTIVE=1
        GL_L6="STALLED: $(printf '%s\n' "$GL_STALLED_RAW" | sed 's/^\[[^]]*\] //' | paste -sd '|' -)"
    else
        STALL_ACTIVE=0
        GL_L6="lane-guard: no stalled lanes"
    fi
}

# Plain (non-pinned) rendering of the same status data — scrolls, used at
# startup always, and periodically while PIN=0 (non-tty / WATCH_PIN=0).
print_status_block() {
    compute_status_lines
    printf '  %s-- status @ %s --%s\n' "$DIMC" "$(now_hm)" "$C0"
    printf '  %s\n' "$GL_L3"
    printf '  %s\n' "$GL_L4"
    printf '  %s\n' "$GL_L5"
    if [ "$STALL_ACTIVE" = 1 ]; then
        printf '  %s-- lane-guard STALLED --%s\n' "$WARNC" "$C0"
        printf '%s\n' "$GL_STALLED_RAW" | sed 's/^/    /'
        printf '%s' "$C0"
    else
        printf '  %s%s%s\n' "$DIMC" "$GL_L6" "$C0"
    fi
    printf '\n'
}

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
cur=""
print_status_block

# ── pinned header machinery (scroll-region) ───────────────────────────────────
FRAMES='⠋ ⠙ ⠹ ⠸ ⠼ ⠴ ⠦ ⠧ ⠇ ⠏'; read -r -a SPIN <<< "$FRAMES"; spin_i=0
LINES_N="$(term_rows)"
HEADER_LINES=6      # l1 spinner/phase, l2 task, l3 guards+backlog, l4 worktrees, l5 logs, l6 stalled
SLOW_REFRESH_SEC=5  # situational-awareness data (git/jq/pgrep/df) redraws at most this often
slow_last=0

setup_pane() {
    [ "$PIN" = 1 ] || return 0
    LINES_N="$(term_rows)"
    printf '\033[2J\033[H'                          # clear
    printf '\033[%d;%dr' "$((HEADER_LINES + 1))" "$LINES_N"   # scroll region below header
    printf '\033[%d;1H' "$((HEADER_LINES + 1))"     # park cursor in the region
    printf '\033[?25l'                              # hide cursor (header redraw is cleaner)
}
draw_header() {
    [ "$PIN" = 1 ] || return 0
    local f="${SPIN[$((spin_i % ${#SPIN[@]}))]}"; spin_i=$((spin_i+1))
    local task type att phase model idle cols l1 l2 now_ts
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

    now_ts="$(date +%s)"
    if [ $(( now_ts - slow_last )) -ge "$SLOW_REFRESH_SEC" ] || [ -z "${GL_L3:-}" ]; then
        compute_status_lines
        slow_last="$now_ts"
    fi

    printf '\0337'                                          # save cursor
    printf '\033[1;1H\033[2K%s%.*s%s' "$BANC" "$((cols-1))" "$l1" "$C0"
    printf '\033[2;1H\033[2K%s%.*s%s' "$DIMC" "$((cols-1))" "$l2" "$C0"
    printf '\033[3;1H\033[2K%s%.*s%s' "$DIMC" "$((cols-1))" "$GL_L3" "$C0"
    printf '\033[4;1H\033[2K%s%.*s%s' "$DIMC" "$((cols-1))" "$GL_L4" "$C0"
    printf '\033[5;1H\033[2K%s%.*s%s' "$DIMC" "$((cols-1))" "$GL_L5" "$C0"
    if [ "$STALL_ACTIVE" = 1 ]; then
        printf '\033[6;1H\033[2K%s%.*s%s' "$WARNC" "$((cols-1))" "$GL_L6" "$C0"
    else
        printf '\033[6;1H\033[2K%s%.*s%s' "$DIMC" "$((cols-1))" "$GL_L6" "$C0"
    fi
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
cur=""; idle=0; plain_last=0
while :; do
    if active_tool; then idle=0; else
        idle=$((idle+1))
        [ "$idle" -ge 4 ] && { teardown; printf '  %s-- run ended - monitor done --%s\n' "$DIMC" "$C0"; exit 0; }
    fi
    n="$(newest_log)"
    if [ -n "$n" ] && [ "$n" != "$cur" ]; then stop_tail; cur="$n"; follow "$cur"; fi
    if [ "$PIN" = 0 ]; then
        now_ts="$(date +%s)"
        if [ $(( now_ts - plain_last )) -ge 15 ]; then
            print_status_block
            plain_last="$now_ts"
        fi
    fi
    draw_header
    sleep 1
done
