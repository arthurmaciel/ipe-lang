#!/usr/bin/env bash
# Doctest-style gate for Ipê explain pages (ADR 0059).
#
# Scans every fenced Ipê code block in:
#   src/compiler/diagnostics/explain/*.md
#
# and compiles each one with `ipe type-check`, failing if any block that is
# expected to compile does not — ensuring a reader can copy any example and
# it type-checks.
#
# FENCE MARKERS  (the info-string after ```ipe)
# ─────────────────────────────────────────────
#   ```ipe              default — block MUST type-check (gate fails if it doesn't)
#   ```ipe ipe:error    block is a "bad example"; gate fails if it COMPILES
#   ```ipe ipe:skip     genuinely illustrative pseudocode / FFI / multi-file —
#                       skipped entirely (no compile attempt)
#
# WRAPPING
# ────────
# Blocks that do not start with `module` are wrapped in:
#   module Main exposing (..)
#   <auto-injected imports based on qualified names used>
#   <block body>
#
# Auto-injected imports (keyed on qualified prefix used in the block):
#   Io.*     → import Ipe.Io as Io
#   Task.*   → import Ipe.Task as Task
#   Maybe.*  → import Ipe.Maybe as Maybe
#   String.* → import Ipe.String as String
#   List.*   → import Ipe.List as List
#   Dict.*   → import Ipe.Dict as Dict
#   Debug.*  → import Ipe.Debug as Debug
#   Set.*    → import Ipe.Set as Set
#
# EXIT
#   0 — all expected-compile blocks compile; all ipe:error blocks fail
#   1 — at least one block failed its expectation
#   2 — setup error (missing ipe binary, missing explain dir)
#
set -euo pipefail

source "$(dirname "$0")/lib/env.sh"

EXPLAIN_DIR="$REPO/src/compiler/diagnostics/explain"

if [ ! -d "$EXPLAIN_DIR" ]; then
    echo "ERROR: explain dir not found: $EXPLAIN_DIR" >&2
    exit 2
fi

if [ ! -x "$IPE_BIN" ]; then
    echo "ERROR: ipe binary not found at '$IPE_BIN' — build with: cargo build --release -p ipe" >&2
    exit 2
fi

echo "=== Ipê explain-page example gate ==="
echo "    explain dir: $EXPLAIN_DIR"
echo "    ipe binary:  $IPE_BIN"
echo

# Temporary directory for generated snippet files; cleaned on exit.
TMPWORK="$(mktemp -d)"
trap 'rm -rf "$TMPWORK"' EXIT

total=0
ok=0
error_as_expected=0
skipped=0
failed=()

# process_file PAGE_PATH — extract every ```ipe block in the file.
# Outputs one record per block: MARKER<TAB>CODE (newlines in CODE escaped to \n).
process_file() {
    local file="$1"
    python3 - "$file" << 'PYEOF'
import sys, re
content = open(sys.argv[1]).read()
for m in re.finditer(r'```ipe(?P<marker>[^\n]*)\n(?P<code>.*?)```', content, re.DOTALL):
    marker = m.group('marker').strip()
    code   = m.group('code').rstrip('\n')
    if not code.strip():
        continue
    # Encode: TAB separates marker from code; newlines in code → literal \n
    print(marker + '\t' + code.replace('\\', '\\\\').replace('\n', '\\n'))
PYEOF
}

# infer_imports CODE — emit import lines for qualified names used in CODE.
infer_imports() {
    local code="$1"
    [[ "$code" == *"Io."*     ]] && echo "import Ipe.Io as Io"
    [[ "$code" == *"Task."*   ]] && echo "import Ipe.Task as Task"
    [[ "$code" == *"Maybe."*  ]] && echo "import Ipe.Maybe as Maybe"
    [[ "$code" == *"String."* ]] && echo "import Ipe.String as String"
    [[ "$code" == *"List."*   ]] && echo "import Ipe.List as List"
    [[ "$code" == *"Dict."*   ]] && echo "import Ipe.Dict as Dict"
    [[ "$code" == *"Debug."*  ]] && echo "import Ipe.Debug as Debug"
    [[ "$code" == *"Set."*    ]] && echo "import Ipe.Set as Set"
    return 0
}

for md_file in "$EXPLAIN_DIR"/*.md; do
    page="$(basename "$md_file" .md)"
    block_idx=0

    while IFS=$'\t' read -r marker encoded_code; do
        # Decode: restore literal \n in code.
        code="${encoded_code//\\n/$'\n'}"
        code="${code//\\\\/\\}"
        block_idx=$((block_idx + 1))
        total=$((total + 1))

        # ── ipe:skip ─────────────────────────────────────────────────
        if [ "$marker" = "ipe:skip" ]; then
            skipped=$((skipped + 1))
            printf '  skip   %s #%d  (ipe:skip)\n' "$page" "$block_idx"
            continue
        fi

        # ── Build the snippet file ────────────────────────────────────
        snippet_file="$TMPWORK/Main.ipe"
        if printf '%s' "$code" | head -1 | grep -q '^module '; then
            # Already has a module header — use as-is.
            printf '%s\n' "$code" > "$snippet_file"
        else
            # Wrap in a module header + auto-injected imports.
            {
                printf 'module Main exposing (..)\n'
                imports="$(infer_imports "$code")"
                if [ -n "$imports" ]; then
                    printf '\n%s\n' "$imports"
                fi
                printf '\n%s\n' "$code"
            } > "$snippet_file"
        fi

        # ── Run ipe type-check ────────────────────────────────────────
        check_out="$TMPWORK/check.out"
        if timeout 15 "$IPE_BIN" type-check "$snippet_file" > "$check_out" 2>&1; then
            compiled=1
        else
            compiled=0
        fi

        # ── Evaluate expectation ──────────────────────────────────────
        if [ "$marker" = "ipe:error" ]; then
            # Expected to fail.
            if [ "$compiled" -eq 1 ]; then
                printf '  FAIL   %s #%d  (ipe:error block compiled — it should produce an error)\n' \
                    "$page" "$block_idx"
                failed+=("$page block $block_idx: ipe:error block unexpectedly compiled")
            else
                error_as_expected=$((error_as_expected + 1))
                printf '  ok✗    %s #%d  (ipe:error — fails as expected)\n' "$page" "$block_idx"
            fi
        else
            # Expected to compile (default marker, including "").
            if [ "$compiled" -eq 1 ]; then
                ok=$((ok + 1))
                printf '  ok     %s #%d\n' "$page" "$block_idx"
            else
                printf '  FAIL   %s #%d\n' "$page" "$block_idx"
                sed 's/^/         /' "$check_out"
                failed+=("$page block $block_idx: unmarked block does not type-check")
            fi
        fi

    done < <(process_file "$md_file")
done

echo
echo "=== Results ==="
printf "  total blocks  : %d\n" "$total"
printf "  compile (ok)  : %d\n" "$ok"
printf "  error as exp. : %d\n" "$error_as_expected"
printf "  skipped       : %d\n" "$skipped"
printf "  FAILURES      : %d\n" "${#failed[@]}"

if [ "${#failed[@]}" -gt 0 ]; then
    echo
    echo "=== FAILED blocks ==="
    for msg in "${failed[@]}"; do
        printf "  FAIL: %s\n" "$msg"
    done
    echo
    echo "Fix each FAIL by either:"
    echo "  • Correcting the example so it type-checks (preferred)."
    echo "  • Marking it \`\`\`ipe ipe:error  — intentional bad example."
    echo "  • Marking it \`\`\`ipe ipe:skip   — genuine pseudocode that cannot be"
    echo "    made standalone (requires FFI, multi-file context, etc)."
    echo
    echo "A stale or wrong teaching example is worse than no example (ADR 0059)."
    exit 1
fi

echo
echo "=== VERDICT: PASS — all $((ok + error_as_expected)) checked blocks meet their expectation ==="
exit 0
