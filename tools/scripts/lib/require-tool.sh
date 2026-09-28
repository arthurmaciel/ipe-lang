# shellcheck shell=bash
# tools/scripts/lib/require-tool.sh — fail-closed external-tool dependency for
# gate scripts. SOURCE this (never execute it).
#
# A gate that decides pass/fail from a tool's exit code must tell "the tool
# ran and found nothing" apart from "the tool did not run" — a missing tool is
# an error, never a silent pass. `if rg …; then violations=1; fi` reads rg's
# exit 127 (command not found) the same as exit 1 (no match): the gate then
# passes vacuously whenever the tool is absent from PATH.
#
# The same holds for a gate's INPUT: a producer (git, find, sort) that fails
# part-way yields a partial set, and a scan over a partial set reads as clean.
# Every helper here therefore runs exactly one direct executable and classifies
# that command's own exit code; none accepts a pipeline.

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

# _require_out_name <caller> <name>: exit 2 unless <name> is a valid shell
# identifier outside the reserved `__` prefix. Every helper that writes a
# caller variable (`printf -v`, `mapfile`, `local -n`) binds that name under
# bash's dynamic scope, so a name equal to one of the helper's own locals would
# land in the local and vanish on return. Helper locals all carry a `__<tag>_`
# prefix; refusing that prefix for an output name rules the collision out.
_require_out_name() {
    case "$2" in
        __*)
            echo "$1: output variable '$2' uses the reserved '__' prefix — pick a name without it" >&2
            exit 2
            ;;
    esac
    if ! [[ "$2" =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]]; then
        echo "$1: output variable '$2' is not a valid shell identifier" >&2
        exit 2
    fi
}

# _require_direct_command <caller> <description> <command-word>: exit 2 unless
# <command-word> resolves to an executable file that treats its arguments as
# data. A shell function, alias, keyword, or builtin (`eval`, `source`, `.`,
# `command`, `builtin`, `exec`) can run a pipeline whose failing first stage is
# masked by a later stage's clean "no match" — under pipefail a producer's rc
# 128 followed by a matcher's rc 1 reports rc 1. A shell interpreter or a
# command launcher (`env`, `xargs`, `timeout`, …) hides the same pipeline one
# level down, so those executables are refused too.
_require_direct_command() {
    local __dc_kind
    __dc_kind="$(type -t -- "${3:-}" 2>/dev/null || true)"
    if [ "$__dc_kind" != file ]; then
        echo "$1: $2: '${3:-}' is ${__dc_kind:-not a command} — pass an executable file, never a builtin, shell function, alias, or pipeline" >&2
        exit 2
    fi
    case "${3##*/}" in
        bash|sh|dash|zsh|ksh|mksh|ash|busybox|env|xargs|nice|nohup|timeout|stdbuf|setsid|sudo|doas)
            echo "$1: $2: '$3' runs another command line — pass the scanning executable itself, never a shell or launcher" >&2
            exit 2
            ;;
    esac
}

# match_or_fail <description> -- <command...>: run an rg-shaped command
# (stdout/stderr pass through unredirected, so matches stay visible), classify
# its exit code with rg_status, and hard-exit 2 on anything but 0/1. Returns 0
# when the command matched, 1 when it cleanly found nothing — mirrors rg's own
# convention so callers can write `if match_or_fail … -- rg …; then`.
# The command must be a direct executable. To match a producer's output,
# capture the producer first (capture_nul or a checked `$(…)`), then feed the
# matcher a herestring.
match_or_fail() {
    local __mf_desc="$1"; shift
    [ "${1:-}" = "--" ] && shift
    _require_direct_command match_or_fail "$__mf_desc" "${1:-}"
    local __mf_rc=0
    "$@" || __mf_rc=$?
    case "$(rg_status "$__mf_rc")" in
        match) return 0 ;;
        no-match) return 1 ;;
        error)
            echo "match_or_fail: $__mf_desc: command exited $__mf_rc (neither match nor no-match) — treating as a hard failure" >&2
            exit 2
            ;;
    esac
}

# match_capture <var> <description> -- <command...>: like match_or_fail, but
# captures the command's stdout into <var> for a caller that needs the matched
# TEXT. 0 -> match (returns 0), 1 -> clean no-match (returns 1), anything else
# -> hard-exit 2. The command must be a direct executable — never a pipeline,
# nor a function wrapping one: capture the producer first (checked), then match
# on a herestring.
match_capture() {
    _require_out_name match_capture "${1:-}"
    local __mc_var="$1" __mc_desc="$2"; shift 2
    [ "${1:-}" = "--" ] && shift
    _require_direct_command match_capture "$__mc_desc" "${1:-}"
    local __mc_out __mc_rc=0
    __mc_out="$("$@")" || __mc_rc=$?
    case "$(rg_status "$__mc_rc")" in
        match)    printf -v "$__mc_var" '%s' "$__mc_out"; return 0 ;;
        no-match) printf -v "$__mc_var" '%s' "$__mc_out"; return 1 ;;
        error)
            printf -v "$__mc_var" '%s' ""
            echo "match_capture: $__mc_desc: command exited $__mc_rc (neither match nor no-match) — treating as a hard failure" >&2
            exit 2
            ;;
    esac
}

# capture_nul <array-var> <description> -- <command...>: run a producer that
# prints NUL-delimited records (`git ls-files -z`, `find … -print0`) and load
# them into <array-var>. Any non-zero producer exit hard-exits 2 — a producer
# that died part-way yields a partial set, and a partial set must never stand
# in for the whole one. The command must be a direct executable.
capture_nul() {
    _require_out_name capture_nul "${1:-}"
    _capture_nul_into "$@"
}

# _capture_nul_into: capture_nul without the output-name check, for the lib's
# own `__`-prefixed targets.
_capture_nul_into() {
    local __cn_var="$1" __cn_desc="$2"; shift 2
    [ "${1:-}" = "--" ] && shift
    _require_direct_command capture_nul "$__cn_desc" "${1:-}"
    local __cn_tmp __cn_rc=0
    __cn_tmp="$(mktemp)" || { echo "capture_nul: $__cn_desc: mktemp failed" >&2; exit 2; }
    "$@" >"$__cn_tmp" || __cn_rc=$?
    if [ "$__cn_rc" -ne 0 ]; then
        rm -f "$__cn_tmp"
        echo "capture_nul: $__cn_desc: producer exited $__cn_rc — refusing a possibly partial set" >&2
        exit 2
    fi
    mapfile -d '' -t "$__cn_var" <"$__cn_tmp"
    rm -f "$__cn_tmp"
}

# enumerate_files <array-var> <glob> <root...>: load every regular file named
# <glob> under the roots into <array-var>, byte-order sorted and NUL-safe.
# Hard-exits 2 when no root is given, a root is not a directory, find or sort
# exits non-zero (an unreadable subtree), or the set is empty — a scan over a
# missing, partial, or empty file set must never read as a clean scan.
enumerate_files() {
    _require_out_name enumerate_files "${1:-}"
    _enumerate_files_into "$@"
}

# _enumerate_files_into: enumerate_files without the output-name check, for
# the lib's own `__`-prefixed targets.
_enumerate_files_into() {
    local __ef_var="$1" __ef_glob="$2"; shift 2
    if [ "$#" -eq 0 ]; then
        echo "enumerate_files: no scan root given for '$__ef_glob'" >&2
        exit 2
    fi
    local __ef_root
    for __ef_root in "$@"; do
        if [ ! -d "$__ef_root" ]; then
            echo "enumerate_files: missing scan root: $__ef_root" >&2
            exit 2
        fi
    done
    local -a __ef_found=()
    _capture_nul_into __ef_found "find '$__ef_glob' under $*" -- \
        find "$@" -type f -name "$__ef_glob" -print0
    if [ "${#__ef_found[@]}" -eq 0 ]; then
        echo "enumerate_files: no file matching '$__ef_glob' under $* — nothing to scan" >&2
        exit 2
    fi
    local __ef_tmp
    __ef_tmp="$(mktemp)" || { echo "enumerate_files: mktemp failed" >&2; exit 2; }
    printf '%s\0' "${__ef_found[@]}" >"$__ef_tmp"
    local -a __ef_sorted=()
    local -x LC_ALL=C
    _capture_nul_into __ef_sorted "sort '$__ef_glob' set" -- sort -z "$__ef_tmp"
    rm -f "$__ef_tmp"
    local -n __ef_out="$__ef_var"
    __ef_out=("${__ef_sorted[@]}")
}

# require_scan_root <dir> <glob>: exit 2 when <dir> does not exist, find
# cannot walk it, or it has no file matching <glob> anywhere under it — a
# missing root or an emptied fixture directory must never let a scan tool's
# own "no match" pass the gate vacuously.
require_scan_root() {
    local -a __rsr_files=()
    _enumerate_files_into __rsr_files "$2" "$1"
}
