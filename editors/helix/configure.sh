#!/bin/sh
# editors/helix/configure.sh — one-shot Ipê integration for Helix.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/arthurmaciel/ipe-lang/main/editors/helix/configure.sh | sh
#
# What it does:
#   1. Preflight: ipe, hx, curl present.
#   2. Appends the Ipê language + grammar block to languages.toml (idempotent).
#   3. Fetches + builds the grammar via hx --grammar.
#   4. Curls the four query files into the Helix runtime.
#
# Non-destructive: backs up languages.toml before any edit; skips if an
# existing `[language-server.ipe-lsp]` block is already present.

set -eu

# --- helpers -----------------------------------------------------------------

die() { printf 'error: %s\n' "$*" >&2; exit 1; }

check_cmd() {
    command -v "$1" >/dev/null 2>&1 || die "'$1' not found — $2"
}

# --- preflight ---------------------------------------------------------------

check_cmd ipe    "install the Ipê toolchain first: https://github.com/arthurmaciel/ipe-lang"
check_cmd hx     "install Helix first: https://helix-editor.com"
check_cmd curl   "install curl"

# --- config dir --------------------------------------------------------------

CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/helix"
mkdir -p "$CONFIG_DIR"
LANG_FILE="$CONFIG_DIR/languages.toml"

# --- idempotency guard -------------------------------------------------------

if [ -f "$LANG_FILE" ] && grep -qF '[language-server.ipe-lsp]' "$LANG_FILE" 2>/dev/null; then
    printf 'Helix: Ipê block already present in %s — skipping append.\n' "$LANG_FILE"
else
    # Back up before any edit.
    if [ -f "$LANG_FILE" ]; then
        cp "$LANG_FILE" "$LANG_FILE.bak"
        printf 'Helix: backed up %s -> %s.bak\n' "$LANG_FILE" "$LANG_FILE"
    fi

    # Append a blank separator only when file exists and is non-empty.
    if [ -f "$LANG_FILE" ] && [ -s "$LANG_FILE" ]; then
        printf '\n' >> "$LANG_FILE"
    fi

    cat >> "$LANG_FILE" << 'TOML_BLOCK'
# --- Ipê (added by editors/helix/configure.sh) ---
[[language]]
name = "ipe"
scope = "source.ipe"
file-types = ["ipe"]
roots = ["package.ipe"]
language-servers = ["ipe-lsp"]
auto-format = true
formatter = { command = "ipe", args = ["fmt", "--stdin"] }
comment-tokens = ["--"]
block-comment-tokens = { start = "{-", end = "-}" }
indent = { tab-width = 4, unit = "    " }

[language-server.ipe-lsp]
command = "ipe"
args = ["lsp"]

[[grammar]]
name = "ipe"
source = { git = "https://github.com/arthurmaciel/ipe-lang", rev = "main", subpath = "editors/tree-sitter-ipe" }
# --- end Ipê ---
TOML_BLOCK

    printf 'Helix: Ipê config block appended to %s\n' "$LANG_FILE"
fi

# --- grammar fetch + build ---------------------------------------------------

printf 'Helix: fetching grammar...\n'
hx --grammar fetch
printf 'Helix: building grammar...\n'
hx --grammar build

# --- query files -------------------------------------------------------------

QUERY_DIR="$CONFIG_DIR/runtime/queries/ipe"
mkdir -p "$QUERY_DIR"

BASE="https://raw.githubusercontent.com/arthurmaciel/ipe-lang/main/editors/tree-sitter-ipe/queries"

for q in highlights injections locals tags textobjects indents; do
    DEST="$QUERY_DIR/$q.scm"
    if [ -f "$DEST" ]; then
        printf 'Helix: query %s.scm already present — skipping.\n' "$q"
    else
        curl -fsSL "$BASE/$q.scm" -o "$DEST"
        printf 'Helix: installed query %s.scm\n' "$q"
    fi
done

# --- done --------------------------------------------------------------------

printf '\n'
printf 'Helix setup complete.\n'
printf '  config : %s\n' "$LANG_FILE"
printf '  queries: %s\n' "$QUERY_DIR"
printf '\n'
printf 'Verify with: hx --health ipe\n'
printf 'Open a project directory (the one containing package.ipe) for cross-module analysis.\n'
printf '\n'
printf 'Tip: Helix does not draw diagnostics inline by default. For squiggles, add to\n'
printf '     ~/.config/helix/config.toml:\n'
printf '       [editor]\n'
printf '       end-of-line-diagnostics = "hint"\n'
printf '       [editor.inline-diagnostics]\n'
printf '       cursor-line = "warning"\n'
printf '       other-lines = "error"\n'
