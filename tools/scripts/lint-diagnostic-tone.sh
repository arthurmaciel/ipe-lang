#!/usr/bin/env bash
# Diagnostic tone/jargon gate: no compiler-internal vocabulary in user-facing
# text. This gate mechanises the "no internal jargon" rule for the two
# user-facing text sets:
#
#   render_goldens/*.txt   the byte-locked rendered diagnostic output
#   explain/*.md           the long-form `ipe explain <CODE>` pages
#
# Rust source (.rs) is NOT scanned — internal names belong in the implementation.
#
# Exit 0 when clean; non-zero listing each violation as `file:line:term`.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# Overridable so tools/scripts/tests/test-require-tool.sh can point the gate
# at a fixture dir and prove it actually fails on a known violation.
goldens_dir="${GOLDENS_DIR:-$repo_root/src/compiler/diagnostics/tests/render_goldens}"
explain_dir="${EXPLAIN_DIR:-$repo_root/src/compiler/diagnostics/explain}"

source "$repo_root/tools/scripts/lib/require-tool.sh"
require_tool rg

# Internal jargon banned in ANY user-facing text (goldens + explain).
# Word-boundary, case-insensitive — these tokens are distinctive enough that no
# ordinary English word collides with them, so case-folding is safe and also
# catches inflected forms (`canonicalise` -> "Canonicalisation").
jargon='salsa|zonk|zonking|VarId|IrType|canonicaliser|canonicalise|unification|HM inference|lowerer'

# `Symbol` and `Ty` are the internal type names, gated CASE-SENSITIVELY as
# standalone whole words. The lowercase English words "symbol" (a keyboard
# symbol, an interned symbol name) and "ty" are legitimate user-facing prose, so
# a case-insensitive match here would be a false positive. `-w` bounds each so
# neither fires inside `Symbols`, `Type`, `Typed`, or an `.ipe` identifier.
jargon_cased='Symbol|Ty'

# Raw Rust types banned in GOLDENS ONLY. Explain pages legitimately reference
# Rust types to motivate why a restriction exists (diagnostic-tone.md), so they
# are exempt here — the tone guide's plain-language rule covers them by style,
# not by this gate. These are NOT scanned with `-w`: the generic openers end in
# `<`, whose trailing non-word boundary would defeat a word-boundary match; the
# bare primitives carry their own explicit `\b` anchors instead.
rust_types='Vec<|Box<|Pin<|\bu32\b|\bu64\b|\busize\b'

# Soft/style-guide only, NOT gated here (documented for the reader):
#   `illegal` / `invalid` / `forbidden` — legitimate contrastive uses appear in
#       goldens and explain prose; the no-blame rule is a style review item.
#   raw Rust types inside explain/*.md — permitted to motivate restrictions.

violations=0

# scan DIR PATTERN GLOB CASE WORD — print each match as file:line:term and set
# the violations flag on any hit. CASE is "-i" (case-insensitive) or ""
# (sensitive); WORD is "-w" (whole-word) or "" (pattern carries its own anchors).
# Uses match_or_fail so an rg error (a bad pattern, a permission error, or rg
# itself missing) hard-fails the gate instead of reading as "no match".
# require_scan_root hard-fails when DIR is missing or has no file matching
# GLOB — a missing root or an emptied fixture dir must never read as "clean"
# just because rg then finds nothing to match against.
scan() {
    local dir="$1" pattern="$2" glob="$3" case_flag="${4:-}" word_flag="${5:-}"
    require_scan_root "$dir" "$glob"
    # rg: -o print only the matched term, --no-heading + -n for file:line, -H to
    # always print the filename. The roots are explicit, so --no-ignore
    # --hidden keeps a .gitignore/.ignore rule or a dotfile from silently
    # shrinking the scanned set; -a keeps a NUL byte from ending a file's scan
    # before the rest of it is read.
    if match_or_fail "$dir ($pattern)" -- \
        rg --no-ignore --hidden -a -o -n -H --no-heading ${case_flag:+"$case_flag"} ${word_flag:+"$word_flag"} \
        -e "$pattern" "$dir" --glob "$glob"; then
        violations=1
    fi
}

scan "$goldens_dir" "$jargon"       '*.txt' -i -w
scan "$explain_dir" "$jargon"       '*.md'  -i -w
scan "$goldens_dir" "$jargon_cased" '*.txt' '' -w
scan "$explain_dir" "$jargon_cased" '*.md'  '' -w
scan "$goldens_dir" "$rust_types"   '*.txt'

if [ "$violations" -ne 0 ]; then
    echo "diagnostic-tone: internal jargon found in user-facing text" >&2
    exit 1
fi

echo "diagnostic-tone: clean"
