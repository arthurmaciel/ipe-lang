#!/bin/sh
# editors/zed/configure.sh — one-shot Ipê integration for Zed.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/arthurmaciel/ipe-lang/main/editors/zed/configure.sh | sh
#
# What it does:
#   1. Preflight: ipe, zed (or zeditor), curl, jq present.
#   2. Merges the Ipê language + LSP block into ~/.config/zed/settings.json
#      (idempotent — skips if the ipe-lsp key already present).
#   3. Prints the dev-extension install step (Zed grammar install requires
#      the GUI — fully automated grammar install is not yet possible via CLI).
#
# Non-destructive: backs up settings.json before any edit.

set -eu

# --- helpers -----------------------------------------------------------------

die() { printf 'error: %s\n' "$*" >&2; exit 1; }

check_cmd() {
    command -v "$1" >/dev/null 2>&1 || die "'$1' not found — $2"
}

# --- preflight ---------------------------------------------------------------

check_cmd ipe   "install the Ipê toolchain first: https://github.com/arthurmaciel/ipe-lang"
check_cmd curl  "install curl"
check_cmd jq    "install jq (https://jqlang.github.io/jq/) — needed to safely merge JSON config"

# Zed binary may be 'zed' or 'zeditor' depending on platform/install.
ZED_CMD=""
for candidate in zed zeditor; do
    if command -v "$candidate" >/dev/null 2>&1; then
        ZED_CMD="$candidate"
        break
    fi
done
[ -n "$ZED_CMD" ] || die "Zed not found ('zed' or 'zeditor') — install Zed first: https://zed.dev"

# --- config dir --------------------------------------------------------------

CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/zed"
mkdir -p "$CONFIG_DIR"
SETTINGS="$CONFIG_DIR/settings.json"

# --- idempotency guard -------------------------------------------------------

if [ -f "$SETTINGS" ] && jq -e '.language_servers["ipe-lsp"]' "$SETTINGS" >/dev/null 2>&1; then
    printf 'Zed: ipe-lsp already configured in %s — skipping.\n' "$SETTINGS"
else
    # Back up before any edit.
    if [ -f "$SETTINGS" ]; then
        cp "$SETTINGS" "$SETTINGS.bak"
        printf 'Zed: backed up %s -> %s.bak\n' "$SETTINGS" "$SETTINGS"
    fi

    # Read existing JSON (or start with {}), merge in Ipê keys.
    EXISTING="{}"
    if [ -f "$SETTINGS" ] && [ -s "$SETTINGS" ]; then
        EXISTING="$(cat "$SETTINGS")"
    fi

    printf '%s\n' "$EXISTING" | jq \
        --argjson ipeLang '{
            "path_separators": "/",
            "matcher": { "filename": "\\.ipe$" },
            "autoclose_before": "}] \")\n\t",
            "brackets": [
                { "start": "{", "end": "}", "close": true, "newline": true },
                { "start": "[", "end": "]", "close": true, "newline": true },
                { "start": "(", "end": ")", "close": false, "newline": false }
            ],
            "line_comments": ["-- "],
            "block_comment": ["{- ", " -}"]
        }' \
        --argjson ipeLsp '{
            "binary": { "path": "ipe", "arguments": ["lsp"] }
        }' \
        '
          .languages["Ipê"]           = $ipeLang |
          .language_servers["ipe-lsp"] = $ipeLsp  |
          .auto_formatter             = true       |
          .format_on_save             = "on"
        ' > "$SETTINGS.tmp" && mv "$SETTINGS.tmp" "$SETTINGS"

    printf 'Zed: Ipê config merged into %s\n' "$SETTINGS"
fi

# --- grammar (requires GUI) --------------------------------------------------

printf '\n'
printf '================================================================\n'
printf 'Zed: settings updated.\n'
printf '\n'
printf 'Syntax highlighting requires installing the Zed extension via the GUI:\n'
printf '\n'
printf '  1. Open Zed.\n'
printf '  2. Extensions -> Install Dev Extension.\n'
printf '  3. Select the directory: editors/zed-ipe/ (from a local checkout of\n'
printf '     https://github.com/arthurmaciel/ipe-lang).\n'
printf '\n'
printf 'The extension bundles the tree-sitter grammar and highlight queries.\n'
printf 'Zed builds the grammar on first load.\n'
printf '================================================================\n'
printf '\n'
printf 'Zed setup complete (LSP + formatter). Grammar install step above required.\n'
