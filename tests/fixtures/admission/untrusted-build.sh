#!/bin/sh
# Admission-sandbox fixture: simulates an untrusted build.
#
# The jail wrapper (scripts/admission/jail-<platform>.sh) runs this script
# inside the confinement layer with SCRATCH_DIR set to the one writable directory.
#
# Three probes:
#   BENIGN — write to SCRATCH_DIR; must succeed (false-deny = test failure).
#   NET    — TCP connect to 93.184.216.34:80; must be blocked by the jail.
#   FS     — write to /tmp/jail-escape-probe (outside SCRATCH_DIR); must be blocked.
#
# Exit codes:
#   0  all probes consistent with isolation contract
#   2  net probe was NOT blocked
#   3  fs-escape probe was NOT blocked

set -eu

SCRATCH="${SCRATCH_DIR:-/tmp/scratch}"

# ── probe 1: benign write ─────────────────────────────────────────────────────
printf 'benign-write\n' > "$SCRATCH/ok.txt"
echo "PROBE benign: wrote $SCRATCH/ok.txt — OK"

# ── probe 2: network attempt ──────────────────────────────────────────────────
NET_BLOCKED=0
if command -v python3 >/dev/null 2>&1; then
    python3 -c "
import socket, sys
try:
    s = socket.socket()
    s.settimeout(3)
    s.connect(('93.184.216.34', 80))
    s.close()
    sys.exit(0)
except Exception:
    sys.exit(1)
" && NET_BLOCKED=0 || NET_BLOCKED=1
elif command -v nc >/dev/null 2>&1; then
    nc -z -w 3 93.184.216.34 80 2>/dev/null && NET_BLOCKED=0 || NET_BLOCKED=1
elif command -v curl >/dev/null 2>&1; then
    curl --silent --max-time 3 http://93.184.216.34/ >/dev/null 2>&1 \
        && NET_BLOCKED=0 || NET_BLOCKED=1
else
    # No probe tool available; treat as blocked (conservative).
    NET_BLOCKED=1
fi

if [ "$NET_BLOCKED" -eq 1 ]; then
    echo "PROBE net: blocked — OK"
else
    echo "PROBE net: NOT blocked — FAIL"
    exit 2
fi

# ── probe 3: filesystem escape attempt ───────────────────────────────────────
# Target /usr/jail-escape-probe: outside SCRATCH_DIR, on a path the jail
# mounts read-only (bwrap: --ro-bind / /; macOS SBPL: deny default covers
# file-write outside $SCRATCH; FreeBSD jail: rootfs is read-only except scratch).
# Using /tmp would be wrong: bwrap replaces /tmp with a writable tmpfs.
FS_BLOCKED=0
printf 'jail-escape' > /usr/jail-escape-probe 2>/dev/null \
    && FS_BLOCKED=0 || FS_BLOCKED=1

if [ "$FS_BLOCKED" -eq 1 ]; then
    echo "PROBE fs-escape: blocked — OK"
else
    echo "PROBE fs-escape: NOT blocked — FAIL"
    exit 3
fi

echo "All probes passed."
