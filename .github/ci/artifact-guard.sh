#!/usr/bin/env bash
# artifact-guard — fail-closed check that no build artifact or oversized blob is
# git-tracked. Regenerable build output (a `target/` directory, an `.rlib` /
# `.rmeta`, an object/library/wasm blob) bloats every clone forever, and once in
# history it can only be removed by a human-run history rewrite (git-filter-repo).
# This guard is the mechanical enforcement that keeps the tree clean AFTER that
# rewrite: a commit that adds such a file turns CI red.
#
# It inspects the working tree via `git ls-files` (the whole tracked set), so it
# is exact whether run in CI or locally, and needs no diff base. Exit 0 = clean,
# exit 1 = a forbidden artifact is tracked (with the offending paths named),
# exit 2 = the tracked set could not be enumerated (git failed, or it was empty).

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$script_dir/../../tools/scripts/lib/require-tool.sh"
require_tool git

top="$(git rev-parse --show-toplevel)"
cd "$top"

# Max size for a single tracked file, in bytes (5 MiB). Anything larger is
# almost certainly a binary/build blob that belongs in a release asset or LFS,
# not in the source tree. Legitimate large text fixtures should be reviewed
# explicitly rather than waved through, so there is no allowlist here by design.
MAX_BYTES=$((5 * 1024 * 1024))

fail=0
note() { echo "artifact-guard: $*" >&2; }

# The tracked set, enumerated ONCE with git's own exit status checked, then
# reused by every check below — a git failure hard-exits 2 instead of reading
# as an empty (clean) tree. A repo with no tracked file is not a clean repo.
tracked=()
capture_nul tracked "tracked-file enumeration" -- git ls-files -z
if [ "${#tracked[@]}" -eq 0 ]; then
  note "git ls-files returned no tracked file — refusing to pass over an empty set"
  exit 2
fi

target_re='(^|/)target/'
ext_re='\.(rlib|rmeta|rcgu\.o|o|a|so|dylib|wasm)$'
target_hits=()
ext_hits=()
for f in "${tracked[@]}"; do
  # 1) No file inside any `target/` directory (Cargo build output).
  if [[ "$f" =~ $target_re ]]; then target_hits+=("$f"); fi
  # 2) No compiled-artifact extension (rlib/rmeta/object/static-lib/wasm/shared-lib).
  if [[ "$f" =~ $ext_re ]]; then ext_hits+=("$f"); fi
  # 3) No file over the size threshold.
  if [ -f "$f" ]; then
    size=$(wc -c <"$f")
    if [ "$size" -gt "$MAX_BYTES" ]; then
      note "tracked file exceeds ${MAX_BYTES} bytes (${size} bytes): $f"
      fail=1
    fi
  fi
done

if [ "${#target_hits[@]}" -gt 0 ]; then
  note "tracked files inside a target/ build directory (must be gitignored, never committed):"
  printf '  %s\n' "${target_hits[@]}" >&2
  fail=1
fi
if [ "${#ext_hits[@]}" -gt 0 ]; then
  note "tracked files with a compiled-artifact extension (regenerable — do not commit):"
  printf '  %s\n' "${ext_hits[@]}" >&2
  fail=1
fi

if [ "$fail" -ne 0 ]; then
  note "FAIL — remove the artifact from the index (git rm --cached <path>) and add a .gitignore rule."
  note "If a history rewrite is needed to purge it from past commits, that is a human-run step (git-filter-repo)."
  exit 1
fi

echo "artifact-guard: OK — no tracked build artifacts, and no tracked file over ${MAX_BYTES} bytes."
