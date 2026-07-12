#!/usr/bin/env bash
# scripts/disk-guard.sh — disk-space kill-switch for the Ipê (Sky->Rust) dev
# sessions, sibling to mem-guard.sh.
#
# Background: this session runs several parallel `git worktree` build lanes,
# each with its own CARGO_TARGET_DIR under ~/.cache/sky-rust-target-<lane>.
# A cold multi-lane rebuild can burn 15-30GB per lane; left unwatched, disk
# has hit <6GB free multiple times in one session, each time requiring a
# human-in-the-loop reclaim. This watchdog polls free disk space on `/` and
# reclaims disposable Cargo/sccache caches BEFORE the disk fills, in a fixed
# safety order, without ever touching git-tracked source or a target dir
# with a live compiler process still writing to it.
#
# Load-bearing safety invariant: this script ONLY ever deletes paths under
# ~/.cache/sky-rust-target-* and ~/.cache/sccache — disposable build caches
# that regenerate on the next `cargo build`. It NEVER touches a repo
# worktree, `.git`, or any tracked file. Reclaiming a target dir never loses
# WORK (work lives in git commits) — it only costs a future rebuild.
#
# Sandbox enforcement: every deletion routes through safe_rm_rf(), the ONLY
# function permitted to call `rm -rf`. It re-derives and re-validates the
# path from scratch at the point of destruction (never trusts a caller) —
# canonicalizes via `realpath -m` (so a symlink can't smuggle a path outside
# ~/.cache), requires the resolved path's PARENT be exactly the resolved
# ~/.cache (not a prefix/substring match), and requires the basename match
# the allowlist exactly (`sky-rust-target-*` or `sccache`). Refuses on any
# empty/unset-variable/`/`/`$HOME` input. See safe_rm_rf()'s own comment.
#
# Usage:
#   ./scripts/disk-guard.sh                             # foreground, logs to stderr + /tmp/disk-guard.log
#   nohup ./scripts/disk-guard.sh >/tmp/disk-guard.out 2>&1 & disown   # background for the session
#   DISK_GUARD_DRY=1 ./scripts/disk-guard.sh            # log-only rehearsal, never deletes
#
# Tunables (env vars, all optional):
#   DISK_GUARD_WARN_GB       log-only warning floor (GB).            default 20
#   DISK_GUARD_RECLAIM_GB    start reclaiming below this (GB).       default 10
#   DISK_GUARD_PANIC_GB      reclaim PROTECTED dirs too (GB).        default 5
#   DISK_GUARD_INTERVAL      poll interval (seconds).                default 15
#   DISK_GUARD_LOG           log file path.                          default /tmp/disk-guard.log
#   DISK_GUARD_PROTECT_FILE  one substring per line; a target dir     default /tmp/disk-guard-protect.txt
#                            whose name contains any listed substring
#                            is skipped except at PANIC. Missing file
#                            = nothing protected. Editable live.
#   DISK_GUARD_MOUNT         filesystem to watch.                    default /
#   DISK_GUARD_DRY           set to 1 to log only, never delete.     default unset
#
# Reclaim order per pass (stops as soon as the RECLAIM floor is cleared):
#   1. ~/.cache/sccache            — self-healing; a cache-miss, not data loss.
#   2. Orphaned target dirs        — no matching `git worktree list` entry left.
#   3. Non-protected target dirs   — 0 live rustc/cargo procs, sorted largest first.
#   4. [PANIC tier only] Protected target dirs — 0 live procs, largest first.
#
# The one non-negotiable gate: a target dir is NEVER removed while any
# process has it open (checked via `pgrep -f <exact-path>` on the target
# dir's absolute path) — regardless of tier or protection status. A live
# writer always wins over a disk-space goal.

set -euo pipefail

WARN_GB="${DISK_GUARD_WARN_GB:-20}"
RECLAIM_GB="${DISK_GUARD_RECLAIM_GB:-10}"
PANIC_GB="${DISK_GUARD_PANIC_GB:-5}"
INTERVAL="${DISK_GUARD_INTERVAL:-15}"
LOG="${DISK_GUARD_LOG:-/tmp/disk-guard.log}"
PROTECT_FILE="${DISK_GUARD_PROTECT_FILE:-/tmp/disk-guard-protect.txt}"
MOUNT="${DISK_GUARD_MOUNT:-/}"
DRY="${DISK_GUARD_DRY:-}"

CACHE_ROOT="${HOME}/.cache"
SCCACHE_DIR="${CACHE_ROOT}/sccache"
TARGET_GLOB="${CACHE_ROOT}/sky-rust-target-*"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

log() {
    printf '[%s] %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$*" | tee -a "$LOG" >&2
}

# Free space on $MOUNT, in whole GB (floor).
free_gb() {
    df -BG --output=avail "$MOUNT" 2>/dev/null | tail -1 | tr -dc '0-9'
}

# True (0) if any process has $1 (an absolute path) anywhere in its argv/cwd.
path_is_live() {
    local path="$1"
    pgrep -f -- "$path" > /dev/null 2>&1
}

# Substring-match $1 (a dir basename) against every line in PROTECT_FILE.
is_protected() {
    local name="$1"
    [[ -f "$PROTECT_FILE" ]] || return 1
    local pat
    while IFS= read -r pat; do
        [[ -z "$pat" ]] && continue
        [[ "$name" == *"$pat"* ]] && return 0
    done < "$PROTECT_FILE"
    return 1
}

# Does a git worktree still reference this target dir's lane? We infer the
# lane name from the dir suffix (sky-rust-target-<lane>) and check whether
# `.claude/worktrees/agent-<lane>` still exists as a registered worktree.
is_orphaned() {
    local dir="$1"
    local base lane
    base="$(basename "$dir")"
    lane="${base#sky-rust-target-}"
    [[ -z "$lane" || "$lane" == "$base" ]] && return 1   # not a lane-suffixed dir (e.g. the bare shared target)
    local wt="${REPO_ROOT}/.claude/worktrees/agent-${lane}"
    [[ -d "$wt" ]] && return 1
    return 0
}

# ---------------------------------------------------------------------------
# SANDBOX BOUNDARY. This is the ONLY function in this script permitted to
# call `rm -rf`, and every path reaching it (however it was derived — glob
# expansion, du/sort/awk pipelines, caller typos) is re-validated from
# scratch here before deletion. Never trust a caller's path; always
# re-derive and re-check it at the point of destruction.
#
# Checks, in order (any failure REFUSES and returns non-zero, no deletion):
#   1. Non-empty, and not literally "/" or "$HOME" — the classic
#      unset-variable-collapses-to-a-root-path footgun.
#   2. Canonicalizes via `realpath -m` (resolves symlinks, `..`, etc.) —
#      so a target dir that's secretly a symlink pointing outside ~/.cache
#      can't be used to escape the sandbox.
#   3. The canonicalized path's PARENT must be exactly the canonicalized
#      $CACHE_ROOT (~/.cache) — not a substring/prefix match (which
#      `~/.cache-evil-twin/...` would pass), not a deeper-nested path.
#   4. The basename must match the allowlist EXACTLY: `sky-rust-target-*`
#      or `sccache`. Nothing else, ever.
# ---------------------------------------------------------------------------
safe_rm_rf() {
    local raw="$1" reason="$2"

    if [[ -z "$raw" || "$raw" == "/" || "$raw" == "$HOME" || "$raw" == "$HOME/" ]]; then
        log "REFUSED (empty/root/home path '$raw') reason=$reason"
        return 1
    fi

    local real cache_real parent base
    real="$(realpath -m -- "$raw" 2>/dev/null)" || { log "REFUSED (unresolvable path '$raw') reason=$reason"; return 1; }
    cache_real="$(realpath -m -- "$CACHE_ROOT" 2>/dev/null)" || { log "REFUSED (cannot resolve CACHE_ROOT)"; return 1; }

    if [[ -z "$real" || "$real" == "/" || "$real" == "$cache_real" ]]; then
        log "REFUSED (resolved path '$real' is empty/root/cache-root-itself) reason=$reason"
        return 1
    fi

    parent="$(dirname -- "$real")"
    if [[ "$parent" != "$cache_real" ]]; then
        log "REFUSED (resolved path '$real' is not a direct child of '$cache_real') reason=$reason"
        return 1
    fi

    base="$(basename -- "$real")"
    if [[ "$base" != sky-rust-target-* && "$base" != "sccache" ]]; then
        log "REFUSED (basename '$base' not in allowlist: sky-rust-target-*, sccache) reason=$reason"
        return 1
    fi

    if [[ ! -e "$real" ]]; then
        log "SKIP (already gone) $real"
        return 0
    fi

    # sccache runs a background SERVER process that does NOT put the cache
    # directory path in its own argv (it reads SCCACHE_DIR/a config default
    # instead) — path_is_live's `pgrep -f <path>` therefore CANNOT see it,
    # and a bare `rm -rf` here would delete a live server's storage out from
    # under it. Stop the server gracefully first; a later `cargo build`
    # auto-restarts a fresh one pointed at the (now possibly-recreated)
    # directory. Real deletion only — a DRY run must not stop a live server.
    if [[ "$base" == "sccache" && -z "$DRY" ]] && command -v sccache > /dev/null 2>&1; then
        log "  stopping sccache server before reclaim"
        sccache --stop-server > /dev/null 2>&1 || true
    fi

    if path_is_live "$real"; then
        log "  SKIP (live process) $real"
        return 1
    fi

    local size
    size="$(du -sh "$real" 2>/dev/null | cut -f1)"
    if [[ -n "$DRY" ]]; then
        log "DRY-RUN would remove $real (${size:-?}) reason=$reason"
        return 0
    fi

    log "RECLAIM $real (${size:-?}) reason=$reason"
    rm -rf -- "$real"
    return 0
}

reclaim_dir() {
    local dir="$1" reason="$2"
    safe_rm_rf "$dir" "$reason"
}

# One reclaim pass. Returns once free_gb >= RECLAIM_GB or every tier is
# exhausted. $1 = 1 to also spend the PANIC tier (protected dirs).
reclaim_pass() {
    local allow_protected="$1"

    # Tier 1: sccache.
    if [[ -d "$SCCACHE_DIR" ]] && [[ "$(free_gb)" -lt "$RECLAIM_GB" ]]; then
        reclaim_dir "$SCCACHE_DIR" "sccache (self-healing)" || true
    fi
    [[ "$(free_gb)" -ge "$RECLAIM_GB" ]] && return 0

    # Tier 2: orphaned target dirs (no worktree left).
    local dir
    for dir in $TARGET_GLOB; do
        [[ -d "$dir" ]] || continue
        [[ "$(free_gb)" -ge "$RECLAIM_GB" ]] && return 0
        is_orphaned "$dir" && reclaim_dir "$dir" "orphaned (no worktree)" || true
    done
    [[ "$(free_gb)" -ge "$RECLAIM_GB" ]] && return 0

    # Tier 3: non-protected active lanes, largest first.
    while IFS= read -r dir; do
        [[ -z "$dir" ]] && continue
        [[ "$(free_gb)" -ge "$RECLAIM_GB" ]] && return 0
        local base; base="$(basename "$dir")"
        is_protected "$base" && continue
        reclaim_dir "$dir" "active lane, non-protected" || true
    done < <(du -sk $TARGET_GLOB 2>/dev/null | sort -rn | awk '{ $1=""; sub(/^ /,""); print }')
    [[ "$(free_gb)" -ge "$RECLAIM_GB" ]] && return 0

    # Tier 4: PANIC only — protected dirs too.
    if [[ "$allow_protected" == "1" ]]; then
        while IFS= read -r dir; do
            [[ -z "$dir" ]] && continue
            [[ "$(free_gb)" -ge "$RECLAIM_GB" ]] && return 0
            reclaim_dir "$dir" "PANIC: protected lane, last resort" || true
        done < <(du -sk $TARGET_GLOB 2>/dev/null | sort -rn | awk '{ $1=""; sub(/^ /,""); print }')
    fi
}

trap 'log "stopping (signal)"; exit 0' INT TERM

log "starting (warn=${WARN_GB}GB reclaim=${RECLAIM_GB}GB panic=${PANIC_GB}GB poll=${INTERVAL}s mount=${MOUNT} protect_file=${PROTECT_FILE} dry=${DRY:-no})"

while :; do
    free="$(free_gb)"

    if [[ "$free" -lt "$PANIC_GB" ]]; then
        log "PANIC: ${free}GB free (< ${PANIC_GB}GB) — reclaiming including protected dirs"
        reclaim_pass 1
    elif [[ "$free" -lt "$RECLAIM_GB" ]]; then
        log "RECLAIM: ${free}GB free (< ${RECLAIM_GB}GB) — reclaiming non-protected dirs"
        reclaim_pass 0
    elif [[ "$free" -lt "$WARN_GB" ]]; then
        log "warn: ${free}GB free (< ${WARN_GB}GB), above reclaim floor — watching"
    fi

    sleep "$INTERVAL"
done
