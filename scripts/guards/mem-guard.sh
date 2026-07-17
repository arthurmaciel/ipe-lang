#!/usr/bin/env bash
# scripts/guards/mem-guard.sh — memory kill-switch for the Ipê (Sky→Rust) dev sessions.
#
# Background: a runaway `cargo`/`rustc`/linker (or `ipe`, or a wedged agent
# session) can pin the machine to swap and force a hard reboot. This watchdog
# polls memory every few seconds and SIGKILLs the heaviest watched process
# before that happens. Linux port of ../sky/scripts/guards/mem-guard.sh: free memory
# comes from /proc/meminfo `MemAvailable` (not macOS `vm_stat`), and the watched
# set is the Rust toolchain (not the Haskell one).
#
# Usage:
#   ./scripts/guards/mem-guard.sh                          # foreground, logs to stderr + /tmp/mem-guard.log
#   nohup ./scripts/guards/mem-guard.sh >/tmp/mem-guard.out 2>&1 & disown   # background for the session
#   MEM_GUARD_PROC_MB=4000 ./scripts/guards/mem-guard.sh   # tighter per-proc cap
#
# Tunables (env vars, all optional):
#   MEM_GUARD_PROC_MB        per-process RSS kill threshold (MB).       default 6000
#   MEM_GUARD_PANIC_MB       claude/node/terminal kill threshold (MB).  default 10000
#   MEM_GUARD_SYS_FLOOR_MB   MemAvailable floor (MB).                   default 1200
#   MEM_GUARD_INTERVAL       poll interval (seconds).                   default 2
#   MEM_GUARD_LOG            log file path.                             default /tmp/mem-guard.log
#   MEM_GUARD_DRY            set to 1 to log only, never kill.          default unset
#
# Watched process names (basename of comm; Linux comm truncates at 15 chars):
#   Always-kill at PROC_MB:  cargo, rustc, cc1, cc1plus, cc, collect2, ld,
#                            ld.lld, lld, lld-link, ipe, sky-ffi-inspect,
#                            rust-analyzer
#   Last-resort at PANIC_MB: claude, node, ghostty (the host of *this* session —
#                            only killed when they themselves are the runaway,
#                            never for a child build blowing out; higher bar).
#
# Never kills PID 1 / kernel threads (they never match the watched set).

set -euo pipefail

PROC_LIMIT_MB="${MEM_GUARD_PROC_MB:-6000}"
PANIC_LIMIT_MB="${MEM_GUARD_PANIC_MB:-10000}"
SYS_FLOOR_MB="${MEM_GUARD_SYS_FLOOR_MB:-1200}"
INTERVAL="${MEM_GUARD_INTERVAL:-2}"
LOG="${MEM_GUARD_LOG:-/tmp/mem-guard.log}"
DRY="${MEM_GUARD_DRY:-}"

# basename(comm) regexes
ALWAYS_KILL_RE='^(cargo|rustc|cc1|cc1plus|cc|collect2|ld|ld\.lld|lld|lld-link|ipe|sky-ffi-inspect|rust-analyzer)$'
PANIC_KILL_RE='^(claude|node|ghostty)$'

log() {
    printf '[%s] %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$*" | tee -a "$LOG" >&2
}

# MemAvailable (kernel's estimate of allocatable memory without swapping), in MB.
# This is the right Linux signal: it already accounts for reclaimable page cache,
# so we don't hand-roll a free+inactive sum the way the macOS version must.
free_mb() {
    awk '/^MemAvailable:/ { printf "%d\n", $2 / 1024; exit }' /proc/meminfo
}

kill_proc() {
    local pid="$1" rss_mb="$2" comm="$3" reason="$4"
    if [[ -n "$DRY" ]]; then
        log "DRY-RUN would kill pid=$pid rss=${rss_mb}MB comm=$comm reason=$reason"
        return
    fi
    log "KILL pid=$pid rss=${rss_mb}MB comm=$comm reason=$reason"
    kill -TERM "$pid" 2>/dev/null || true
    sleep 1   # brief grace; cargo/rustc usually clean up in <1s
    if kill -0 "$pid" 2>/dev/null; then
        log "  pid=$pid ignored SIGTERM, sending SIGKILL"
        kill -KILL "$pid" 2>/dev/null || true
    fi
}

trap 'log "stopping (signal)"; exit 0' INT TERM

log "starting (proc=${PROC_LIMIT_MB}MB panic=${PANIC_LIMIT_MB}MB sys_floor=${SYS_FLOOR_MB}MB poll=${INTERVAL}s dry=${DRY:-no})"

while :; do
    free=$(free_mb)
    pressure=0
    (( free < SYS_FLOOR_MB )) && pressure=1

    # Snapshot all processes by RSS desc. Linux `ps -o rss=` is in KB, `comm=` is
    # the basename already; the split-on-/ is a harmless belt-and-braces.
    snap=$(ps -A -o pid=,rss=,comm= | awk '
        {
            pid = $1; rss = $2; comm = $3;
            n = split(comm, parts, "/");
            print pid, rss, parts[n]
        }
    ' | sort -k2 -rn)

    while read -r pid rss comm; do
        [[ -z "${pid:-}" ]] && continue
        rss_mb=$(( rss / 1024 ))

        if [[ "$comm" =~ $ALWAYS_KILL_RE ]]; then
            if (( rss_mb > PROC_LIMIT_MB )); then
                kill_proc "$pid" "$rss_mb" "$comm" "exceeded per-proc limit ${PROC_LIMIT_MB}MB"
                continue
            fi
            if (( pressure )); then
                kill_proc "$pid" "$rss_mb" "$comm" "MemAvailable=${free}MB below floor=${SYS_FLOOR_MB}MB (heaviest watched)"
                pressure=0  # one kill per pass; recheck next iteration
                continue
            fi
        elif [[ "$comm" =~ $PANIC_KILL_RE ]]; then
            if (( rss_mb > PANIC_LIMIT_MB )); then
                kill_proc "$pid" "$rss_mb" "$comm" "exceeded panic limit ${PANIC_LIMIT_MB}MB"
                continue
            fi
            if (( pressure )) && (( rss_mb > 4000 )); then
                # Only sacrifice the host (claude/terminal) if it is the heaviest
                # AND already over 4GB itself — a build child blowing out is
                # handled by the always-kill branch above first.
                kill_proc "$pid" "$rss_mb" "$comm" "PANIC: MemAvailable=${free}MB and host >4GB"
                pressure=0
                continue
            fi
        fi
    done <<< "$snap"

    sleep "$INTERVAL"
done
