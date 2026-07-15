#!/usr/bin/env bash
# PreToolUse(Bash) guard — hard-block the two dev-ops bans that agents keep
# ignoring when they live only in prose briefs:
#   1. workspace-wide `cargo fmt` (reformats the ENTIRE workspace)
#   2. self-poll `pgrep` wait-loops (leave zombie subprocesses that re-notify)
# Reads the tool-call JSON on stdin; emits a PreToolUse deny decision on a hit.
# Escape hatch for the ONE sanctioned workspace fmt pass (backlog #214):
# prefix the command with `SKY_ALLOW_FMT=1`.
cmd="$(jq -r '.tool_input.command // empty' 2>/dev/null)"
[ -z "$cmd" ] && exit 0

deny() { jq -cn --arg r "$1" \
  '{hookSpecificOutput:{hookEventName:"PreToolUse",permissionDecision:"deny",permissionDecisionReason:$r}}'
  exit 0; }

# 1) `cargo fmt` that WRITES (not --check), unless explicitly allowed.
if printf '%s' "$cmd" | grep -Eq 'cargo([[:space:]]+\+[^[:space:]]+)?[[:space:]]+fmt' \
   && ! printf '%s' "$cmd" | grep -Eq -- '--check' \
   && ! printf '%s' "$cmd" | grep -q 'SKY_ALLOW_FMT'; then
  deny 'BLOCKED: cargo fmt reformats the WHOLE workspace (DEVELOPMENT.md). Use `rustfmt <exact file>`, or `cargo fmt --check` (read-only). For the one sanctioned workspace pass (#214) prefix `SKY_ALLOW_FMT=1`.'
fi

# 2) self-poll `pgrep` wait-loop (the zombie-subprocess pattern: `while pgrep …; do sleep`).
if printf '%s' "$cmd" | grep -Eq '(while|until)[^;]*pgrep'; then
  deny 'BLOCKED: self-poll pgrep wait-loop (leaves zombie subprocesses that re-notify). Use the Monitor tool, or run the build in the foreground under `timeout` and trust its exit code.'
fi

exit 0
