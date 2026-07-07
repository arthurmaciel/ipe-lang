#!/usr/bin/env bash
# render-stream.sh - turn `claude --output-format stream-json` (on stdin) into
# compact, human-readable lines. Shared by autopilot.sh (inline triage/audit
# heartbeat) and available to watch.sh. Assumes stream-json input (the caller
# only pipes here when PROGDEV_STREAM != 0). Arg 1 = optional tag. tmux-safe
# colors when stdout is a tty; unhandled/raw json lines drop to nothing.
set -uo pipefail
tag="${1:-}"
if [ -t 1 ]; then
    C0=$'\033[0m'; TAGC=$'\033[36m'; TOOLC=$'\033[33m'; RESC=$'\033[32m'; DIMC=$'\033[90m'
else
    C0=; TAGC=; TOOLC=; RESC=; DIMC=
fi
pfx=""; [ -n "$tag" ] && pfx="${TAGC}[${tag}]${C0} "
if command -v jq >/dev/null 2>&1; then
    jq -R -j --unbuffered --arg pfx "$pfx" --arg c0 "$C0" \
        --arg toolc "$TOOLC" --arg resc "$RESC" --arg dimc "$DIMC" '
      ( fromjson? | objects
        | if .type=="assistant" then ( .message.content[]? |
              if   .type=="text" and (.text|length>0) then ((.text|gsub("\n";" "))[0:400])
              elif .type=="tool_use" then $toolc + "> " + .name + $c0 + " " + (((.input.command // .input.file_path // .input.path // .input.pattern // .input.description // (.input|tostring))|tostring|gsub("\n";" "))[0:120])
              else empty end )
          elif .type=="user" then ( .message.content[]? |
              if .type=="tool_result" then $dimc + "  " + (((.content | if type=="string" then . else (map(.text? // "")|join(" ")) end)|gsub("\n";" "))[0:100]) + $c0
              else empty end )
          elif .type=="result" then $resc + "* " + ((.result // .subtype // "done")|tostring|gsub("\n";" "))[0:400] + $c0
          else empty end )
      | "  " + $pfx + . + "\n" '
else
    sed "s/^/  ${pfx}/"
fi
