#!/bin/sh
# Admission sandbox wrapper — Linux x64 / arm64.
#
# Isolation layers (AND-composed, per spec §1.1 redundant-layers):
#   outer: container job runs with --network none (enforced by the GHA job definition)
#   inner: bubblewrap (bwrap) provides read-only rootfs mount + isolated pid/uts/ipc
#          namespaces + a single writable scratch dir.
#
# Network denial is enforced at the outer container layer (--network none in the
# GHA job). The inner bwrap layer adds filesystem and resource scoping on top.
# The two layers are AND-composed: bypassing one is still caught by the other.
#
# Fail-closed: if bwrap is absent the script exits non-zero; the job goes red.
# A jail that cannot establish must never let the untrusted payload run unjailed.

set -eu

FIXTURE="${1:-tests/fixtures/admission/untrusted-build.sh}"
SCRATCH="$(mktemp -d /tmp/admission-scratch-XXXXXX)"
TIMEOUT_SECS="${ADMISSION_TIMEOUT_SECS:-120}"

cleanup() { rm -rf "$SCRATCH"; }
trap cleanup EXIT

# Fail-closed: bwrap must be present.
if ! command -v bwrap >/dev/null 2>&1; then
    echo "ERROR: bubblewrap (bwrap) not found — cannot establish jail" >&2
    exit 1
fi

# Resolve fixture path to an absolute path the bwrap bind mount can reach.
FIXTURE_ABS="$(realpath "$FIXTURE")"
FIXTURE_DIR="$(dirname "$FIXTURE_ABS")"

# Run the fixture inside bubblewrap:
#   --ro-bind / /   — read-only rootfs (entire host root, read-only)
#   --bind $SCRATCH /scratch — the one writable directory
#   --tmpfs /tmp    — replace /tmp with a fresh tmpfs so /tmp writes hit tmpfs,
#                     not the host; the fixture's fs-escape probe writes to /tmp
#                     inside the jail — that tmpfs is discarded on exit.
#   --proc /proc    — proc fs for basic tooling
#   --dev /dev      — minimal device access
#   --ro-bind $FIXTURE_DIR $FIXTURE_DIR — fixture is read-only inside jail
#   --unshare-pid --unshare-uts --unshare-ipc — isolate pid/uts/ipc namespaces
#   --die-with-parent — child dies if the parent bwrap process dies
#   --setenv SCRATCH_DIR /scratch — tell the fixture where its writable dir is
#
# We deliberately do NOT pass --unshare-net: GitHub hosted runners deny
# RTM_NEWADDR inside a user-created netns (see spec §1.2 constraint 1).
# Network denial is handled by the outer container --network none layer.
timeout "$TIMEOUT_SECS" \
    bwrap \
        --ro-bind / / \
        --bind "$SCRATCH" /scratch \
        --tmpfs /tmp \
        --proc /proc \
        --dev /dev \
        --ro-bind "$FIXTURE_DIR" "$FIXTURE_DIR" \
        --unshare-pid \
        --unshare-uts \
        --unshare-ipc \
        --die-with-parent \
        --setenv SCRATCH_DIR /scratch \
        sh "$FIXTURE_ABS"
