#!/bin/sh
# Admission-sandbox fixture: simulates an untrusted build.
#
# The jail wrapper (tools/scripts/admission/jail-<platform>.sh) runs this script
# inside the confinement layer with SCRATCH_DIR set to the one writable
# directory. The same script also runs OUTSIDE the jail as a positive control,
# to prove the forbidden actions are only blocked BECAUSE of the jail — not
# because the target was unreachable or unwritable to begin with.
#
# PROBE_MODE selects which contract to assert:
#   enforce (default) — inside the jail: NET and FS-escape must be BLOCKED.
#   control           — outside the jail: NET and FS-escape must SUCCEED, so a
#                       blocked result under `enforce` can only come from the jail.
#   tier2             — inside a jail scoped to a package's DECLARED capability
#                       set (differential confinement): a DENIED action names the
#                       axis the native code demanded but the declared set
#                       withheld (used-but-undeclared). The exit code is the
#                       wrapper-owned per-axis denial signal the Tier-2 decoder
#                       reads — never scraped from the payload's stdout.
#
# Probes:
#   BENIGN — write to SCRATCH_DIR; must succeed (false-deny = test failure).
#            Skipped when SCRATCH_DIR is unset (the control run has no scratch).
#   NET    — TCP connect to $NET_HOST:$NET_PORT.
#   FS     — write to $ESCAPE_PATH (a path outside SCRATCH_DIR that the jail
#            renders read-only/denied but that the control principal can write).
#
# Exit codes:
#   0   probes consistent with PROBE_MODE (tier2: build clean + no withheld axis)
#   6   tier2: the untrusted child build failed for an ordinary (non-capability)
#       reason with NO withheld axis demanded — an ordinary build-fails-in-jail.
#       Decodes to BuildFailed (a reject), never Clean. THE LOAD-BEARING HINGE:
#       the wrapper NEVER exits 0 when the child build failed, or a broken build
#       would forge a clean certify.
#   2   enforce: net was NOT blocked
#   3   enforce: fs-escape was NOT blocked
#   4   control: net was NOT reachable
#   5   control: fs-escape path was NOT writable
#   10  tier2: network denied — the native code demanded the withheld `network`
#       axis (used-but-undeclared)
#   11  tier2: fs-escape denied — the native code demanded the withheld
#       `filesystem` axis (used-but-undeclared)
#
# The tier2 codes 10/11 live in a range disjoint from the enforce/control codes
# 2–5 so a differential denial can never be confused with a broken-jail control
# failure. They are the single source of truth mirrored by the Rust build-jail
# decoder (`CapabilityAxis::from_exit_code`), asserted equal in the crate tests.

set -eu

PROBE_MODE="${PROBE_MODE:-enforce}"
SCRATCH="${SCRATCH_DIR:-}"
ESCAPE_PATH="${ESCAPE_PATH:-/usr/jail-escape-probe}"
NET_HOST="${NET_HOST:-github.com}"
NET_PORT="${NET_PORT:-443}"
# In tier2 mode, which axes the probe exercises: `network`, `filesystem`, or
# `both`, or `none`. A Tier-2 caller confining one withheld axis exercises
# exactly that axis, so a denial names it unambiguously and a granted-but-unrouted
# host cannot masquerade as the wrong axis. `none` selects the full-run
# child-exit-only mode: no fixed axis probe runs (it would fabricate a demand the
# package never made), and the verdict is the child build's own exit — the sound
# signal for the full declared-scoped run of a real build (see below). Ignored
# outside tier2 mode.
TIER2_AXIS="${TIER2_AXIS:-both}"

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

# ── tier2: run the untrusted child build (the positional argv) ────────────────
# The wrapper runs its positional argv ($@) as a CHILD before its own fixed axis
# probe. A withheld-axis syscall inside the child is killed by the jail; that
# same withheld axis also trips the wrapper's post-build probe below (the jail
# withholds the axis for the whole session), so the per-axis code names it. A
# child that fails for an ordinary reason with no axis demanded is code 6
# (BuildFailed) at the end — the wrapper NEVER exits 0 when the child failed.
CHILD_STATUS=0
if [ "$PROBE_MODE" = "tier2" ] && [ "$#" -gt 0 ]; then
    # Disable `set -e` around the child so a non-zero exit is captured, not fatal.
    set +e
    "$@"
    CHILD_STATUS=$?
    set -e
    echo "PROBE child-build tier2: exit $CHILD_STATUS"
fi

# ── tier2 full declared-scoped run (TIER2_AXIS=none): child-exit-only ─────────
# On the FULL declared-scoped real-build run the wrapper runs NO fixed axis probe:
# a fixed probe would fabricate a capability demand the package never made (a
# socket / out-of-scratch write the build did not do), so no declared set other
# than {network,filesystem} could ever certify. Instead the signal is the child
# build's own exit: a withheld axis is withheld by capability REMOVAL (the net
# namespace is unshared; the escape path is not bound writable), so a build that
# reaches it is killed / errors → non-zero → BuildFailed. A build that caught the
# error and exited anyway performed NO effect (a caught denial is a no-op), so
# exit 0 is positive proof the build reached no withheld axis. The child build we
# run is a fixed `cargo build` of a generated probe crate (our argv, our probe
# main), so the untrusted crate cannot own this exit's meaning.
if [ "$PROBE_MODE" = "tier2" ] && [ "$TIER2_AXIS" = "none" ]; then
    if [ "$CHILD_STATUS" -ne 0 ]; then
        echo "PROBE full-run tier2: child build failed (exit $CHILD_STATUS) — BuildFailed"
        exit 6
    fi
    echo "PROBE full-run tier2: child build clean — no withheld axis reached"
    exit 0
fi

# ── probe 2: network attempt ──────────────────────────────────────────────────
# In tier2 mode, skip the net probe when the caller selected the filesystem axis
# only — so a granted-but-unrouted host cannot trip a spurious network denial.
if [ "$PROBE_MODE" = "tier2" ] && [ "$TIER2_AXIS" = "filesystem" ]; then
    NET_CONNECTED=1
    echo "PROBE net tier2: skipped (axis=filesystem)"
elif net_connect; then
    NET_CONNECTED=1
else
    NET_CONNECTED=0
fi

case "$PROBE_MODE" in
    control)
        if [ "$NET_CONNECTED" -eq 1 ]; then
            echo "PROBE net control: $NET_HOST:$NET_PORT reachable — OK"
        else
            echo "PROBE net control: $NET_HOST:$NET_PORT UNREACHABLE — FAIL"
            exit 4
        fi
        ;;
    tier2)
        # Differential confinement: a DENIED net attempt means the native code
        # demanded the `network` axis the declared-scoped jail withheld. Name it.
        if [ "$NET_CONNECTED" -eq 0 ]; then
            echo "PROBE net tier2: denied — the code demanded the withheld network axis"
            exit 10
        else
            echo "PROBE net tier2: reached — network axis not withheld/not demanded"
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
# In tier2 mode, skip the fs-escape probe when the caller selected the network
# axis only — the run then proves a CLEAN outcome (exit 0) for the network case.
if [ "$PROBE_MODE" = "tier2" ] && [ "$TIER2_AXIS" = "network" ]; then
    echo "PROBE fs-escape tier2: skipped (axis=network)"
    # No axis was denied above; a non-zero child build here is an ordinary
    # build failure (BuildFailed), never a clean certify.
    if [ "$CHILD_STATUS" -ne 0 ]; then
        echo "PROBE child-build tier2: ordinary failure (exit $CHILD_STATUS), no axis demanded"
        exit 6
    fi
    echo "All probes passed ($PROBE_MODE)."
    exit 0
fi

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
    tier2)
        # Differential confinement: a DENIED out-of-scratch write means the code
        # demanded the `filesystem` axis the declared-scoped jail withheld.
        if [ "$FS_WROTE" -eq 0 ]; then
            echo "PROBE fs-escape tier2: denied — the code demanded the withheld filesystem axis"
            exit 11
        else
            echo "PROBE fs-escape tier2: wrote — filesystem axis not withheld/not demanded"
            rm -f "$ESCAPE_PATH" 2>/dev/null || true
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

# The load-bearing hinge (tier2): no withheld axis was denied above, so a
# non-zero child build here is an ORDINARY build failure — never a clean certify.
if [ "$PROBE_MODE" = "tier2" ] && [ "$CHILD_STATUS" -ne 0 ]; then
    echo "PROBE child-build tier2: ordinary failure (exit $CHILD_STATUS), no axis demanded"
    exit 6
fi

echo "All probes passed ($PROBE_MODE)."
