#!/usr/bin/env bash
# remeasure.sh — rebuild skyc, run every example through it, and print the first
# blocker per example (or PASS), diffing against the last run. This is the
# "measure" half of the refill cadence: it SURFACES new/changed blockers so a
# human can triage which are mechanical (→ backlog.md [progdev-safe], loop-eligible)
# vs guardian/design (→ their own backlog section). It deliberately does NOT
# triage or edit the backlog — that is a judgment call the loop can't make.
#
# Usage:
#   scripts/progressive-development/remeasure.sh            # all examples
#   scripts/progressive-development/remeasure.sh 00 12 37   # only matching dirs
#
# Env: MASTER_GATE_TARGET (~/.cache/master-gate-target) — where skyc is built.
set -uo pipefail
cd "$(dirname "$0")/../.."
TARGET="${MASTER_GATE_TARGET:-$HOME/.cache/master-gate-target}"
RUNTIME="$(pwd)/runtime/src/sky_runtime"
SNAP="docs/architecture/remeasure-snapshot.tsv"

echo "remeasure: building skyc (CARGO_TARGET_DIR=$TARGET) …"
CARGO_TARGET_DIR="$TARGET" timeout 1800 cargo build -p skyc >/tmp/remeasure-build.log 2>&1 \
    || { echo "skyc build FAILED — see /tmp/remeasure-build.log"; tail -20 /tmp/remeasure-build.log; exit 1; }
SKYC="$TARGET/debug/skyc"
[ -x "$SKYC" ] || { echo "skyc binary not found at $SKYC"; exit 1; }

# Select example dirs: all, or only those matching the given prefixes.
dirs=()
if [ "$#" -gt 0 ]; then
    for pat in "$@"; do for d in examples/${pat}*/; do [ -d "$d" ] && dirs+=("$d"); done; done
else
    for d in examples/*/; do [ -d "$d" ] && dirs+=("$d"); done
fi

tmp="$(mktemp)"
printf '%-32s | %s\n' "example" "first blocker (or PASS)"
printf '%s\n' "$(printf '%.0s─' $(seq 1 78))"
pass=0; fail=0
for d in "${dirs[@]}"; do
    name="$(basename "$d")"
    [ -f "$d/src/Main.sky" ] || continue
    out="$(cd "$d" && SKY_RUNTIME_DIR="$RUNTIME" timeout 150 "$SKYC" build src/Main.sky 2>&1)"; rc=$?
    if [ "$rc" -eq 0 ]; then
        blk="PASS"; pass=$((pass+1))
    else
        blk="$(printf '%s\n' "$out" | rg -o 'error\[[A-Z0-9-]+\][^\n]*' | head -1)"
        [ -z "$blk" ] && blk="$(printf '%s\n' "$out" | rg -iN 'error' | head -1)"
        [ -z "$blk" ] && blk="(build failed rc=$rc, no error line — see full output)"
        fail=$((fail+1))
    fi
    printf '%-32s | %s\n' "$name" "${blk:0:96}"
    printf '%s\t%s\n' "$name" "$blk" >> "$tmp"
done
echo ""
printf 'summary: %d PASS, %d blocked (of %d examples)\n' "$pass" "$fail" "$((pass+fail))"

# Diff against the previous snapshot to highlight what CHANGED (newly passing,
# newly broken, or a different blocker) — that is what needs triage.
if [ -f "$SNAP" ]; then
    echo ""; echo "── changes since last remeasure (< was, > now) ──"
    if diff <(sort "$SNAP") <(sort "$tmp") | rg '^[<>]'; then :; else echo "(no changes)"; fi
fi
mkdir -p "$(dirname "$SNAP")"; cp "$tmp" "$SNAP"; rm -f "$tmp"
echo ""
echo "snapshot → $SNAP"
echo "NEXT: triage any changed/new blocker into docs/architecture/backlog.md —"
echo "  mechanical (kernel wire / module register / fixture, reference-backed) → [progdev-safe]"
echo "  divergence / feature-gap / security / type-system → its own section (guardian)."
