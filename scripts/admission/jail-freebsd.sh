#!/bin/sh
# Admission sandbox wrapper — FreeBSD x64.
#
# Isolation: jail(8) with:
#   - ip4=disable ip6=disable: no network inside the jail
#   - allow.raw_sockets=0: no raw socket access
#   - allow.sysvipc=0: no SysV IPC
#   - a nullfs read-only bind of / as the jail root
#   - a single writable mount at /scratch inside the jail
#
# The jail root is built from a nullfs read-only bind of the live system root,
# so all system binaries (sh, python3, etc.) are available without copying.
# The /scratch mount is the one writable directory.
#
# Fail-closed: if jail(8) is absent the script exits non-zero.
# Must run as root (required for jail + nullfs mount setup).

set -eu

FIXTURE="${1:-tests/fixtures/admission/untrusted-build.sh}"
TIMEOUT_SECS="${ADMISSION_TIMEOUT_SECS:-120}"

if ! command -v jail >/dev/null 2>&1; then
    echo "ERROR: jail(8) not found — cannot establish jail" >&2
    exit 1
fi

if [ "$(id -u)" -ne 0 ]; then
    echo "ERROR: jail setup requires root" >&2
    exit 1
fi

JAIL_ROOT="$(mktemp -d /tmp/admission-jail-XXXXXX)"
SCRATCH_HOST="$(mktemp -d /tmp/admission-scratch-XXXXXX)"

cleanup() {
    # Unmount in reverse order; ignore errors on individual unmounts.
    umount "${JAIL_ROOT}/scratch"  2>/dev/null || true
    umount "${JAIL_ROOT}/dev"      2>/dev/null || true
    umount "${JAIL_ROOT}"          2>/dev/null || true
    rm -rf "$JAIL_ROOT" "$SCRATCH_HOST"
}
trap cleanup EXIT

# Build the jail root via nullfs read-only bind of the live system.
mkdir -p "${JAIL_ROOT}"
mount -t nullfs -o ro / "${JAIL_ROOT}"

# Writable scratch inside the jail.
mkdir -p "${JAIL_ROOT}/scratch"
mount -t nullfs -o rw "$SCRATCH_HOST" "${JAIL_ROOT}/scratch"

# Minimal devfs for the shell and python3.
mkdir -p "${JAIL_ROOT}/dev"
mount -t devfs devfs "${JAIL_ROOT}/dev"
devfs -m "${JAIL_ROOT}/dev" rule applyset

FIXTURE_ABS="$(realpath "$FIXTURE")"

# Copy the fixture into the jail root (it must be reachable as an absolute path
# inside the jail; the repo checkout is on the host at the same path).
FIXTURE_INSIDE="${JAIL_ROOT}${FIXTURE_ABS}"
mkdir -p "$(dirname "$FIXTURE_INSIDE")"
cp "$FIXTURE_ABS" "$FIXTURE_INSIDE"
chmod +x "$FIXTURE_INSIDE"

# jail(8) one-shot execution:
#   exec.start runs the command and the jail terminates when it exits.
#   SCRATCH_DIR is passed via setenv.
timeout "$TIMEOUT_SECS" \
    jail -c \
        path="${JAIL_ROOT}" \
        host.hostname=admission-jail \
        ip4=disable \
        ip6=disable \
        allow.raw_sockets=0 \
        allow.sysvipc=0 \
        persist=0 \
        "exec.jail_user=nobody" \
        "setenv.SCRATCH_DIR=/scratch" \
        "exec.start=/bin/sh ${FIXTURE_ABS}"
