#!/bin/sh
# editors/neovim/configure.sh — Ipê query files + config snippet for Neovim.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/arthurmaciel/ipe-lang/main/editors/neovim/configure.sh | sh
#
# What it does:
#   1. Preflight: ipe, nvim, curl present.
#   2. Fetches the four tree-sitter query files into ~/.config/nvim/queries/ipe/
#      (idempotent — skips files already present).
#   3. Prints the Lua config snippet you must paste into your init.lua.
#
# Non-destructive: does NOT edit init.lua or any Neovim config file.

set -eu

# --- helpers -----------------------------------------------------------------

die() { printf 'error: %s\n' "$*" >&2; exit 1; }

check_cmd() {
    command -v "$1" >/dev/null 2>&1 || die "'$1' not found — $2"
}

# --- preflight ---------------------------------------------------------------

check_cmd ipe   "install the Ipê toolchain first: https://github.com/arthurmaciel/ipe-lang"
check_cmd nvim  "install Neovim first: https://neovim.io"
check_cmd curl  "install curl"

# --- query files -------------------------------------------------------------

QUERY_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/nvim/queries/ipe"
mkdir -p "$QUERY_DIR"

BASE="https://raw.githubusercontent.com/arthurmaciel/ipe-lang/main/editors/tree-sitter-ipe/queries"

for q in highlights injections locals tags textobjects indents; do
    DEST="$QUERY_DIR/$q.scm"
    if [ -f "$DEST" ]; then
        printf 'Neovim: query %s.scm already present — skipping.\n' "$q"
    else
        curl -fsSL "$BASE/$q.scm" -o "$DEST"
        printf 'Neovim: installed query %s.scm\n' "$q"
    fi
done

# --- config snippet ----------------------------------------------------------

printf '\n'
printf '================================================================\n'
printf 'Neovim: query files installed at %s\n' "$QUERY_DIR"
printf '\n'
printf 'Add the following to your init.lua (requires nvim-lspconfig and\n'
printf 'nvim-treesitter; install conform.nvim for format-on-save):\n'
printf '================================================================\n'
printf '\n'

cat << 'LUA_SNIPPET'
-- ── Ipê — LSP ────────────────────────────────────────────────────────────────
local lspconfig = require("lspconfig")
local configs   = require("lspconfig.configs")

if not configs.ipe then
  configs.ipe = {
    default_config = {
      cmd      = { "ipe", "lsp" },
      filetypes = { "ipe" },
      root_dir = lspconfig.util.root_pattern("package.ipe", ".git"),
      settings = {},
    },
  }
end

lspconfig.ipe.setup({})

-- Filetype detection
vim.filetype.add({ extension = { ipe = "ipe" } })

-- ── Ipê — tree-sitter grammar ────────────────────────────────────────────────
local parsers = require("nvim-treesitter.parsers").get_parser_configs()

parsers.ipe = {
  install_info = {
    -- Fetch grammar from the repo's subdirectory:
    url      = "https://github.com/arthurmaciel/ipe-lang",
    location = "editors/tree-sitter-ipe",
    files    = { "src/parser.c", "src/scanner.c" },
    branch   = "main",
  },
  filetype = "ipe",
}
-- Run :TSInstall ipe after adding the above.

-- ── Ipê — format on save (requires conform.nvim) ─────────────────────────────
require("conform").setup({
  formatters_by_ft = { ipe = { "ipe_fmt" } },
})

require("conform").formatters.ipe_fmt = {
  command = "ipe",
  args    = { "fmt", "--stdin" },
  stdin   = true,
}
LUA_SNIPPET

printf '================================================================\n'
printf '\n'
printf 'After pasting the snippet, run :TSInstall ipe inside Neovim.\n'
printf 'Open a directory containing package.ipe for cross-module analysis.\n'
