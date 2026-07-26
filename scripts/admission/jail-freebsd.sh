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

# Pre-create mountpoint stubs BEFORE the ro nullfs covers them.
# The JAIL_ROOT dir is empty at this point; mkdir succeeds.
mkdir -p "${JAIL_ROOT}/scratch"
mkdir -p "${JAIL_ROOT}/dev"

# Mount the live system as read-only over JAIL_ROOT.
# The stub dirs are shadowed but the mountpoints now exist in the VFS.
mount -t nullfs -o ro / "${JAIL_ROOT}"

# Mount writable scratch over the /scratch stub.
mount -t nullfs -o rw "$SCRATCH_HOST" "${JAIL_ROOT}/scratch"

# Minimal devfs.
mount -t devfs devfs "${JAIL_ROOT}/dev"

# Copy the fixture into the jail root at a fixed path.
# /tmp inside the jail points to the real /tmp (ro), so use the
# JAIL_ROOT directly from the host (which is not part of the nullfs tree).
cp "$FIXTURE_ABS" "/tmp/admission-fixture-$$.sh"
chmod +x "/tmp/admission-fixture-$$.sh"

cleanup_fixture() { rm -f "/tmp/admission-fixture-$$.sh"; }
trap 'cleanup_fixture; cleanup' EXIT

# Bind the fixture into the jail via the scratch volume so we can reach it.
cp "$FIXTURE_ABS" "${SCRATCH_HOST}/fixture.sh"
chmod +x "${SCRATCH_HOST}/fixture.sh"

# Run inside jail: no ip4/ip6, no raw socket, no sysvipc.
# SCRATCH_DIR and the fixture path use /scratch which is rw inside.
timeout "$TIMEOUT_SECS" \
    jail -c \
        path="${JAIL_ROOT}" \
        host.hostname=admission-jail \
        ip4=disable \
        ip6=disable \
        allow.raw_sockets=0 \
        allow.sysvipc=0 \
        persist=0 \
        exec.start="/usr/bin/env SCRATCH_DIR=/scratch /bin/sh /scratch/fixture.sh"
