#!/bin/sh
# Admission sandbox wrapper — macOS arm64.
#
# Isolation: sandbox-exec(1) with an SBPL (Seatbelt) profile that:
#   - denies all network by default
#   - allows reading the system root read-only
#   - allows writing only to the scratch directory
#   - denies all process execution outside the current process tree
#
# Fail-closed: if sandbox-exec is absent the script exits non-zero.

set -eu

FIXTURE="${1:-tests/fixtures/admission/untrusted-build.sh}"
SCRATCH="$(mktemp -d /tmp/admission-scratch-XXXXXX)"
TIMEOUT_SECS="${ADMISSION_TIMEOUT_SECS:-120}"
PROFILE_FILE="$(mktemp /tmp/admission-sbpl-XXXXXX.sb)"

cleanup() { rm -rf "$SCRATCH" "$PROFILE_FILE"; }
trap cleanup EXIT

# Fail-closed: sandbox-exec must be present.
if ! command -v sandbox-exec >/dev/null 2>&1; then
    echo "ERROR: sandbox-exec not found — cannot establish jail" >&2
    exit 1
fi

FIXTURE_ABS="$(cd "$(dirname "$FIXTURE")" && pwd)/$(basename "$FIXTURE")"
FIXTURE_DIR="$(dirname "$FIXTURE_ABS")"

# Write the SBPL profile.
# deny* comes first; allow* overrides follow.
cat > "$PROFILE_FILE" << SBPL
(version 1)

; Default-deny everything. Explicit allows follow; no conflicting deny/allow
; for the same subpath (last-rule wins in some Seatbelt versions, first-rule
; in others — keep it unambiguous by not denying what we already allow).
(deny default)

; Allow reading the system root and standard paths needed by sh + python3.
(allow file-read*)
(allow file-read-metadata)

; Allow writes only inside the scratch directory.
(allow file-write*
    (subpath "$SCRATCH"))

; Allow shared-memory / pipe / semaphore operations used by the POSIX shell.
(allow ipc-posix-shm-read-data)
(allow ipc-posix-shm-write-data)
(allow ipc-posix-sem)
(allow ipc-sysv-shm)

; Allow process operations needed to run the fixture.
(allow process-exec)
(allow process-fork)
(allow signal (target self))
(allow signal (target children))

; Allow sysctl reads used by the runtime.
(allow sysctl-read)

; Allow Mach operations needed by the POSIX layer on macOS.
(allow mach-lookup)
(allow mach-task-name)

; All network is denied by the default-deny above; no explicit network rule
; needed. The fixture's net probe must fail because (deny default) covers
; network-outbound and network-bind.
SBPL

export SCRATCH_DIR="$SCRATCH"

timeout "$TIMEOUT_SECS" \
    sandbox-exec -f "$PROFILE_FILE" \
        sh "$FIXTURE_ABS"
