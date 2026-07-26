#!/bin/sh
# Admission sandbox wrapper — macOS arm64.
#
# Isolation: sandbox-exec(1) with a Seatbelt SBPL profile that:
#   - denies all outbound/inbound network (network-outbound, network-inbound,
#     network-bind, and the low-level socket operations)
#   - allows everything else (allow default), so system tools needed by the
#     fixture (sh, python3, etc.) can run without exhaustive allow-listing
#
# FS isolation (write outside scratch is blocked):
#   The profile denies file-write* except inside the scratch dir.
#   On macOS 26, (deny default) blocks too many system operations and
#   causes benign writes to fail even with explicit (allow file-write*)
#   overrides. The pattern that works: (allow default) + targeted denials.
#
# Fail-closed: if sandbox-exec is absent the script exits non-zero.

set -eu

FIXTURE="${1:-tests/fixtures/admission/untrusted-build.sh}"
TMPBASE="${TMPDIR:-/tmp}"
SCRATCH="$(mktemp -d "${TMPBASE}admission-scratch-XXXXXX")"
TIMEOUT_SECS="${ADMISSION_TIMEOUT_SECS:-120}"
PROFILE_FILE="$(mktemp "${TMPBASE}admission-sbpl-XXXXXX.sb")"
# Forwarded into the jail so the enforce run and the unjailed control run target
# the SAME fs-escape path (the SBPL profile denies file-write* outside scratch).
ESCAPE_PATH="${ESCAPE_PATH:-/usr/jail-escape-probe}"

cleanup() { rm -rf "$SCRATCH" "$PROFILE_FILE"; }
trap cleanup EXIT

if ! command -v sandbox-exec >/dev/null 2>&1; then
    echo "ERROR: sandbox-exec not found -- cannot establish jail" >&2
    exit 1
fi

FIXTURE_ABS="$(cd "$(dirname "$FIXTURE")" && pwd)/$(basename "$FIXTURE")"

# SBPL profile: allow default, deny network, deny FS writes outside scratch.
# (allow default) is the safe base on macOS 26+: it allows all operations
# the shell and its children need, and we selectively deny the threats.
cat > "$PROFILE_FILE" << SBPL
(version 1)

; Allow everything by default so the shell and its tools work.
(allow default)

; Deny all network operations.
(deny network*)
(deny network-outbound)
(deny network-inbound)
(deny network-bind)

; Deny file writes everywhere except the scratch dir.
(deny file-write*)
(allow file-write*
    (subpath "$SCRATCH"))

; Also allow writes to standard system temp locations the shell uses.
(allow file-write*
    (subpath "/private/var/folders"))
(allow file-write*
    (subpath "/private/tmp"))
SBPL

export SCRATCH_DIR="$SCRATCH"
export ESCAPE_PATH

# macOS ships `gtimeout` (GNU coreutils, Homebrew), not POSIX `timeout`.
if command -v gtimeout >/dev/null 2>&1; then
    TIMEOUT_CMD=gtimeout
elif command -v timeout >/dev/null 2>&1; then
    TIMEOUT_CMD=timeout
else
    echo "WARNING: no timeout command found; running without wall-clock cap" >&2
    TIMEOUT_CMD=
fi

if [ -n "$TIMEOUT_CMD" ]; then
    "$TIMEOUT_CMD" "$TIMEOUT_SECS" \
        sandbox-exec -f "$PROFILE_FILE" \
            sh "$FIXTURE_ABS"
else
    sandbox-exec -f "$PROFILE_FILE" \
        sh "$FIXTURE_ABS"
fi
