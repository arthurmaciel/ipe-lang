#!/bin/sh
# Admission sandbox wrapper — FreeBSD x64.
#
# Isolation: jail(8) with:
#   - ip4=disable ip6=disable: no network
#   - allow.raw_sockets=0: no raw socket access
#   - allow.sysvipc=0: no SysV IPC
#   - nullfs read-only bind of the live system as the jail root
#   - a single writable nullfs mount at /scratch inside the jail
#
# Fail-closed: if jail(8) is absent or mounts fail, exit non-zero.
# Runs as root inside the vmactions VM (required for jail + nullfs).

set -eu

FIXTURE="${1:-tests/fixtures/admission/untrusted-build.sh}"
TIMEOUT_SECS="${ADMISSION_TIMEOUT_SECS:-120}"

if ! command -v jail >/dev/null 2>&1; then
    echo "ERROR: jail(8) not found -- cannot establish jail" >&2
    exit 1
fi

JAIL_ROOT="$(mktemp -d /tmp/admission-jail-XXXXXX)"
SCRATCH_HOST="$(mktemp -d /tmp/admission-scratch-XXXXXX)"

cleanup() {
    umount "${JAIL_ROOT}/scratch" 2>/dev/null || true
    umount "${JAIL_ROOT}/dev"     2>/dev/null || true
    umount "${JAIL_ROOT}"         2>/dev/null || true
    rm -rf "$JAIL_ROOT" "$SCRATCH_HOST"
}
trap cleanup EXIT

FIXTURE_ABS="$(realpath "$FIXTURE")"

# Build jail root via nullfs read-only bind of the live system.
mount -t nullfs -o ro / "${JAIL_ROOT}"

# Writable scratch dir inside the jail.
mkdir -p "${JAIL_ROOT}/scratch"
mount -t nullfs -o rw "$SCRATCH_HOST" "${JAIL_ROOT}/scratch"

# Minimal devfs.
mkdir -p "${JAIL_ROOT}/dev"
mount -t devfs devfs "${JAIL_ROOT}/dev"

# Copy the fixture into the jail root at a fixed path.
cp "$FIXTURE_ABS" "${JAIL_ROOT}/fixture.sh"
chmod +x "${JAIL_ROOT}/fixture.sh"

# Run inside jail: no ip4/ip6, no raw socket, no sysvipc.
# SCRATCH_DIR is passed via `exec.start` invoking env(1) before sh.
timeout "$TIMEOUT_SECS" \
    jail -c \
        path="${JAIL_ROOT}" \
        host.hostname=admission-jail \
        ip4=disable \
        ip6=disable \
        allow.raw_sockets=0 \
        allow.sysvipc=0 \
        persist=0 \
        exec.start="/usr/bin/env SCRATCH_DIR=/scratch /bin/sh /fixture.sh"
