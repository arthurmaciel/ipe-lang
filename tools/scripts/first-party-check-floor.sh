#!/usr/bin/env bash
# Ipê FIRST-PARTY `ipe type-check` FLOOR — the cheap, gating compile floor.
#
# Runs `ipe type-check` (type-check ONLY — no `ipe build`, no cargo) over every
# shipped first-party example (first_party_check_set in tools/scripts/lib/examples.sh:
# examples/shapes/** + examples/wasm/**, minus the FFI-gated ones). A shipped
# example that fails to type-check FAILS this floor LOUD, naming each broken
# entry — so a compiler change that reddens a canonical example breaks the gate
# instead of rotting silently in the tree.
#
# This is a FLOOR, complementary to (not a duplicate of) the heavier check:
#   • ci.yml's `shapes-examples` build gate — `ipe type-check` + `ipe build` +
#     cargo over the shape examples. This floor is check-only but WIDER: it also
#     covers examples/wasm/**, which otherwise only reaches `ipe type-check` in the
#     non-gating nightly E2E suite.
#
# Exit: 0 = every first-party example type-checks · 1 = one or more failed ·
#       2 = setup (no repo / no ipe binary).
set -uo pipefail

source "$(dirname "$0")/lib/env.sh"
source "$(dirname "$0")/lib/examples.sh"

if [ -z "$REPO" ] || [ ! -f "$REPO/tools/scripts/first-party-check-floor.sh" ]; then
  echo "ERROR: can't locate the repo. cd into it, or set IPE_REPO=/path/to/sky-rust." >&2; exit 2
fi
cd "$REPO" || { echo "ERROR: could not cd into repo '$REPO'." >&2; exit 2; }
if [ ! -x "$IPE_BIN" ]; then
  echo "ERROR: ipe binary not at '$IPE_BIN' — build it: cargo build --release -p ipe (or set IPE_BIN)." >&2; exit 2
fi

echo "=== Ipê first-party ipe-check floor (repo: $REPO · ipe: $IPE_BIN) ==="

failed=()
checked=0
while IFS= read -r dir; do
  [ -z "$dir" ] && continue
  checked=$((checked + 1))
  # `ipe type-check` type-checks the source graph reachable from the entry; it emits
  # no Rust and runs no cargo, so the floor stays fast and deterministic.
  if timeout 120 "$IPE_BIN" type-check "$dir/src/Main.ipe" >/tmp/first-party-check.$$.log 2>&1; then
    printf '  ok    %s\n' "$dir"
  else
    printf '  FAIL  %s\n' "$dir"
    sed 's/^/          /' /tmp/first-party-check.$$.log
    failed+=("$dir")
  fi
done < <(first_party_check_set)
rm -f /tmp/first-party-check.$$.log

echo
if [ "${#failed[@]}" -gt 0 ]; then
  echo "=== VERDICT: FAIL — ${#failed[@]} of $checked first-party example(s) do not 'ipe type-check' clean:"
  for d in "${failed[@]}"; do echo "  BROKEN: $d"; done
  echo
  echo "A shipped first-party example must type-check. Fix the compiler regression"
  echo "or the example — never skip it to make this floor green (PRINCIPLES.md §0)."
  exit 1
fi
echo "=== VERDICT: PASS — all $checked first-party example(s) type-check ==="
exit 0
