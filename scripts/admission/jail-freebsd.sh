#!/bin/sh
# Admission sandbox wrapper — FreeBSD x64.
#
# Isolation: jail(8) with:
#   - ip4=disable ip6=disable: no network
#   - allow.raw_sockets=0: no raw socket access
#   - allow.sysvipc=0: no SysV IPC
#   - path=/ (live root, chrooted by jail)
#   - exec.jail_user=nobody (non-root; FS writes outside scratch fail)
#
# The jail path is set to the real root so there are no nullfs mount ordering
# issues.  Network isolation is enforced by jail's ip4/ip6=disable.  Write
# isolation comes from running as nobody (uid 65534) -- writes outside the
# writable scratch dir owned by nobody will fail with EACCES.
#
# Fail-closed: if jail(8) is absent, exit non-zero.

set -eu

FIXTURE="${1:-tests/fixtures/admission/untrusted-build.sh}"
TIMEOUT_SECS="${ADMISSION_TIMEOUT_SECS:-120}"

if ! command -v jail >/dev/null 2>&1; then
    echo "ERROR: jail(8) not found -- cannot establish jail" >&2
    exit 1
fi

# Create a scratch dir owned by nobody so the jailed process can write there.
SCRATCH="$(mktemp -d /tmp/admission-scratch-XXXXXX)"
chown nobody "$SCRATCH"

# Copy the fixture into the scratch dir so nobody can read and exec it.
cp "$(realpath "$FIXTURE")" "$SCRATCH/fixture.sh"
chown nobody "$SCRATCH/fixture.sh"
chmod 550 "$SCRATCH/fixture.sh"

cleanup() { rm -rf "$SCRATCH"; }
trap cleanup EXIT

# Run inside jail as nobody: chrooted to live root, no network, limited IPC.
timeout "$TIMEOUT_SECS" \
    jail -c \
        path=/ \
        host.hostname=admission-jail \
        ip4=disable \
        ip6=disable \
        allow.raw_sockets=0 \
        allow.sysvipc=0 \
        persist=0 \
        exec.jail_user=nobody \
        exec.start="/usr/bin/env SCRATCH_DIR=$SCRATCH /bin/sh $SCRATCH/fixture.sh"
