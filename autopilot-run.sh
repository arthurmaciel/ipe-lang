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
# don't have to type the PROGDEV_* env prefixes. (autopilot.sh itself dispatches
# mem-guard.sh when it isn't up, so the launcher doesn't need to.)
set -uo pipefail
cd "$(dirname "$0")"

AP="scripts/progressive-development/autopilot.sh"
[ -x "$AP" ] || { echo "autopilot-run: $AP missing or not executable" >&2; exit 1; }

# Pass --help / -h straight through (autopilot prints its own reference).
case "${1:-}" in -h|--help) exec "$AP" --help ;; esac

# Cap mode: default = supervised (2 cycles / 1 guardian); --full = native (6/2).
# 2 cycles matters on a FIRST run with an empty queue: cycle 1 is spent on
# DISCOVERY (remeasure + fuzz + triage fills the queue), and only cycle 2 can
# ACT on what it found — a 1-cycle run would discover and stop, looking like it
# did nothing. Guardian stays capped at 1 (each is a heavy Opus fix + review).
if [ "${1:-}" = "--full" ]; then
    shift
    echo "autopilot-run: FULL run (autopilot native caps) — watch auto-attaches; Ctrl-C stops."
else
    export PROGDEV_MAX_CYCLES="${PROGDEV_MAX_CYCLES:-2}"
    export PROGDEV_MAX_GUARDIAN="${PROGDEV_MAX_GUARDIAN:-1}"
    echo "autopilot-run: supervised run (cycles=$PROGDEV_MAX_CYCLES guardian=$PROGDEV_MAX_GUARDIAN; cycle 1 discovers, cycle 2 acts) — watch auto-attaches; Ctrl-C stops. Use --full for a longer run."
fi

exec "$AP" "$@"
