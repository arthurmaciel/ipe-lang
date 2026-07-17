#!/usr/bin/env bash
# PreToolUse(Bash) guard — hard-block two dev-ops bans agents keep ignoring when
# they live only in prose briefs:
#   1. `cargo fmt` WRITES (reformat the ENTIRE workspace)
#   2. `pgrep` (self-matches its own cmdline / the invoking shell → false positives
#      every caller must reason around; in a loop it also leaves zombie subprocesses)
# Matches INVOCATIONS (command position) only, so mentioning them as an argument
# (rg pgrep, echo, a heredoc) is fine. Escape hatch for the one sanctioned
# workspace fmt pass (#214): prefix the command with `IPE_ALLOW_FMT=1`.
cmd="$(jq -r '.tool_input.command // empty' 2>/dev/null)"
[ -z "$cmd" ] && exit 0
deny() { jq -cn --arg r "$1" \
  '{hookSpecificOutput:{hookEventName:"PreToolUse",permissionDecision:"deny",permissionDecisionReason:$r}}'
  exit 0; }

# command-position prefix: start | ; | & | | | ( | && | ||, then optional VAR=…
# assignments and/or `env VAR=…`, then the program name.
CP='(^|[;&|(]|&&|[|][|])[[:space:]]*([A-Za-z_][A-Za-z0-9_]*=[^[:space:]]*[[:space:]]+)*(env[[:space:]]+([A-Za-z_][A-Za-z0-9_]*=[^[:space:]]*[[:space:]]+)*)?'

# 1) cargo fmt that WRITES (not --check), unless the IPE_ALLOW_FMT escape hatch.
if printf '%s' "$cmd" | grep -Eq "${CP}cargo([[:space:]]+\+[^[:space:]]+)?[[:space:]]+fmt" \
   && ! printf '%s' "$cmd" | grep -Eq -- '--check' \
   && ! printf '%s' "$cmd" | grep -q 'IPE_ALLOW_FMT'; then
  deny 'BLOCKED: cargo fmt reformats the WHOLE workspace (DEVELOPMENT.md). Use `rustfmt <exact file>`, or `cargo fmt --check` (read-only). For the one sanctioned workspace pass (#214) prefix `IPE_ALLOW_FMT=1`.'
fi

# 2) pgrep invocation (any form — even one-shot `pgrep -f` self-matches).
if printf '%s' "$cmd" | grep -Eq "${CP}pgrep\b"; then
  deny 'BLOCKED: pgrep self-matches (its own cmdline / the invoking shell) -> false positives. Use the bracket trick that cannot self-match: `ps -eo pid,args | grep "[c]argo"` (first char in [] so the grep process itself does not match), or `ps -eo args | grep -c "[c]argo"` for a live count, or a pidfile. Never poll in a loop — use the Monitor tool or a foreground `timeout` command.'
fi

exit 0
