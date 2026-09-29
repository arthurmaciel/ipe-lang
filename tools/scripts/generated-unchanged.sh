#!/usr/bin/env bash
# Fail when a regenerated path no longer matches what is committed: a tracked
# file modified or deleted in the working tree, or a file the generator wrote
# that git does not track yet. `git diff --exit-code` alone is blind to that
# last case, so a generator emitting a new file would pass it.
#
# Usage: tools/scripts/generated-unchanged.sh PATH...
# Every PATH must name tracked content; a PATH matching nothing is refused
# rather than vacuously clean.
set -euo pipefail

if [ "$#" -eq 0 ]; then
  echo "generated-unchanged: no paths given" >&2
  exit 2
fi
for path in "$@"; do
  if ! git ls-files --error-unmatch -- "$path" >/dev/null 2>&1; then
    echo "generated-unchanged: '$path' matches no tracked file; refusing to call it clean" >&2
    exit 2
  fi
done

# Porcelain v1: column 2 is the working tree against the index, `?` for an
# untracked file. Staged-but-matching content is clean, as with `git diff`.
status=$(git status --porcelain=v1 --untracked-files=all --no-renames -- "$@")
drift=$(printf '%s\n' "$status" | awk 'length($0) > 2 && substr($0, 2, 1) != " "')
if [ -n "$drift" ]; then
  echo "generated-unchanged: regenerated output differs from the committed files:" >&2
  printf '%s\n' "$drift" >&2
  git --no-pager diff -- "$@" >&2 || true
  exit 1
fi
