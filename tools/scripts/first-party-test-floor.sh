#!/usr/bin/env bash
# Ipê FIRST-PARTY `ipe test` FLOOR — every shipped example's own test suite.
#
# Runs `ipe test <dir>` for every first-party example project that carries a
# `tests/Main.ipe` (first_party_check_set in tools/scripts/lib/examples.sh:
# every project under examples/shapes/** and examples/wasm/**, nested
# sub-projects included, minus the FFI-gated ones). A suite that fails to
# build or reports a failing case FAILS this floor LOUD, naming each.
#
# `ipe test` builds and runs the suite through cargo, so this floor needs the
# pinned Rust toolchain; first-party-check-floor.sh is the cargo-free
# type-check floor over the same set.
#
# Exit: 0 = every suite passes · 1 = one or more failed ·
#       2 = setup (no repo / no ipe binary / no suite found).
set -uo pipefail

source "$(dirname "$0")/lib/env.sh"
source "$(dirname "$0")/lib/examples.sh"

if [ -z "$REPO" ] || [ ! -f "$REPO/tools/scripts/first-party-test-floor.sh" ]; then
  echo "ERROR: can't locate the repo. cd into it, or set IPE_REPO=/path/to/sky-rust." >&2; exit 2
fi
cd "$REPO" || { echo "ERROR: could not cd into repo '$REPO'." >&2; exit 2; }
if [ ! -x "$IPE_BIN" ]; then
  echo "ERROR: ipe binary not at '$IPE_BIN' — build it: cargo build --release -p ipe (or set IPE_BIN)." >&2; exit 2
fi

echo "=== Ipê first-party ipe-test floor (repo: $REPO · ipe: $IPE_BIN) ==="

# The producer's status is checked before the set is read: a failed walk must
# never read as a short, passing set.
set_output="$(first_party_check_set)"; set_rc=$?
if [ "$set_rc" -ne 0 ]; then
  echo "ERROR: first_party_check_set failed to enumerate the first-party example set (exit $set_rc)." >&2
  exit 2
fi

log="$(mktemp)" || { echo "ERROR: could not create a log file." >&2; exit 2; }
trap 'rm -f "$log"' EXIT
failed=()
tested=0
while IFS= read -r dir; do
  [ -z "$dir" ] && continue
  [ -f "$dir/tests/Main.ipe" ] || continue
  tested=$((tested + 1))
  if timeout 1800 "$IPE_BIN" test "$dir" >"$log" 2>&1; then
    printf '  ok    %s\n' "$dir"
  else
    printf '  FAIL  %s\n' "$dir"
    sed 's/^/          /' "$log"
    failed+=("$dir")
  fi
done <<< "$set_output"

if [ "$tested" -eq 0 ]; then
  echo "ERROR: no first-party example carries a tests/Main.ipe — the set drifted; this floor cannot vacuously pass." >&2
  exit 2
fi

echo
if [ "${#failed[@]}" -gt 0 ]; then
  echo "=== VERDICT: FAIL — ${#failed[@]} of $tested first-party test suite(s) failed:"
  for d in "${failed[@]}"; do echo "  BROKEN: $d"; done
  echo
  echo "A shipped first-party example's tests must pass. Fix the compiler regression"
  echo "or the example — never skip it to make this floor green (PRINCIPLES.md)."
  exit 1
fi
echo "=== VERDICT: PASS — all $tested first-party test suite(s) pass ==="
exit 0
