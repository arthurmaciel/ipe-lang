#!/usr/bin/env bash
# autopilot-stop.sh - request a GRACEFUL stop of a running autopilot.
# Drops the ./autopilot.stop flag. The loop finishes the current phase (the
# in-flight agent + its verdict), then stops at the next boundary - never
# mid-agent. autopilot-run.sh clears the flag automatically on the next launch.
set -uo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
touch "$REPO/autopilot.stop"
if pgrep -f 'progressive-development/autopilot' >/dev/null 2>&1; then
    echo "autopilot-stop: flag set - autopilot stops after the current phase finishes (no interrupt)."
else
    echo "autopilot-stop: flag set, but no autopilot is running. It would block the next startup;"
    echo "               autopilot-run.sh clears it automatically, or 'rm autopilot.stop' to cancel."
fi
