#!/usr/bin/env bash
# Fail-closed gate: user-facing shipped source must not name the private
# reference implementation.  The forbidden-term list lives in
# tools/scripts/reference-impl-forbidden-terms.txt — the single source of truth
# shared with the embedded-stdlib Rust test in src/stdlib/src/lib.rs.
#
# Usage:  bash tools/scripts/no-reference-impl-leak.sh
# Exit 0  — no leaks found.
# Exit 1  — one or more violations; each printed as "file:line: content".
#
# Scanned set (user-facing shipped source):
#   src/stdlib/Ipe/**/*.ipe   — embedded verbatim into the compiler binary
#   src/stdlib/src/**/*.rs    — the embed glue (include_str! wrappers)
#   README.md                 — public landing page
#   docs/*.md                 — public top-level documentation
#
# Carve-outs (SSOT — enumerated here, not spread across the codebase):
#   src/ipe-cli/tests/            — internal test oracles, never shipped to users
#   examples/sky/                 — tracked reference-impl mirror, not our source

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
TERMS_FILE="$SCRIPT_DIR/reference-impl-forbidden-terms.txt"

if [[ ! -f "$TERMS_FILE" ]]; then
    echo "ERROR: term list not found: $TERMS_FILE" >&2
    exit 1
fi

cd "$REPO_ROOT"

# ---------------------------------------------------------------------------
# Build the forbidden-phrase alternation from the term file.
# Lines starting with # or \b (word-bounded terms handled separately) are
# excluded from the general pattern.
# ---------------------------------------------------------------------------
PHRASE_PATTERN=""
while IFS= read -r line; do
    [[ -z "$line" || "$line" == \#* || "$line" == \\b* ]] && continue
    if [[ -z "$PHRASE_PATTERN" ]]; then
        PHRASE_PATTERN="$line"
    else
        PHRASE_PATTERN="$PHRASE_PATTERN|$line"
    fi
done < "$TERMS_FILE"

if [[ -z "$PHRASE_PATTERN" ]]; then
    echo "ERROR: no phrase terms found in $TERMS_FILE" >&2
    exit 1
fi

# ---------------------------------------------------------------------------
# Carve-out helper: returns 0 if the repo-relative path is carved out.
# ---------------------------------------------------------------------------
is_carved_out() {
    local fp="$1"
    local carve
    for carve in \
        "src/ipe-cli/tests" \
        "examples/sky"
    do
        if [[ "$fp" == "$carve" || "$fp" == "$carve/"* ]]; then
            return 0
        fi
    done
    return 1
}

FOUND=0

# ---------------------------------------------------------------------------
# scan PATTERN ROOT [RG_EXTRA_ARGS...] — run rg on ROOT, filter carve-outs
# and print violations.  Uses process substitution (not a pipe) so the outer
# FOUND variable is set in the current shell, not a subshell.
# Optional extra rg args (e.g. --max-depth 1 --glob "*.md") must be passed
# as separate arguments after ROOT.
# ---------------------------------------------------------------------------
scan_phrase() {
    local pat="$1"
    local root="$2"
    shift 2
    [[ ! -e "$root" ]] && return
    while IFS= read -r hit; do
        local filepath="${hit%%:*}"
        local rest="${hit#*:}"
        local lineno="${rest%%:*}"
        local content="${rest#*:}"
        is_carved_out "$filepath" && continue
        echo "$filepath:$lineno: $content"
        FOUND=1
    done < <(rg -n -i "$pat" "$@" "$root" 2>/dev/null || true)
}

# ---------------------------------------------------------------------------
# scan_sky ROOT — scan for \bSky\b, filtering out doc-link lines.
# ---------------------------------------------------------------------------
scan_sky() {
    local root="$1"
    [[ ! -e "$root" ]] && return
    while IFS= read -r hit; do
        local filepath="${hit%%:*}"
        local rest="${hit#*:}"
        local lineno="${rest%%:*}"
        local content="${rest#*:}"
        is_carved_out "$filepath" && continue
        # Allow lines whose only Sky occurrence is a doc-link to our
        # own divergence ledger — an internal cross-reference, not a
        # private-impl citation.
        [[ "$content" == *"divergences-from-sky"* ]] && continue
        echo "$filepath:$lineno: $content"
        FOUND=1
    done < <(rg -n -i "\\bSky\\b" "$root" 2>/dev/null || true)
}

# ---------------------------------------------------------------------------
# Pass 1 — phrase terms: all user-facing shipped source.
# docs scan limited to top-level *.md (--max-depth 1); architecture/adr are
# developer-facing internal docs.
# ---------------------------------------------------------------------------
scan_phrase "$PHRASE_PATTERN" "src/stdlib/Ipe"
scan_phrase "$PHRASE_PATTERN" "src/stdlib/src"
scan_phrase "$PHRASE_PATTERN" "README.md"
scan_phrase "$PHRASE_PATTERN" "docs" --max-depth 1 --glob "*.md"

# ---------------------------------------------------------------------------
# Pass 2 — word-bounded src/-only terms (Sky, Std.).
# These appear legitimately in docs and internal Rust; scan only the .ipe
# stdlib source that ships verbatim to users.
# ---------------------------------------------------------------------------
scan_sky "src/stdlib/Ipe"
scan_sky "src/stdlib/src"
scan_phrase "Std\\." "src/stdlib/Ipe"
scan_phrase "Std\\." "src/stdlib/src"

# ---------------------------------------------------------------------------
# Result
# ---------------------------------------------------------------------------
if [[ "$FOUND" -eq 1 ]]; then
    echo "" >&2
    echo "FAIL: reference-implementation leak(s) found in shipped source." >&2
    echo "      Rewrite each hit to describe the behaviour on its own terms." >&2
    exit 1
fi

echo "OK: no reference-implementation leaks in shipped source."
