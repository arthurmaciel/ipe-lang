#!/usr/bin/env bash
#
# issue-ticket.sh — the project's backlog interface, backed by GitHub issues.
#
# Every backlog item is a self-contained GitHub issue: one ticket per command or
# future task, with enough context to be picked up cold. This wraps `gh issue`
# so filing, listing, and closing are one-liners, and supports --dry-run
# everywhere so a batch can be rehearsed before anything is created.
#
# Usage:
#   issue-ticket.sh add   <title> [--body <text> | --body-file <path>]
#                               [--label <name>]... [--milestone <m>]
#                               [--assignee <user>] [--dry-run]
#   issue-ticket.sh list  [--label <name>] [--state open|closed|all] [--dry-run]
#   issue-ticket.sh close <number> [--comment <text>] [--dry-run]
#   issue-ticket.sh ensure-label <name> [color-hex] [description]
#
# Env:
#   IPE_ISSUE_REPO   target repo (default: arthurmaciel/ipe-lang)
#
# A label named on `add` that does not yet exist is created automatically
# (unless --dry-run). Requires the `gh` CLI, authenticated.

set -euo pipefail

REPO="${IPE_ISSUE_REPO:-arthurmaciel/ipe-lang}"

die() { printf 'issue-ticket: %s\n' "$1" >&2; exit 1; }

require_gh() { command -v gh >/dev/null 2>&1 || die "the GitHub CLI \`gh\` is not installed or not on PATH"; }

# Print a command for --dry-run without executing it, quoting each argument so
# the printed line is copy-pasteable and unambiguous.
show() {
  printf '[dry-run] would run:'
  local a
  for a in "$@"; do printf ' %q' "$a"; done
  printf '\n'
}

usage() {
  # Print the leading comment block (lines 3..) as help, stopping at the first
  # non-comment line so the code below is never echoed.
  awk 'NR>2 { if (/^#/) { sub(/^# ?/, ""); print } else { exit } }' "$0"
  exit "${1:-0}"
}

# ensure-label <name> [color] [description]
# Create the label if the repo does not already have it. Idempotent.
cmd_ensure_label() {
  local name="${1:-}" color="${2:-ededed}" desc="${3:-backlog item}"
  [ -n "$name" ] || die "ensure-label needs a <name>"
  require_gh
  if gh label list --repo "$REPO" --json name --jq '.[].name' | grep -Fxq "$name"; then
    return 0
  fi
  gh label create "$name" --repo "$REPO" --color "$color" --description "$desc" >/dev/null 2>&1 \
    || true # a race or pre-existing label is fine
}

cmd_add() {
  local title="" body="" body_file="" milestone="" assignee="" dry=0
  local -a labels=()
  while [ $# -gt 0 ]; do
    case "$1" in
      --body)      body="${2:?--body needs a value}"; shift 2;;
      --body-file) body_file="${2:?--body-file needs a path}"; shift 2;;
      --label)     labels+=("${2:?--label needs a value}"); shift 2;;
      --milestone) milestone="${2:?--milestone needs a value}"; shift 2;;
      --assignee)  assignee="${2:?--assignee needs a value}"; shift 2;;
      --dry-run)   dry=1; shift;;
      -h|--help)   usage 0;;
      --*)         die "unknown flag for add: $1";;
      *)           if [ -z "$title" ]; then title="$1"; else die "unexpected argument: $1"; fi; shift;;
    esac
  done
  [ -n "$title" ] || die "add needs a <title>"
  if [ -n "$body_file" ]; then
    [ -f "$body_file" ] || die "body file not found: $body_file"
    body="$(cat "$body_file")"
  fi
  [ -n "$body" ] || body="_(no description — a self-contained ticket should say what, why, and done-when)_"

  local -a cmd=(gh issue create --repo "$REPO" --title "$title" --body "$body")
  local l
  for l in "${labels[@]:-}"; do [ -n "$l" ] && cmd+=(--label "$l"); done
  [ -n "$milestone" ] && cmd+=(--milestone "$milestone")
  [ -n "$assignee" ]  && cmd+=(--assignee "$assignee")

  if [ "$dry" -eq 1 ]; then show "${cmd[@]}"; return 0; fi
  require_gh
  for l in "${labels[@]:-}"; do [ -n "$l" ] && cmd_ensure_label "$l"; done
  "${cmd[@]}"
}

cmd_list() {
  local label="" state="open" dry=0
  while [ $# -gt 0 ]; do
    case "$1" in
      --label) label="${2:?--label needs a value}"; shift 2;;
      --state) state="${2:?--state needs a value}"; shift 2;;
      --dry-run) dry=1; shift;;
      -h|--help) usage 0;;
      *) die "unknown argument for list: $1";;
    esac
  done
  local -a cmd=(gh issue list --repo "$REPO" --state "$state" --limit 200)
  [ -n "$label" ] && cmd+=(--label "$label")
  if [ "$dry" -eq 1 ]; then show "${cmd[@]}"; return 0; fi
  require_gh
  "${cmd[@]}"
}

cmd_close() {
  local number="" comment="" dry=0
  while [ $# -gt 0 ]; do
    case "$1" in
      --comment) comment="${2:?--comment needs a value}"; shift 2;;
      --dry-run) dry=1; shift;;
      -h|--help) usage 0;;
      --*) die "unknown flag for close: $1";;
      *) if [ -z "$number" ]; then number="$1"; else die "unexpected argument: $1"; fi; shift;;
    esac
  done
  [ -n "$number" ] || die "close needs an issue <number>"
  local -a cmd=(gh issue close "$number" --repo "$REPO")
  [ -n "$comment" ] && cmd+=(--comment "$comment")
  if [ "$dry" -eq 1 ]; then show "${cmd[@]}"; return 0; fi
  require_gh
  "${cmd[@]}"
}

main() {
  [ $# -gt 0 ] || usage 1
  local sub="$1"; shift
  case "$sub" in
    add)          cmd_add "$@";;
    list)         cmd_list "$@";;
    close)        cmd_close "$@";;
    ensure-label) cmd_ensure_label "$@";;
    -h|--help)    usage 0;;
    *)            die "unknown subcommand: $sub (try add|list|close|ensure-label)";;
  esac
}

main "$@"
