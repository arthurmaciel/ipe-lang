#!/usr/bin/env bash
# Doc-test gate for Ipê doc-string examples (component F).
#
# Extracts every fenced ` ```ipe ` block from every `{-| … -}` doc-string in
# the standard library and type-checks each one via `ipe doc --check-examples`.
#
# FENCE MARKERS
# ─────────────
# Blocks are extracted by the compiler itself (the `DocString` AST node from the
# parse component). A block with `-->` annotations on expression lines is also
# used for result-assertion in the E2E tier (when IPE_E2E=1).
#
# EXIT
#   0 — all example blocks type-check
#   1 — at least one block failed
#   2 — setup error (missing ipe binary)
#
set -euo pipefail

source "$(dirname "$0")/lib/env.sh"

if [ ! -x "$IPE_BIN" ]; then
    echo "ERROR: ipe binary not found at '$IPE_BIN'" >&2
    echo "       build with: cargo build --release -p ipe" >&2
    exit 2
fi

echo "=== Ipê doc-string example gate ==="
echo "    ipe binary: $IPE_BIN"
echo

"$IPE_BIN" doc --check-examples
