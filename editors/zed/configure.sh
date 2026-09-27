#!/bin/sh
# editors/zed/configure.sh — prepares the Ipê extension for Zed.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/arthurmaciel/ipe-lang/main/editors/zed/configure.sh | sh
#   (or from a checkout: sh editors/zed/configure.sh)
#
# What it does:
#   1. Preflight: ipe, zed (or zeditor), rustup + cargo (Zed compiles the
#      extension's Rust part itself), git.
#   2. Assembles the extension (editors/zed-ipe + the grammar's highlight
#      queries) into ${XDG_DATA_HOME:-~/.local/share}/ipe/zed-ipe.
#   3. Prints the one step Zed only offers in its UI: install that directory
#      as a dev extension. The extension provides highlighting and starts
#      `ipe lsp` itself — settings.json is never edited.
#
# Environment: IPE_EDITORS_REF (git ref a curl run fetches, default main),
# XDG_DATA_HOME, XDG_CONFIG_HOME.

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
    curl -fsSL "https://raw.githubusercontent.com/arthurmaciel/ipe-lang/$IPE_EDITORS_REF/editors/lib/ipe-editors.sh" -o "$_lib" \
        || { rm -f "$_lib"; printf 'error: cannot download editors/lib/ipe-editors.sh\n' >&2; exit 1; }
    # shellcheck source=/dev/null
    . "$_lib"
    rm -f "$_lib"
fi
IPE_TAG="Zed"

# --- preflight ---------------------------------------------------------------
need ipe "install the Ipê toolchain first: https://github.com/arthurmaciel/ipe-lang"
if ! ipe_have zed && ! ipe_have zeditor; then
    die "Zed not found ('zed' or 'zeditor' on PATH) — install Zed first: https://zed.dev"
fi
need rustup "Zed builds the extension's Rust part with rustup — install it: https://rustup.rs"
need cargo "install a Rust toolchain with rustup: https://rustup.rs"
need git "Zed fetches the grammar with git"

EXT_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/ipe/zed-ipe"

# --- assemble the extension -------------------------------------------------------
# The highlight queries come straight from the grammar (their single source);
# the rest is editors/zed-ipe.
ipe_workdir
X="$IPE_WORK/zed-ipe"
for f in extension.toml Cargo.toml Cargo.lock .cargo/config.toml src/lib.rs languages/ipe/config.toml; do
    ipe_fetch "editors/zed-ipe/$f" "$X/$f"
done
# Zed reads only highlights.scm from these; it rejects a capture-less
# injections query, so none is shipped.
ipe_fetch "$IPE_GRAMMAR_DIR/queries/highlights.scm" "$X/languages/ipe/highlights.scm"
grep -Eq '^rev = "[0-9a-f]{40}"$' "$X/extension.toml" \
    || die "extension.toml does not pin the grammar to a commit"

for f in extension.toml Cargo.toml Cargo.lock .cargo/config.toml src/lib.rs \
    languages/ipe/config.toml languages/ipe/highlights.scm; do
    ipe_install_file "$X/$f" "$EXT_DIR/$f"
done
# Query files an earlier run installed that Zed must no longer load.
for q in injections locals; do
    stale="$EXT_DIR/languages/ipe/$q.scm"
    if [ -f "$stale" ]; then
        b="$(ipe_backup "$stale")"
        rm -f "$stale"
        say "retired $stale (kept as $b)"
    fi
done

# --- settings.json: read-only check ---------------------------------------------------
# Older versions of this script merged keys into settings.json; the extension
# makes them unnecessary and some were never valid Zed settings. Point them out,
# never edit the file.
SETTINGS="${XDG_CONFIG_HOME:-$HOME/.config}/zed/settings.json"
if [ -f "$SETTINGS" ] && grep -Eq '"ipe-lsp"|"Ipê"|"auto_formatter"' "$SETTINGS"; then
    warn "$SETTINGS has Ipê keys from an older setup (\"Ipê\", \"ipe-lsp\", \"auto_formatter\")"
    warn "they are no longer needed — remove them by hand; this script does not edit settings.json"
fi

say "extension assembled at $EXT_DIR"
cat << EOF

  One step left — Zed installs local extensions only from its UI:
    1. Open Zed; run the command palette action  zed: install dev extension
    2. Choose the directory:  $EXT_DIR
  Zed compiles the grammar and the extension (about a minute, first time only),
  then highlights .ipe files and starts 'ipe lsp' for them. Trust the project
  folder when Zed asks, or language servers stay off. Re-run this script and
  "Rebuild" the extension in Zed's Extensions page to update.

EOF
