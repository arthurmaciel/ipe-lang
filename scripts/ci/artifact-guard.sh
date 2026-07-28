#!/usr/bin/env bash
# artifact-guard — fail-closed check that no build artifact or oversized blob is
# git-tracked. Regenerable build output (a `target/` directory, an `.rlib` /
# `.rmeta`, an object/library/wasm blob) bloats every clone forever, and once in
# history it can only be removed by a human-run history rewrite (git-filter-repo;
# see docs/architecture/tbd/prune-git-history-from-binaries.md). This guard is
# the mechanical enforcement that keeps the tree clean AFTER that rewrite: a
# commit that adds such a file turns CI red.
#
# It inspects the working tree via `git ls-files` (the whole tracked set), so it
# is exact whether run in CI or locally, and needs no diff base. Exit 0 = clean,
# exit 1 = a forbidden artifact is tracked (with the offending paths named).

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

# Max size for a single tracked file, in bytes (5 MiB). Anything larger is
# almost certainly a binary/build blob that belongs in a release asset or LFS,
# not in the source tree. Legitimate large text fixtures should be reviewed
# explicitly rather than waved through, so there is no allowlist here by design.
MAX_BYTES=$((5 * 1024 * 1024))

fail=0
note() { echo "artifact-guard: $*" >&2; }

# 1) No file inside any `target/` directory (Cargo build output).
target_hits="$(git ls-files | grep -E '(^|/)target/' || true)"
if [ -n "$target_hits" ]; then
  note "tracked files inside a target/ build directory (must be gitignored, never committed):"
  echo "$target_hits" | awk '{ print "  " $0 }' >&2
  fail=1
fi

# 2) No compiled-artifact extensions (rlib/rmeta/object/static-lib/wasm/shared-lib).
ext_hits="$(git ls-files | grep -E '\.(rlib|rmeta|rcgu\.o|o|a|so|dylib|wasm)$' || true)"
if [ -n "$ext_hits" ]; then
  note "tracked files with a compiled-artifact extension (regenerable — do not commit):"
  echo "$ext_hits" | awk '{ print "  " $0 }' >&2
  fail=1
fi

# 3) No file over the size threshold. `git ls-files -z` + a stat loop keeps this
# robust to spaces/newlines in paths.
while IFS= read -r -d '' f; do
  [ -f "$f" ] || continue
  size=$(wc -c <"$f")
  if [ "$size" -gt "$MAX_BYTES" ]; then
    note "tracked file exceeds ${MAX_BYTES} bytes (${size} bytes): $f"
    fail=1
  fi
done < <(git ls-files -z)

if [ "$fail" -ne 0 ]; then
  note "FAIL — remove the artifact from the index (git rm --cached <path>) and add a .gitignore rule."
  note "If a history rewrite is needed to purge it from past commits, that is a human-run step:"
  note "  docs/architecture/tbd/prune-git-history-from-binaries.md"
  exit 1
fi

echo "artifact-guard: OK — no tracked build artifacts, and no tracked file over ${MAX_BYTES} bytes."
