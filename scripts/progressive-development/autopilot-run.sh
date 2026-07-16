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
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

AP="$(dirname "${BASH_SOURCE[0]}")/autopilot.sh"
[ -x "$AP" ] || { echo "autopilot-run: $AP missing or not executable" >&2; exit 1; }

# Pass --help / -h straight through (autopilot prints its own reference).
case "${1:-}" in -h|--help) exec "$AP" --help ;; esac

# Clear a leftover graceful-stop flag from a previous run (else autopilot's
# startup precondition would refuse to launch). ./autopilot-stop.sh sets it.
[ -f "$REPO/autopilot.stop" ] && { rm -f "$REPO/autopilot.stop"; echo "autopilot-run: cleared stale autopilot.stop"; }

# Runs until DONE: autopilot converges on its own when nothing tractable remains
# (2 passes with no new findings; escalated/blocked items are suppressed so they
# can't spin it). Override caps only if you want to via PROGDEV_MAX_GUARDIAN /
# PROGDEV_MAX_CYCLES; touch autopilot.stop to halt after the current cycle.
echo "autopilot-run: running until done - stops when no tractable work remains. watch auto-attaches; Ctrl-C stops."

exec "$AP" "$@"
