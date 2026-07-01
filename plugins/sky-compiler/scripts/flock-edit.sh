#!/usr/bin/env bash
# flock-edit — per-file advisory write lock for parallel swarm executors.
#
# ENFORCED POLICY (data-race protocol): before ANY agent opens a file for
# writing (Edit/Write), it MUST acquire that file's lock; after writing, it MUST
# release it. Serialises concurrent writes to a shared file (e.g. the kernel
# registry) so parallel agents never clobber each other. A stale lock (holder
# died) is stolen after LOCK_STALE_SECS so a dead agent cannot deadlock the swarm.
#
# Usage:
#   flock-edit.sh acquire <abs-file-path>   # blocks until held (or steals stale); prints OK
#   flock-edit.sh release <abs-file-path>   # frees the lock
#   flock-edit.sh with <abs-file-path> -- <cmd...>   # acquire, run cmd, release (even on failure)
#
# The lock is a directory (mkdir is atomic on POSIX) under $LOCK_DIR, keyed by a
# hash of the absolute path. A holder writes its PID + epoch into the dir so a
# waiter can detect + reap a stale holder.
set -uo pipefail

LOCK_DIR="${IPE_LOCK_DIR:-/tmp/ipe-swarm-locks}"
LOCK_STALE_SECS="${IPE_LOCK_STALE_SECS:-90}"
LOCK_WAIT_MAX="${IPE_LOCK_WAIT_MAX:-300}"   # give up after this many seconds

mkdir -p "$LOCK_DIR" 2>/dev/null

_key() { printf '%s' "$1" | cksum | tr -d ' \t' ; }
_now() { date +%s ; }   # date is fine here (script, not workflow JS)

acquire() {
  local file="$1" dir; dir="$LOCK_DIR/$(_key "$file")"
  local waited=0
  while : ; do
    if mkdir "$dir" 2>/dev/null; then
      printf '%s\n' "$$" > "$dir/pid"
      _now > "$dir/ts"
      printf '%s\n' "$file" > "$dir/file"
      echo "OK acquired: $file"
      return 0
    fi
    # Held — check staleness.
    local ts; ts="$(cat "$dir/ts" 2>/dev/null || echo 0)"
    local age=$(( $(_now) - ts ))
    if [ "$age" -ge "$LOCK_STALE_SECS" ]; then
      echo "STALE lock on $file (age ${age}s) — stealing"
      rm -rf "$dir" 2>/dev/null
      continue
    fi
    sleep 1
    waited=$((waited + 1))
    if [ "$waited" -ge "$LOCK_WAIT_MAX" ]; then
      echo "TIMEOUT waiting for lock on $file after ${waited}s" >&2
      return 1
    fi
  done
}

release() {
  local file="$1" dir; dir="$LOCK_DIR/$(_key "$file")"
  rm -rf "$dir" 2>/dev/null
  echo "OK released: $file"
}

case "${1:-}" in
  acquire) shift; acquire "$1" ;;
  release) shift; release "$1" ;;
  with)
    shift; file="$1"; shift
    [ "${1:-}" = "--" ] && shift
    acquire "$file" || exit 1
    "$@"; rc=$?
    release "$file"
    exit "$rc"
    ;;
  *) echo "usage: flock-edit.sh {acquire|release|with} <file> [-- cmd...]" >&2; exit 2 ;;
esac
