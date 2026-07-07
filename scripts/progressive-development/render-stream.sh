#!/usr/bin/env bash
# render-stream.sh - turn `claude --output-format stream-json` (on stdin) into
# compact, human-readable lines. SINGLE source of rendering, shared by
# autopilot.sh (show_agent heartbeat) AND watch.sh (live follow). Arg 1 = tag.
# tmux-safe colors when stdout is a tty; unhandled/raw json lines drop to nothing.
#
# Un-clipped (was: squash-to-one-line + 100-char cut). Assistant text and tool
# commands render IN FULL; multi-line content keeps its lines, each continuation
# indented under the content column so it reads as one block, not noise. Tool
# RESULTS are the one exception — capped to $RS_RESULT_LINES lines (default 12)
# with a "+N lines" note, so a file dump / big grep can't flood the view.
set -uo pipefail
tag="${1:-}"
RS_RESULT_LINES="${RS_RESULT_LINES:-12}"
if [ -t 1 ]; then
    C0=$'\033[0m'; TAGC=$'\033[36m'; TOOLC=$'\033[33m'; RESC=$'\033[32m'; DIMC=$'\033[90m'
else
    C0=; TAGC=; TOOLC=; RESC=; DIMC=
fi
pfx=""; [ -n "$tag" ] && pfx="${TAGC}[${tag}]${C0} "
IND="      "   # continuation indent — clears "  [tag] " so wrapped lines align
if command -v jq >/dev/null 2>&1; then
    jq -R -j --unbuffered \
        --arg pfx "$pfx" --arg c0 "$C0" --arg toolc "$TOOLC" --arg resc "$RESC" \
        --arg dimc "$DIMC" --arg ind "$IND" --argjson rlines "$RS_RESULT_LINES" '
      def indent: gsub("\n"; "\n" + $ind);
      def cap(n): (split("\n")
                    | if length > n
                      then (.[0:n] | join("\n" + $ind)) + "\n" + $ind
                           + "… +" + ((length - n)|tostring) + " lines"
                      else join("\n" + $ind) end);
      ( fromjson? | objects
        | if .type=="assistant" then ( .message.content[]? |
              if   .type=="text" and (.text|length>0) then (.text | indent)
              elif .type=="tool_use" then $toolc + "> " + .name + $c0 + " "
                    + ((.input.command // .input.file_path // .input.path
                        // .input.pattern // .input.description
                        // (.input|tostring)) | tostring | indent)
              else empty end )
          elif .type=="user" then ( .message.content[]? |
              if .type=="tool_result" then $dimc
                    + ((.content | if type=="string" then .
                        else (map(.text? // "")|join("\n")) end) | cap($rlines))
                    + $c0
              else empty end )
          elif .type=="result" then $resc + "* "
                + ((.result // .subtype // "done")|tostring | indent) + $c0
          else empty end )
      | "  " + $pfx + . + "\n" '
else
    sed "s/^/  ${pfx}/"
fi
