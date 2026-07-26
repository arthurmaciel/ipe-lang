#!/bin/sh
# Admission sandbox wrapper — Linux x64 / arm64.
#
# Isolation layers (AND-composed, per spec §1.1 redundant-layers):
#   outer: `unshare --net` creates a new network namespace in which no
#          network interfaces are configured. All socket connect() calls
#          to external addresses fail ENETUNREACH. Unlike bwrap --unshare-net,
#          `unshare -n` does NOT call RTM_NEWADDR to configure loopback, so it
#          does not trigger the GHA runner's CAP_NET_ADMIN restriction.
#   inner: bubblewrap (bwrap) provides read-only rootfs + isolated
#          pid/uts/ipc namespaces + single writable scratch dir.
#
# Fail-closed: if bwrap or unshare is absent the script exits non-zero.

set -eu

FIXTURE="${1:-tests/fixtures/admission/untrusted-build.sh}"
SCRATCH="$(mktemp -d /tmp/admission-scratch-XXXXXX)"
TIMEOUT_SECS="${ADMISSION_TIMEOUT_SECS:-120}"

cleanup() { rm -rf "$SCRATCH"; }
trap cleanup EXIT

if ! command -v bwrap >/dev/null 2>&1; then
    echo "ERROR: bubblewrap (bwrap) not found -- cannot establish jail" >&2
    exit 1
fi

if ! command -v unshare >/dev/null 2>&1; then
    echo "ERROR: unshare not found -- cannot establish network namespace" >&2
    exit 1
fi

FIXTURE_ABS="$(realpath "$FIXTURE")"

# unshare -n: new network namespace (no loopback config, no RTM_NEWADDR).
# bwrap inside: read-only rootfs, writable scratch, isolated pid/uts/ipc.
timeout "$TIMEOUT_SECS" \
    unshare --net \
    bwrap \
        --ro-bind / / \
        --bind "$SCRATCH" /scratch \
        --tmpfs /tmp \
        --proc /proc \
        --dev /dev \
        --unshare-pid \
        --unshare-uts \
        --unshare-ipc \
        --die-with-parent \
        --setenv SCRATCH_DIR /scratch \
        sh "$FIXTURE_ABS"
