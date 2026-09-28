# shellcheck shell=bash
# tools/scripts/lib/require-tool.sh — fail-closed external-tool dependency for
# gate scripts. SOURCE this (never execute it).
#
# A gate that decides pass/fail from a tool's exit code must tell "the tool
# ran and found nothing" apart from "the tool did not run" — a missing tool is
# an error, never a silent pass. `if rg …; then violations=1; fi` reads rg's
# exit 127 (command not found) the same as exit 1 (no match): the gate then
# passes vacuously whenever the tool is absent from PATH.

# require_tool <name> [<name> ...]: exit 2 naming every missing tool. Call this
# before any command whose absence must not read as a clean gate.
require_tool() {
    local missing=() t
    for t in "$@"; do
        command -v "$t" >/dev/null 2>&1 || missing+=("$t")
    done
    if [ "${#missing[@]}" -gt 0 ]; then
        echo "require_tool: missing required tool(s): ${missing[*]}" >&2
        exit 2
    fi
}

# rg_status <exit-code>: classify an rg exit code. rg's own contract is 0 =
# match, 1 = no match; every other code (2 = rg-reported error, 127 = command
# not found, …) is a hard failure, never "no match".
rg_status() {
    case "$1" in
        0) echo match ;;
        1) echo no-match ;;
        *) echo error ;;
    esac
}

# match_or_fail <description> -- <command...>: run an rg-shaped command
# (stdout/stderr pass through unredirected, so matches stay visible), classify
# its exit code with rg_status, and hard-exit 2 on anything but 0/1. Returns 0
# when the command matched, 1 when it cleanly found nothing — mirrors rg's own
# convention so callers can write `if match_or_fail … -- rg …; then`.
match_or_fail() {
    local desc="$1"; shift
    [ "${1:-}" = "--" ] && shift
    local rc=0
    "$@" || rc=$?
    case "$(rg_status "$rc")" in
        match) return 0 ;;
        no-match) return 1 ;;
        error)
            echo "match_or_fail: $desc: command exited $rc (neither match nor no-match) — treating as a hard failure" >&2
            exit 2
            ;;
    esac
}

# match_capture <var> <description> -- <command...>: like match_or_fail, but
# for a caller that needs the matched TEXT, not just whether it matched.
# Captures the command's stdout into <var> (via a plain shell variable — the
# caller reads it as text, one hit per line for an rg/grep-shaped command) and
# classifies its exit code exactly like match_or_fail: 0 -> match (returns 0,
# var holds the hits), 1 -> clean no-match (returns 1, var holds whatever the
# command printed, normally empty), anything else -> hard-exit 2. A caller
# piping into grep/rg (git ls-files | grep -E …) should wrap the pipeline in a
# shell function first, since this runs its argv directly (no shell), not a
# pipeline string.
match_capture() {
    local __mc_var="$1" desc="$2"; shift 2
    [ "${1:-}" = "--" ] && shift
    local __mc_out rc=0
    __mc_out="$("$@")" || rc=$?
    case "$(rg_status "$rc")" in
        match)    printf -v "$__mc_var" '%s' "$__mc_out"; return 0 ;;
        no-match) printf -v "$__mc_var" '%s' "$__mc_out"; return 1 ;;
        error)
            printf -v "$__mc_var" '%s' ""
            echo "match_capture: $desc: command exited $rc (neither match nor no-match) — treating as a hard failure" >&2
            exit 2
            ;;
    esac
}

# require_scan_root <dir> <glob>: exit 2 when <dir> does not exist, or exists
# but has no file matching <glob> anywhere under it. A directory-scanning gate
# that decides pass/fail from a scan tool's exit code must also tell "the root
# is real and has something to scan" apart from "the root is missing or
# empty" — both currently read as the scan tool's own exit 1 (no match), so a
# typo'd override or an emptied fixture directory passes the gate vacuously
# instead of erroring (issue #3036).
require_scan_root() {
    local dir="$1" glob="$2"
    if [ ! -d "$dir" ]; then
        echo "require_scan_root: missing scan root: $dir" >&2
        exit 2
    fi
    local hit
    hit="$(find "$dir" -type f -name "$glob" -print -quit 2>/dev/null)"
    if [ -z "$hit" ]; then
        echo "require_scan_root: no file matching '$glob' under $dir — nothing to scan" >&2
        exit 2
    fi
}
