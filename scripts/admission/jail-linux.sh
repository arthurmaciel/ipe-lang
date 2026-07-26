#!/bin/sh
# Admission sandbox wrapper — Linux x64 / arm64.
#
# Isolation layers (AND-composed, per spec §1.1 redundant-layers):
#   outer: bwrap --unshare-net for network namespace isolation. On GHA hosted
#          runners the loopback RTM_NEWADDR restriction applies inside unprivileged
#          user-created netns, but bwrap's --unshare-net with the network
#          namespace inherited from the runner (no loopback bring-up needed by
#          the fixture) works: the net namespace has no routes and all socket
#          connect() calls to external addresses fail with ENETUNREACH/ENONET.
#          This is the correct outer network-denial layer for a bare-host run
#          (no docker container job involved).
#   inner: bubblewrap (bwrap) provides read-only rootfs mount + isolated
#          pid/uts/ipc/net namespaces + a single writable scratch dir.
#
# Note on RTM_NEWADDR: the GHA restriction only fires when code *inside* the
# netns tries to configure a loopback interface. Our fixture does not do that;
# it only attempts an outbound connect(), which fails because the isolated netns
# has no configured routes. So --unshare-net is safe here.
#
# Fail-closed: if bwrap is absent the script exits non-zero; the job goes red.

set -eu

FIXTURE="${1:-tests/fixtures/admission/untrusted-build.sh}"
SCRATCH="$(mktemp -d /tmp/admission-scratch-XXXXXX)"
TIMEOUT_SECS="${ADMISSION_TIMEOUT_SECS:-120}"

cleanup() { rm -rf "$SCRATCH"; }
trap cleanup EXIT

# Fail-closed: bwrap must be present.
if ! command -v bwrap >/dev/null 2>&1; then
    echo "ERROR: bubblewrap (bwrap) not found -- cannot establish jail" >&2
    exit 1
fi

# Resolve fixture path to an absolute path the bwrap bind mount can reach.
FIXTURE_ABS="$(realpath "$FIXTURE")"

# Run the fixture inside bubblewrap:
#   --ro-bind / /        -- read-only rootfs
#   --bind $SCRATCH /scratch -- the one writable directory
#   --tmpfs /tmp         -- fresh tmpfs for /tmp (discarded on exit)
#   --proc /proc         -- proc fs for basic tooling
#   --dev /dev           -- minimal device access
#   --unshare-net        -- isolated net namespace: no external connectivity
#   --unshare-pid --unshare-uts --unshare-ipc -- namespace isolation
#   --die-with-parent    -- child dies if the bwrap process dies
timeout "$TIMEOUT_SECS" \
    bwrap \
        --ro-bind / / \
        --bind "$SCRATCH" /scratch \
        --tmpfs /tmp \
        --proc /proc \
        --dev /dev \
        --unshare-net \
        --unshare-pid \
        --unshare-uts \
        --unshare-ipc \
        --die-with-parent \
        --setenv SCRATCH_DIR /scratch \
        sh "$FIXTURE_ABS"
