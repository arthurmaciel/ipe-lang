#!/usr/bin/env bash
# autopilot-run.sh — one-command launcher for the autonomous development loop.
#
#   ./autopilot-run.sh            supervised first-run: 1 cycle, 1 guardian item,
#                                 live watch.sh auto-attached (Ctrl-C stops both)
#   ./autopilot-run.sh --full     full run using autopilot's native caps
#                                 (6 cycles, 2 guardian items)
#   ./autopilot-run.sh --no-watch [...]   any autopilot flag passes straight through
#
# Convenience wrapper around scripts/progressive-development/autopilot.sh so you
# don't have to type the PROGDEV_* env prefixes. It also guarantees mem-guard.sh
# (the memory kill-switch autopilot HARD-requires) is running before it starts.
set -uo pipefail
cd "$(dirname "$0")"

AP="scripts/progressive-development/autopilot.sh"
[ -x "$AP" ] || { echo "autopilot-run: $AP missing or not executable" >&2; exit 1; }

# Pass --help / -h straight through (autopilot prints its own reference).
case "${1:-}" in -h|--help) exec "$AP" --help ;; esac

# mem-guard is a HARD precondition of autopilot — start it if it isn't up.
if ! pgrep -f mem-guard.sh >/dev/null 2>&1; then
    echo "autopilot-run: mem-guard.sh not running — starting it (memory kill-switch)…"
    nohup ./scripts/mem-guard.sh >/tmp/mem-guard.out 2>&1 & disown
    sleep 1
fi

# Cap mode: default = supervised (1/1); --full = autopilot's native caps (6/2).
if [ "${1:-}" = "--full" ]; then
    shift
    echo "autopilot-run: FULL run (autopilot native caps) — watch auto-attaches; Ctrl-C stops."
else
    export PROGDEV_MAX_CYCLES="${PROGDEV_MAX_CYCLES:-1}"
    export PROGDEV_MAX_GUARDIAN="${PROGDEV_MAX_GUARDIAN:-1}"
    echo "autopilot-run: supervised run (cycles=$PROGDEV_MAX_CYCLES guardian=$PROGDEV_MAX_GUARDIAN) — watch auto-attaches; Ctrl-C stops. Use --full for a longer run."
fi

exec "$AP" "$@"
