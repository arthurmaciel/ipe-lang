#!/usr/bin/env bash
# Parity / drift gate for tree-sitter-ipe.
#
# The compiler's hand-written parser is the source of truth for what Ipê
# accepts. This grammar must not silently diverge from it, so it is run over
# EVERY reference source in the repo — all `examples/**/*.ipe` and
# `src/stdlib/Ipe/**/*.ipe` — and fails on any `ERROR` or `MISSING` node. A
# grammar that ERRORs on real stdlib/example source is not done (same concern
# as the byte-exact goldens).
#
# Usage: run from anywhere; paths are resolved relative to the repo root.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
grammar_dir="$(cd "$script_dir/.." && pwd)"
repo_root="$(cd "$grammar_dir/../.." && pwd)"

if ! command -v tree-sitter >/dev/null 2>&1; then
  echo "error: tree-sitter CLI not found on PATH (cargo install tree-sitter-cli)" >&2
  exit 2
fi

# The grammar is loaded from its own directory.
cd "$grammar_dir"
tree-sitter generate >/dev/null

# Collect every reference source.
mapfile -d '' files < <(
  find "$repo_root/examples" "$repo_root/src/stdlib/Ipe" \
    -type f -name '*.ipe' -print0 2>/dev/null | sort -z
)

if [ "${#files[@]}" -eq 0 ]; then
  echo "error: no .ipe files found under examples/ or src/stdlib/Ipe/" >&2
  exit 2
fi

total=0
failed=0
fail_list=()
for f in "${files[@]}"; do
  total=$((total + 1))
  # `parse --quiet` prints an `(ERROR …)`/`(MISSING …)` summary line and exits
  # non-zero when the tree contains an error; a clean parse prints only timing.
  if ! out="$(tree-sitter parse --quiet "$f" 2>&1)"; then
    failed=$((failed + 1))
    fail_list+=("$f")
    echo "ERROR/MISSING: $f" >&2
    echo "$out" | grep -E 'ERROR|MISSING' >&2 || true
  elif echo "$out" | grep -qE 'ERROR|MISSING'; then
    # Defensive: a MISSING can be recovered (rc 0) yet still print a summary.
    failed=$((failed + 1))
    fail_list+=("$f")
    echo "MISSING: $f" >&2
    echo "$out" | grep -E 'ERROR|MISSING' >&2 || true
  fi
done

echo "parsed $total file(s); $failed with ERROR/MISSING"
if [ "$failed" -ne 0 ]; then
  exit 1
fi
echo "parity OK"
