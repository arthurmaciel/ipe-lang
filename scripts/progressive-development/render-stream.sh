#!/usr/bin/env bash
# render-stream.sh - turn `claude --output-format stream-json` (on stdin) into
# compact, human-readable lines. SINGLE source of rendering, shared by
# autopilot.sh (show_agent heartbeat) AND watch.sh (live follow). Arg 1 = tag.
# tmux-safe colors when stdout is a tty.
#
# Per-line policy (this is the fix for watch.sh's "degrades to raw json"):
#   · a line that PARSES as a json OBJECT → render it if it's an
#     assistant / user-tool_result / result event; DROP it otherwise
#     (system / thinking_tokens / stream deltas are noise).
#   · a line that does NOT parse as json → ECHO it verbatim (dimmed). This is
#     what makes render-stream safe for BOTH json logs AND plain logs (cargo
#     output, shell errors), so watch.sh can pipe EVERY log through it without
#     a fragile first-byte guess that races an empty freshly-created file.
#
# Un-clipped: assistant text + tool commands render in full; multi-line content
# keeps its lines, continuation-indented so it reads as one block. Tool RESULTS
# cap at $RS_RESULT_LINES lines (default 12) + "+N lines" so a dump can't flood.
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
      # render a parsed json OBJECT → zero-or-more display strings (empty = drop)
      def render:
          if .type=="assistant" then ( .message.content[]? |
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
          else empty end;      # system / thinking_tokens / deltas → drop
      . as $raw
      | ( [ $raw | fromjson? ] ) as $p          # [] if not json, [value] if json
      | if ($p|length)==0
        then ("  " + $pfx + $dimc + $raw + $c0 + "\n")           # non-json → echo verbatim
        else ( $p[0] | if type=="object" then (render | "  " + $pfx + . + "\n") else empty end )
        end '
else
    sed "s/^/  ${pfx}/"
fi
