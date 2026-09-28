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
# Every helper here therefore runs exactly one executable drawn from an
# allowlist of known exit contracts, classifies that command's own exit code,
# and checks every write into the caller's variable; anything else exits 2.

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

# _require_out_name <caller> <name> <scalar|array>: exit 2 unless <name> is a
# safe target for a helper's write of that shape. Positive rule: a lowercase
# identifier starting with a letter, either unset or declared by the caller
# with exactly the shape the helper writes — a plain scalar (`declare --`) for
# a scalar write, an indexed array (`declare -a`) for an array write, since a
# scalar write into an array keeps its stale elements. That excludes, by
# construction:
# - every helper local (all carry a `__<tag>_` prefix), which would capture a
#   write under bash's dynamic scope and drop it on return;
# - `_` and every bash special or environment variable (`BASH_VERSINFO`,
#   `GROUPS`, `FUNCNAME`, `RANDOM`, `PATH`, …), all of which are uppercase or
#   `_`, where bash drops, clobbers, or refuses the write;
# - a readonly, nameref, integer, case-folding, or associative target, whose
#   write fails, redirects, or rewrites the records.
_require_out_name() {
    if ! [[ "$2" =~ ^[a-z][a-z0-9_]*$ ]]; then
        echo "$1: output variable '$2' is not a lowercase identifier starting with a letter — bash specials, '_', and the helpers' reserved '__' names are refused" >&2
        exit 2
    fi
    local __on_decl
    __on_decl="$(declare -p -- "$2" 2>/dev/null)" || return 0
    local __on_want
    case "$3" in
        scalar) __on_want='declare -- ' ;;
        array) __on_want='declare -a ' ;;
        *) echo "$1: internal: unknown output shape '$3'" >&2; exit 2 ;;
    esac
    if [ "${__on_decl:0:${#__on_want}}" = "$__on_want" ]; then
        return 0
    fi
    # A plain scalar-vs-array cross (the common typo) gets a message naming
    # the shape mismatch directly; anything else (readonly, nameref,
    # integer, associative, …) keeps the generic attribute dump, since
    # calling e.g. a readonly array merely "an array" would hide the actual
    # blocker (its readonly-ness, not its shape).
    local __on_flags="${__on_decl#declare }"
    __on_flags="${__on_flags%% *}"
    case "$3:$__on_flags" in
        scalar:-a)
            echo "$1: output variable '$2' is an array, but this helper writes a scalar — pass an unset name or a plain scalar one" >&2
            ;;
        array:--)
            echo "$1: output variable '$2' is a scalar, but this helper writes an array — pass an unset name or a plain array (declare -a) one" >&2
            ;;
        *)
            echo "$1: output variable '$2' carries attributes (${__on_decl%% "$2"*}) — pass an unset name or a $3 one (${__on_want% })" >&2
            ;;
    esac
    exit 2
}

# Known exit contracts. A helper classifies an exit code only for a command
# whose contract gives that code a meaning; every other executable is refused,
# because an opaque runner (a shell, a launcher, an interpreter, `find -exec`,
# a symlink or hash entry disguising one) can report a masked pipeline's rc 1.
#
# _require_matcher_name <basename>: 0 when <basename> is a matcher whose
# contract is 0 = match, 1 = no match, >=2 = error.
_require_matcher_name() {
    case "$1" in
        rg|grep) return 0 ;;
        *) return 1 ;;
    esac
}
# _require_producer_name <basename>: 0 when <basename> is a producer whose
# contract is 0 = complete output, non-zero = failure (possibly partial).
_require_producer_name() {
    case "$1" in
        git|find|sort) return 0 ;;
        *) return 1 ;;
    esac
}

# _require_known_command <caller> <description> <matcher|producer> <command...>:
# exit 2 unless the command word is an executable file (never a builtin,
# keyword, alias, or function) whose invoked name AND resolved target (hash
# entry, symlinks followed) are both on the <contract> allowlist, and whose
# arguments stay inside that contract (no option or subcommand that hands
# control to another program: `rg --pre`, `find -exec`, `sort
# --compress-program`, any `git` use but `ls-files` and the bare
# `rev-parse --show-toplevel`).
_require_known_command() {
    local __kc_caller="$1" __kc_desc="$2" __kc_contract="$3"; shift 3
    local __kc_cmd="${1:-}" __kc_kind __kc_path __kc_real
    __kc_kind="$(type -t -- "$__kc_cmd" 2>/dev/null || true)"
    if [ "$__kc_kind" != file ]; then
        echo "$__kc_caller: $__kc_desc: '$__kc_cmd' is ${__kc_kind:-not a command} — pass an executable file, never a builtin, shell function, alias, or pipeline" >&2
        exit 2
    fi
    if ! { __kc_path="$(type -P -- "$__kc_cmd" 2>/dev/null)" && [ -n "$__kc_path" ] \
        && __kc_real="$(readlink -f -- "$__kc_path" 2>/dev/null)" && [ -n "$__kc_real" ]; }; then
        echo "$__kc_caller: $__kc_desc: cannot resolve '$__kc_cmd' to an executable file" >&2
        exit 2
    fi
    if ! "_require_${__kc_contract}_name" "${__kc_cmd##*/}" \
        || [ "${__kc_real##*/}" != "${__kc_cmd##*/}" ]; then
        echo "$__kc_caller: $__kc_desc: '$__kc_cmd' (resolves to $__kc_real) is not a known-contract $__kc_contract — see _require_${__kc_contract}_name" >&2
        exit 2
    fi
    local __kc_arg __kc_bad=""
    shift
    case "${__kc_cmd##*/}" in
        rg)
            for __kc_arg in "$@"; do
                case "$__kc_arg" in --pre|--pre=*) __kc_bad="$__kc_arg" ;; esac
            done
            ;;
        find)
            for __kc_arg in "$@"; do
                case "$__kc_arg" in -exec|-execdir|-ok|-okdir) __kc_bad="$__kc_arg" ;; esac
            done
            ;;
        sort)
            for __kc_arg in "$@"; do
                case "$__kc_arg" in --co*) __kc_bad="$__kc_arg" ;; esac
            done
            ;;
        git)
            case "${1:-}" in
                ls-files) ;;
                rev-parse) [ "$#" -eq 2 ] && [ "${2:-}" = --show-toplevel ] || __kc_bad="rev-parse ${*:2}" ;;
                *) __kc_bad="${1:-<no subcommand>}" ;;
            esac
            ;;
    esac
    if [ -n "$__kc_bad" ]; then
        echo "$__kc_caller: $__kc_desc: '$__kc_cmd $__kc_bad' is outside the known exit contract — it can hand control to another program" >&2
        exit 2
    fi
}

# _require_set_scalar <var> <value>: checked `printf -v` into a caller name.
_require_set_scalar() {
    printf -v "$1" '%s' "$2" || {
        echo "match_capture: could not write '$1'" >&2
        exit 2
    }
}

# _require_drop_var <name>: unset <name> in every dynamic scope that binds it
# — a caller's `local` shadows the global, so one `unset` only reveals the
# next binding — bounded by the call-stack depth plus the global scope.
# `unset -n` removes a nameref binding itself (a plain `unset` would follow
# the alias to its target and leave the exported name in place); `declare -p`
# reports the name's own binding rather than chasing an alias. Returns 1 when
# a binding survives (readonly).
_require_drop_var() {
    local -i __dv_left=$((${#FUNCNAME[@]} + 1))
    while declare -p "$1" >/dev/null 2>&1; do
        [ "$__dv_left" -gt 0 ] || return 1
        unset -n "$1" 2>/dev/null || true
        unset -v "$1" 2>/dev/null || true
        __dv_left=$((__dv_left - 1))
    done
}

# _require_contract_env <caller> <description> <basename>: in the subshell
# that is about to exec an allowlisted command, rewrite the environment to
# that command's contract — drop every variable through which the command
# reads outside configuration, pin the ones its output depends on — and exit
# 2 when the result differs from the contract. Per contract:
# - rg: RIPGREP_CONFIG_PATH (an options file: `--invert-match`, `--pre`).
# - grep: GREP_OPTIONS (injected options).
# - git: every `GIT_*` (`GIT_CONFIG_COUNT`/`KEY_n`/`VALUE_n` and
#   `GIT_CONFIG_PARAMETERS` inject config such as a `core.fsmonitor` hook
#   command; `GIT_INDEX_FILE`, `GIT_DIR`, `GIT_WORK_TREE` swap the tracked
#   set), then `GIT_CONFIG_NOSYSTEM=1` and `GIT_CONFIG_GLOBAL=/dev/null` so
#   only the repository's own config is read, and _capture_nul_rc adds
#   `-c core.fsmonitor=false` over that.
# - sort: LC_ALL=C (byte order, independent of the caller's locale).
# - find: no variable rewrites what `-type f -name … -print0` emits.
_require_contract_env() {
    local __ce_caller="$1" __ce_desc="$2" __ce_v
    local -a __ce_drop=() __ce_bad=()
    case "$3" in
        rg) __ce_drop=(RIPGREP_CONFIG_PATH) ;;
        grep) __ce_drop=(GREP_OPTIONS) ;;
        git)
            # `compgen -v` lists every binding, a nameref included; its
            # names are identifiers, so the unquoted split is exact.
            # shellcheck disable=SC2207
            __ce_drop=($(compgen -v GIT_ || true))
            ;;
        sort|find) ;;
        *) echo "$__ce_caller: internal: no environment contract for '$3'" >&2; exit 2 ;;
    esac
    for __ce_v in "${__ce_drop[@]}"; do
        _require_drop_var "$__ce_v" || __ce_bad+=("$__ce_v")
    done
    _require_env_refuse "$__ce_caller" "$__ce_desc" "$3" "${__ce_bad[@]}"
    case "$3" in
        git)
            export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null 2>/dev/null || true
            [ "${GIT_CONFIG_NOSYSTEM-}" = 1 ] || __ce_bad+=(GIT_CONFIG_NOSYSTEM)
            [ "${GIT_CONFIG_GLOBAL-}" = /dev/null ] || __ce_bad+=(GIT_CONFIG_GLOBAL)
            for __ce_v in $(compgen -v GIT_ || true); do
                case "$__ce_v" in
                    GIT_CONFIG_NOSYSTEM|GIT_CONFIG_GLOBAL) ;;
                    *) __ce_bad+=("$__ce_v") ;;
                esac
            done
            ;;
        sort)
            export LC_ALL=C 2>/dev/null || true
            [ "${LC_ALL-}" = C ] || __ce_bad+=(LC_ALL)
            ;;
    esac
    _require_env_refuse "$__ce_caller" "$__ce_desc" "$3" "${__ce_bad[@]}"
}

# _require_env_refuse <caller> <description> <command> [<var>...]: exit 2
# naming each <var> — the variables that survived the contract rewrite.
_require_env_refuse() {
    [ "$#" -gt 3 ] || return 0
    local __er_caller="$1" __er_desc="$2" __er_cmd="$3"; shift 3
    echo "$__er_caller: $__er_desc: could not scrub $* — refusing to run $__er_cmd under a variable that rewrites its result" >&2
    exit 2
}

# match_or_fail <description> -- <command...>: run an rg-shaped command
# (stdout/stderr pass through unredirected, so matches stay visible), classify
# its exit code with rg_status, and hard-exit 2 on anything but 0/1. Returns 0
# when the command matched, 1 when it cleanly found nothing — mirrors rg's own
# convention so callers can write `if match_or_fail … -- rg …; then`.
# The command must be an allowlisted matcher (`rg`, `grep`). To match a producer's output,
# capture the producer first (capture_nul or a checked `$(…)`), then feed the
# matcher a herestring.
match_or_fail() {
    local __mf_desc="$1"; shift
    [ "${1:-}" = "--" ] && shift
    _require_known_command match_or_fail "$__mf_desc" matcher "$@"
    local __mf_rc=0
    (_require_contract_env match_or_fail "$__mf_desc" "${1##*/}"; exec "$@") || __mf_rc=$?
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
# -> hard-exit 2. The command must be an allowlisted matcher (`rg`, `grep`) —
# never a pipeline, nor a function wrapping one: capture the producer first
# (checked), then match on a herestring.
match_capture() {
    _require_out_name match_capture "${1:-}" scalar
    local __mc_var="$1" __mc_desc="$2"; shift 2
    [ "${1:-}" = "--" ] && shift
    _require_known_command match_capture "$__mc_desc" matcher "$@"
    local __mc_out __mc_rc=0
    __mc_out="$(_require_contract_env match_capture "$__mc_desc" "${1##*/}"; exec "$@")" || __mc_rc=$?
    case "$(rg_status "$__mc_rc")" in
        match)    _require_set_scalar "$__mc_var" "$__mc_out"; return 0 ;;
        no-match) _require_set_scalar "$__mc_var" "$__mc_out"; return 1 ;;
        error)
            echo "match_capture: $__mc_desc: command exited $__mc_rc (neither match nor no-match) — treating as a hard failure" >&2
            exit 2
            ;;
    esac
}

# git_toplevel_into <var>: write the working tree's top directory into
# <var>, asked of git under the same scrubbed environment as every other git
# producer — a caller's `GIT_DIR`/`GIT_WORK_TREE` never picks the tree. Hard-
# exits 2 when git fails or the answer is not one absolute existing directory.
git_toplevel_into() {
    _require_out_name git_toplevel_into "${1:-}" scalar
    local -a __gt_rec=()
    _capture_nul_rc __gt_rec "repository top-level" -- git rev-parse --show-toplevel || exit 2
    local __gt_top="${__gt_rec[0]-}"
    __gt_top="${__gt_top%$'\n'}"
    if [ "${#__gt_rec[@]}" -ne 1 ] || [ "${__gt_top:0:1}" != / ] \
        || [[ "$__gt_top" == *$'\n'* ]] || [ ! -d "$__gt_top" ]; then
        echo "git_toplevel_into: git reported no single absolute top-level directory" >&2
        exit 2
    fi
    _require_set_scalar "$1" "$__gt_top"
}

# capture_nul <array-var> <description> -- <command...>: run a producer that
# prints NUL-delimited records (`git ls-files -z`, `find … -print0`) and load
# them into <array-var>. Any non-zero producer exit hard-exits 2 — a producer
# that died part-way yields a partial set, and a partial set must never stand
# in for the whole one. The command must be an allowlisted producer (`git`,
# `find`, `sort`).
capture_nul() {
    _require_out_name capture_nul "${1:-}" array
    _capture_nul_rc "$@" || exit 2
}

# _capture_nul_rc <var> <description> -- <command...>: capture_nul without the
# output-name check, for the lib's own targets; returns 2 (message printed)
# instead of exiting, so a caller can release its own temp files first. The
# producer is the ONLY writer of the staged file and runs as one exec'd
# process (no pipe, no process substitution), so its own exit code is the
# whole verdict on the bytes loaded.
_capture_nul_rc() {
    local __cn_var="$1" __cn_desc="$2"; shift 2
    [ "${1:-}" = "--" ] && shift
    _require_known_command capture_nul "$__cn_desc" producer "$@"
    local __cn_tmp __cn_rc=0
    __cn_tmp="$(mktemp)" || { echo "capture_nul: $__cn_desc: mktemp failed" >&2; return 2; }
    (
        _require_contract_env capture_nul "$__cn_desc" "${1##*/}"
        case "${1##*/}" in
            git) set -- "$1" -c core.fsmonitor=false "${@:2}" ;;
        esac
        exec "$@"
    ) >"$__cn_tmp" || __cn_rc=$?
    if [ "$__cn_rc" -ne 0 ]; then
        rm -f "$__cn_tmp"
        echo "capture_nul: $__cn_desc: producer exited $__cn_rc — refusing a possibly partial set" >&2
        return 2
    fi
    mapfile -d '' -t "$__cn_var" <"$__cn_tmp" || {
        rm -f "$__cn_tmp"
        echo "capture_nul: $__cn_desc: could not load the set into '$__cn_var'" >&2
        return 2
    }
    rm -f "$__cn_tmp"
}

# enumerate_files <array-var> <glob> <root...>: load every regular file named
# <glob> under the roots into <array-var>, byte-order sorted and NUL-safe.
# Hard-exits 2 when no root is given, a root is not a directory, find or sort
# exits non-zero (an unreadable subtree), the set is empty, or the sorted set
# is not exactly a permutation of the found one — a scan over a missing,
# partial, forged, or empty file set must never read as a clean scan.
enumerate_files() {
    _require_out_name enumerate_files "${1:-}" array
    _enumerate_files_into "$@"
}

# _enumerate_files_into: enumerate_files without the output-name check, for
# the lib's own `__`-prefixed targets. Every stage is a checked write into a
# lib-owned file or array — the found set goes to a staged file through the
# builtin printf, sort reads that file itself (its exit code covers the read)
# and is captured straight into the target — and the result is then proven a
# permutation of the found set: equal record count, and every sorted record
# consumes one occurrence of itself from the found multiset. A truncated,
# dropped, or substituted record therefore fails the proof whatever exit code
# sort reported.
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
    # A root find would read as the start of its expression (`-delete`, `!`,
    # `(`, `)`, `,`) is pinned to a path first, so it is only ever a root.
    local -a __ef_roots=()
    for __ef_root in "$@"; do
        case "$__ef_root" in
            -* | '!'* | '('* | ')'* | ,*) __ef_roots+=("./$__ef_root") ;;
            *) __ef_roots+=("$__ef_root") ;;
        esac
    done
    set -- "${__ef_roots[@]}"
    local -a __ef_found=()
    _capture_nul_rc __ef_found "find '$__ef_glob' under $*" -- \
        find "$@" -type f -name "$__ef_glob" -print0 || exit 2
    if [ "${#__ef_found[@]}" -eq 0 ]; then
        echo "enumerate_files: no file matching '$__ef_glob' under $* — nothing to scan" >&2
        exit 2
    fi
    # Refuse an off-contract `sort` before the staged file exists, so no
    # refusal path leaves it behind.
    _require_known_command enumerate_files "sort '$__ef_glob' set" producer sort -z
    local __ef_tmp
    __ef_tmp="$(mktemp)" || { echo "enumerate_files: mktemp failed" >&2; exit 2; }
    if ! printf '%s\0' "${__ef_found[@]}" >"$__ef_tmp"; then
        rm -f "$__ef_tmp"
        echo "enumerate_files: could not stage the '$__ef_glob' set for sort" >&2
        exit 2
    fi
    _capture_nul_rc "$__ef_var" "sort '$__ef_glob' set" -- sort -z -- "$__ef_tmp" \
        || { rm -f "$__ef_tmp"; exit 2; }
    rm -f "$__ef_tmp"
    local -n __ef_out="$__ef_var"
    if [ "${#__ef_out[@]}" -ne "${#__ef_found[@]}" ]; then
        echo "enumerate_files: sort returned ${#__ef_out[@]} record(s) for '$__ef_glob' under $* but ${#__ef_found[@]} were found — refusing a possibly truncated set" >&2
        exit 2
    fi
    # Keys carry a `k` prefix so no record ("", "@", "*") can collide with an
    # associative subscript bash treats specially.
    local -A __ef_left=()
    local __ef_f __ef_n
    for __ef_f in "${__ef_found[@]}"; do
        __ef_n="${__ef_left["k$__ef_f"]:-0}"
        __ef_left["k$__ef_f"]=$((__ef_n + 1))
    done
    for __ef_f in "${__ef_out[@]}"; do
        __ef_n="${__ef_left["k$__ef_f"]:-0}"
        if [ "$__ef_n" -le 0 ]; then
            echo "enumerate_files: sort returned a record that is not one of the found '$__ef_glob' files under $* — refusing a truncated or forged set" >&2
            exit 2
        fi
        __ef_left["k$__ef_f"]=$((__ef_n - 1))
    done
}


# require_scan_root <dir> <glob>: exit 2 when <dir> does not exist, find
# cannot walk it, or it has no file matching <glob> anywhere under it — a
# missing root or an emptied fixture directory must never let a scan tool's
# own "no match" pass the gate vacuously.
require_scan_root() {
    local -a __rsr_files=()
    _enumerate_files_into __rsr_files "$2" "$1"
}
