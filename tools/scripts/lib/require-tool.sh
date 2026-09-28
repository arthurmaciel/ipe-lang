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
