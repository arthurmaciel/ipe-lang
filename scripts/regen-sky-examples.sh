#!/usr/bin/env bash
# scripts/regen-sky-examples.sh — regenerate the committed Sky example trees.
#
# Fetches every manifest example from the upstream GitHub repo (anzellai/sky)
# and writes two committed trees:
#   examples/sky/original/<name>/  — the raw upstream Sky source, verbatim.
#   examples/sky/ipe/<name>/       — the runnable Ipê port (rename-map token
#                                    rewrite + the content-anchored
#                                    ipe-edits/<name>.edits semantic delta).
#
# After a fresh clone, run this once, then `cd examples/sky/ipe/<name> && ipe run`.
# CI runs it regularly to track upstream; a drift PR lands any refresh.
#
# USAGE
#   scripts/regen-sky-examples.sh [--only "NAME ..."]   regenerate (default: all)
#   scripts/regen-sky-examples.sh --check               regenerate to a temp and
#                                                       fail if it differs from the
#                                                       committed trees (CI gate).
#
# Exit: 0 ok · 1 regeneration produced nothing / --check found drift · 2 setup.
set -uo pipefail

source "$(dirname "$0")/lib/env.sh"
source "$(dirname "$0")/lib/mirror.sh"

[ -n "$REPO" ] && cd "$REPO" || { echo "regen: cannot locate repo (set IPE_REPO)" >&2; exit 2; }

MODE=regen
ONLY=""
while [ $# -gt 0 ]; do
  case "$1" in
    --check) MODE=check ;;
    --only)  ONLY="${2:-}"; shift ;;
    -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
    *) echo "regen: unknown arg '$1'" >&2; exit 2 ;;
  esac
  shift
done

# Regenerate a set of examples into <original-root> <ipe-root>. Echoes a count
# summary; returns non-zero if nothing regenerated.
_regen_into() {
  local oroot="$1" iroot="$2" names name ok=0 editfail=0 fail=0 rc
  names="$(sky_example_names)" || { echo "regen: cannot read manifest" >&2; return 2; }
  [ -n "$ONLY" ] && names="$ONLY"
  for name in $names; do
    [ -z "$name" ] && continue
    sky_mirror_one "$name" "$oroot/$name" "$iroot/$name"; rc=$?
    case "$rc" in
      0) ok=$((ok+1)) ;;
      2) editfail=$((editfail+1)); echo "regen: FAILED edit for $name" >&2 ;;
      *) fail=$((fail+1)); echo "regen: no source for $name" >&2 ;;
    esac
  done
  echo "regen: $ok ok${fail:+, $fail no-source}${editfail:+, $editfail edit-fail}"
  [ "$editfail" = 0 ] && [ "$ok" -gt 0 ]
}

if [ "$MODE" = check ]; then
  # OFFLINE consistency gate: re-derive each ipe/ port from the COMMITTED
  # original/ tree (no network) and diff vs the committed ipe/ port. This proves
  # the committed ports match the current rename-map + ipe-edits — it does NOT
  # re-fetch upstream (upstream drift is the nightly refresh's job, not a gate).
  [ -d examples/sky/original ] || { echo "regen --check: examples/sky/original/ missing — run regen first" >&2; exit 1; }
  tmp="$(mktemp -d)" || exit 2
  trap 'rm -rf "$tmp"' EXIT
  drift=0 checked=0
  names="$(sky_example_names)" || { echo "regen --check: cannot read manifest" >&2; exit 2; }
  [ -n "$ONLY" ] && names="$ONLY"
  for name in $names; do
    [ -z "$name" ] && continue
    src="examples/sky/original/$name"
    [ -d "$src" ] || { echo "regen --check: examples/sky/original/$name missing (run regen)" >&2; drift=1; continue; }
    if ! sky_transform_one "$name" "$src" "$tmp/$name"; then
      echo "regen --check: transform/edits failed for $name" >&2; drift=1; continue
    fi
    checked=$((checked+1))
    # Exclude gitignored build artefacts (out/, target/, .ipe caches, …): a local
    # `ipe build`/`run` leaves them in the working tree, but the fresh transform
    # never builds, so they are not drift.
    dx=(-x out -x target -x sky-out -x .ipe -x .ipecache -x .ipedeps -x node_modules -x .sky-src)
    if ! diff -qr "${dx[@]}" "examples/sky/ipe/$name" "$tmp/$name" >/dev/null 2>&1; then
      echo "regen --check: examples/sky/ipe/$name differs from re-deriving it from original/:" >&2
      diff -qr "${dx[@]}" "examples/sky/ipe/$name" "$tmp/$name" 2>&1 | sed 's/^/  /' >&2
      drift=1
    fi
  done
  if [ "$drift" = 1 ]; then
    echo "" >&2
    echo "regen --check: committed ports are stale vs rename-map/ipe-edits. Run:" >&2
    echo "  scripts/regen-sky-examples.sh && git add examples/sky/original examples/sky/ipe" >&2
    exit 1
  fi
  echo "regen --check: $checked committed port(s) match a fresh transform of original/."
  exit 0
fi

_regen_into "$SKY_ORIGINAL_DIR" "$SKY_IPE_DIR" || exit 1
echo "regen: wrote examples/sky/original/ + examples/sky/ipe/"
