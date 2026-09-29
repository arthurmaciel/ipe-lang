#!/bin/sh
# editors/neovim/configure.sh — one-shot Ipê integration for Neovim 0.11+.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/ipe-lang/compiler/main/editors/neovim/configure.sh | sh
#   (or from a checkout: sh editors/neovim/configure.sh)
#
# What it does:
#   1. Preflight: ipe, nvim (0.11+), a C compiler.
#   2. Installs into Neovim's data "site" directory (on the default
#      runtimepath, outside your config):
#        parser/ipe.so    the tree-sitter grammar, built locally
#        queries/ipe/     the matching query files
#        plugin/ipe.lua   filetype + highlighting + `ipe lsp` via the built-in
#                         client — no plugins, no init.lua edit
#   3. Verifies headlessly that Neovim loads the parser, the highlight query
#      and the LSP config; exits non-zero otherwise.
#
# Environment: IPE_EDITORS_REF (git ref a curl run fetches, default main),
# XDG_DATA_HOME, XDG_CONFIG_HOME, CC.

set -eu

# --- bootstrap the shared helpers (local checkout, else the same ref) -------
IPE_EDITORS_REF="${IPE_EDITORS_REF:-main}"
IPE_SRC_ROOT=""
case "$0" in
    */configure.sh)
        _d="$(cd "$(dirname "$0")/../.." 2>/dev/null && pwd)" || _d=""
        if [ -n "$_d" ] && [ -f "$_d/editors/lib/ipe-editors.sh" ]; then IPE_SRC_ROOT="$_d"; fi ;;
esac
if [ -n "$IPE_SRC_ROOT" ]; then
    # shellcheck source=../lib/ipe-editors.sh
    . "$IPE_SRC_ROOT/editors/lib/ipe-editors.sh"
else
    _lib="$(mktemp)" || exit 1
    curl -fsSL "https://raw.githubusercontent.com/ipe-lang/compiler/$IPE_EDITORS_REF/editors/lib/ipe-editors.sh" -o "$_lib" \
        || { rm -f "$_lib"; printf 'error: cannot download editors/lib/ipe-editors.sh\n' >&2; exit 1; }
    # shellcheck source=/dev/null
    . "$_lib"
    rm -f "$_lib"
fi
IPE_TAG="Neovim"

# --- preflight ---------------------------------------------------------------
need ipe "install the Ipê toolchain first: https://github.com/ipe-lang/compiler"
need nvim "install Neovim 0.11+ first: https://neovim.io"

NVIM_VERSION="$(nvim --version 2>/dev/null | sed -n '1s/^NVIM v\([0-9][0-9.]*\).*/\1/p')"
[ -n "$NVIM_VERSION" ] || die "cannot read the Neovim version from 'nvim --version'"
# vim.lsp.config / vim.lsp.enable and vim.lsp.completion arrived in 0.11.
ipe_version_ge "$NVIM_VERSION" "0.11" \
    || die "Neovim $NVIM_VERSION is too old — Ipê needs Neovim 0.11 or newer ($(command -v nvim))"

SITE="${XDG_DATA_HOME:-$HOME/.local/share}/nvim/site"

# --- grammar, queries, plugin ---------------------------------------------------
ipe_workdir
ipe_fetch_grammar "$IPE_WORK/grammar"
ipe_build_grammar "$IPE_WORK/grammar" "$IPE_WORK/ipe.so"
ipe_install_file "$IPE_WORK/ipe.so" "$SITE/parser/ipe.so"
for q in $IPE_QUERY_NAMES; do
    ipe_install_file "$IPE_WORK/grammar/queries/$q.scm" "$SITE/queries/ipe/$q.scm"
done
ipe_fetch "editors/neovim/ipe.lua" "$IPE_WORK/ipe.lua"
ipe_install_file "$IPE_WORK/ipe.lua" "$SITE/plugin/ipe.lua"

# An older version of this script put the queries in the config directory,
# which precedes the site directory on the runtimepath and would shadow the
# fresh queries with stale ones. Move that directory aside (never delete it)
# when it holds nothing but those query files.
LEGACY="${XDG_CONFIG_HOME:-$HOME/.config}/nvim/queries/ipe"
if [ -d "$LEGACY" ]; then
    only_ours=1
    for f in "$LEGACY"/* "$LEGACY"/.[!.]*; do
        [ -e "$f" ] || continue
        case "$(basename "$f")" in
            highlights.scm | injections.scm | locals.scm | tags.scm | textobjects.scm | indents.scm) ;;
            *) only_ours=0 ;;
        esac
    done
    if [ "$only_ours" = 1 ]; then
        moved="$LEGACY.ipe-backup-$(date +%Y%m%d-%H%M%S)"
        while [ -e "$moved" ]; do moved="$moved-1"; done
        mv "$LEGACY" "$moved" || die "cannot move $LEGACY aside"
        say "moved stale queries $LEGACY -> $moved (they would shadow the new ones)"
    else
        warn "$LEGACY holds your own files and shadows the installed Ipê queries — merge or remove it"
    fi
fi

# --- verify (fail closed) -------------------------------------------------------
cat > "$IPE_WORK/check.lua" << 'LUA'
local ok, err = pcall(function()
  assert(vim.filetype.match({ filename = "x.ipe" }) == "ipe", "filetype not detected")
  assert(vim.treesitter.language.add("ipe"), "parser not loadable")
  assert(vim.treesitter.query.get("ipe", "highlights"), "highlight query missing")
  local parser = vim.treesitter.get_string_parser("module Main exposing (main)\n", "ipe")
  assert(not parser:parse()[1]:root():has_error(), "parser rejects a trivial module")
  assert(vim.lsp.config.ipe and vim.lsp.config.ipe.cmd[1] == "ipe", "ipe LSP config missing")
end)
if not ok then
  io.stderr:write(tostring(err) .. "\n")
  vim.cmd("cquit 1")
end
vim.cmd("qall!")
LUA
nvim --headless -i NONE -c "luafile $IPE_WORK/check.lua" > "$IPE_WORK/check.log" 2>&1 || {
    cat "$IPE_WORK/check.log" >&2
    die "headless Neovim check failed — see above"
}

say "setup complete (parser, queries, plugin/ipe.lua) — verified headlessly"
say "open the folder holding package.ipe; completion pops up after '.' (<C-x><C-o> any time),"
say "<C-]> = go to definition, gra = code actions, gq/:lua vim.lsp.buf.format() = format"
