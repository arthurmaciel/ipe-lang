#!/usr/bin/env bash
# Fail when a regenerated path no longer matches what is committed: a tracked
# file modified or deleted in the working tree, a file the generator wrote
# that git does not track yet, or one the ignore rules hide from git.
# `git diff --exit-code` alone is blind to the last two cases, so a generator
# emitting a new file would pass it.
#
# Usage: tools/scripts/generated-unchanged.sh PATH...
# Every PATH must name tracked content; a PATH matching nothing is refused
# rather than vacuously clean.
#
# Status is read as `git status --porcelain=v2 -z`: NUL-terminated records,
# so no path is quoted or split, and every record type is handled by name.
# Staged-but-matching content is clean, as with `git diff`.
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

status_file=$(mktemp)
trap 'rm -f "$status_file"' EXIT
git status --porcelain=v2 -z --untracked-files=all --ignored=matching --no-renames -- "$@" >"$status_file"

drift=()
while IFS= read -r -d '' record; do
  case "$record" in
    '1 '*)
      # `1 XY sub mH mI mW hH hI path`: Y is the working tree against the index.
      if [ "${record:3:1}" != "." ]; then
        drift+=("changed: ${record#* * * * * * * * }")
      fi
      ;;
    '2 '*)
      # A rename or copy record carries its origin as one more record.
      IFS= read -r -d '' origin || origin=""
      drift+=("renamed: ${record#* * * * * * * * * } (from $origin)")
      ;;
    'u '*)
      drift+=("unmerged: ${record#* * * * * * * * * * }")
      ;;
    '? '*)
      drift+=("untracked: ${record#? }")
      ;;
    '! '*)
      drift+=("ignored: ${record#! } (the ignore rules hide this generated output from git)")
      ;;
    *)
      echo "generated-unchanged: unreadable git status record '$record'; refusing to call it clean" >&2
      exit 2
      ;;
  esac
done <"$status_file"

if [ "${#drift[@]}" -gt 0 ]; then
  echo "generated-unchanged: regenerated output differs from the committed files:" >&2
  printf '  %s\n' "${drift[@]}" >&2
  git --no-pager diff -- "$@" >&2 || true
  exit 1
fi
