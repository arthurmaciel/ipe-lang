#!/bin/sh
# Admission-sandbox fixture: simulates an untrusted build.
#
# The jail wrapper (scripts/admission/jail-<platform>.sh) runs this script
# inside the confinement layer with SCRATCH_DIR set to the one writable
# directory. The same script also runs OUTSIDE the jail as a positive control,
# to prove the forbidden actions are only blocked BECAUSE of the jail — not
# because the target was unreachable or unwritable to begin with.
#
# PROBE_MODE selects which contract to assert:
#   enforce (default) — inside the jail: NET and FS-escape must be BLOCKED.
#   control           — outside the jail: NET and FS-escape must SUCCEED, so a
#                       blocked result under `enforce` can only come from the jail.
#
# Probes:
#   BENIGN — write to SCRATCH_DIR; must succeed (false-deny = test failure).
#            Skipped when SCRATCH_DIR is unset (the control run has no scratch).
#   NET    — TCP connect to $NET_HOST:$NET_PORT.
#   FS     — write to $ESCAPE_PATH (a path outside SCRATCH_DIR that the jail
#            renders read-only/denied but that the control principal can write).
#
# Exit codes:
#   0  probes consistent with PROBE_MODE
#   2  enforce: net was NOT blocked
#   3  enforce: fs-escape was NOT blocked
#   4  control: net was NOT reachable
#   5  control: fs-escape path was NOT writable

set -eu

PROBE_MODE="${PROBE_MODE:-enforce}"
SCRATCH="${SCRATCH_DIR:-}"
ESCAPE_PATH="${ESCAPE_PATH:-/usr/jail-escape-probe}"
NET_HOST="${NET_HOST:-github.com}"
NET_PORT="${NET_PORT:-443}"

# Attempt to write the fs-escape probe. In a function so a failed output
# redirection (read-only/denied target) is caught quietly instead of leaking
# the shell's "cannot create" message onto stderr.
fs_write() {
    printf 'jail-escape' > "$ESCAPE_PATH"
}

# Attempt a TCP connect. Returns 0 on success, non-zero on failure/no-tool.
net_connect() {
    if command -v python3 >/dev/null 2>&1; then
        python3 - "$NET_HOST" "$NET_PORT" <<'PYEOF'
import socket, sys
host, port = sys.argv[1], int(sys.argv[2])
try:
    s = socket.socket()
    s.settimeout(5)
    s.connect((host, port))
    s.close()
    sys.exit(0)
except Exception:
    sys.exit(1)
PYEOF
    elif command -v nc >/dev/null 2>&1; then
        nc -z -w 5 "$NET_HOST" "$NET_PORT" >/dev/null 2>&1
    elif command -v curl >/dev/null 2>&1; then
        curl --silent --max-time 5 "https://$NET_HOST:$NET_PORT/" >/dev/null 2>&1
    else
        return 1
    fi
}

# ── probe 1: benign write (jail run only — the control has no scratch dir) ────
if [ -n "$SCRATCH" ]; then
    printf 'benign-write\n' > "$SCRATCH/ok.txt"
    echo "PROBE benign: wrote $SCRATCH/ok.txt — OK"
fi

# ── probe 2: network attempt ──────────────────────────────────────────────────
if net_connect; then NET_CONNECTED=1; else NET_CONNECTED=0; fi

case "$PROBE_MODE" in
    control)
        if [ "$NET_CONNECTED" -eq 1 ]; then
            echo "PROBE net control: $NET_HOST:$NET_PORT reachable — OK"
        else
            echo "PROBE net control: $NET_HOST:$NET_PORT UNREACHABLE — FAIL"
            exit 4
        fi
        ;;
    *)
        if [ "$NET_CONNECTED" -eq 0 ]; then
            echo "PROBE net: blocked — OK"
        else
            echo "PROBE net: NOT blocked — FAIL"
            exit 2
        fi
        ;;
esac

# ── probe 3: filesystem escape attempt ───────────────────────────────────────
# $ESCAPE_PATH is outside SCRATCH_DIR. Under each jail it is read-only or denied
# (bwrap: --ro-bind / /; macOS SBPL: file-write* denied outside scratch; FreeBSD
# jail: owned by root, the jailed process runs as nobody). The control run uses
# the SAME path with the SAME principal, so a blocked write under `enforce`
# proves jail enforcement rather than a mere pre-existing permission denial.
if fs_write 2>/dev/null; then FS_WROTE=1; else FS_WROTE=0; fi

case "$PROBE_MODE" in
    control)
        if [ "$FS_WROTE" -eq 1 ]; then
            echo "PROBE fs-escape control: $ESCAPE_PATH writable — OK"
            rm -f "$ESCAPE_PATH" 2>/dev/null || true
        else
            echo "PROBE fs-escape control: $ESCAPE_PATH NOT writable — FAIL"
            exit 5
        fi
        ;;
    *)
        if [ "$FS_WROTE" -eq 0 ]; then
            echo "PROBE fs-escape: blocked — OK"
        else
            echo "PROBE fs-escape: NOT blocked — FAIL"
            rm -f "$ESCAPE_PATH" 2>/dev/null || true
            exit 3
        fi
        ;;
esac

echo "All probes passed ($PROBE_MODE)."
