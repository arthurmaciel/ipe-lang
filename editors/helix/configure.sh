#!/bin/sh
# editors/helix/configure.sh — one-shot Ipê integration for Helix.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/ipe-lang/compiler/main/editors/helix/configure.sh | sh
#   (or from a checkout: sh editors/helix/configure.sh)
#
# What it does:
#   1. Preflight: ipe, hx (24.03+), a C compiler.
#   2. Builds the tree-sitter grammar into <helix config>/runtime/grammars/ and
#      installs the matching query files into runtime/queries/ipe/.
#   3. Writes the Ipê language + `ipe lsp` definition into languages.toml as
#      one managed block (backed up first; the rest of the file is untouched).
#   4. Verifies with `hx --health ipe`; on failure restores languages.toml and
#      exits non-zero.
#
# Environment: IPE_EDITORS_REF (git ref a curl run fetches, default main),
# XDG_CONFIG_HOME, CC.

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
IPE_TAG="Helix"

# --- preflight ---------------------------------------------------------------
need ipe "install the Ipê toolchain first: https://github.com/ipe-lang/compiler"
need hx "install Helix first: https://helix-editor.com"

HX_VERSION="$(hx --version 2>/dev/null | awk '{print $2}')"
[ -n "$HX_VERSION" ] || die "cannot read the Helix version from 'hx --version'"
# The list form of `comment-tokens` and `block-comment-tokens` need 24.03+.
ipe_version_ge "$HX_VERSION" "24.03" || die "Helix $HX_VERSION is too old — Ipê needs Helix 24.03 or newer"

CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/helix"
LANG_FILE="$CONFIG_DIR/languages.toml"
RUNTIME="$CONFIG_DIR/runtime"
EXT="$(ipe_dylib_ext)"

# --- grammar + queries ---------------------------------------------------------
ipe_workdir
ipe_fetch_grammar "$IPE_WORK/grammar"
ipe_build_grammar "$IPE_WORK/grammar" "$IPE_WORK/ipe.$EXT"
ipe_install_file "$IPE_WORK/ipe.$EXT" "$RUNTIME/grammars/ipe.$EXT"
for q in $IPE_QUERY_NAMES; do
    ipe_install_file "$IPE_WORK/grammar/queries/$q.scm" "$RUNTIME/queries/ipe/$q.scm"
done

# --- languages.toml --------------------------------------------------------------
# An older version of this script appended an unmanaged, marked block; fold it
# into the managed one. A hand-written Ipê definition is left alone: TOML
# rejects a repeated `[language-server.ipe-lsp]` table, so adding ours next to
# it would break every language in Helix.
LEGACY_BEGIN='# --- Ipê (added by editors/helix/configure.sh) ---'
if [ -f "$LANG_FILE" ] && grep -qF "$LEGACY_BEGIN" "$LANG_FILE"; then
    b="$(ipe_backup "$LANG_FILE")"
    say "backed up $LANG_FILE -> $b"
    awk -v begin="$LEGACY_BEGIN" '
        index($0, begin) { skip = 1; next }
        skip && index($0, "# --- end Ipê ---") { skip = 0; next }
        !skip { print }
    ' "$b" > "$LANG_FILE.ipe-tmp.$$" && mv -f "$LANG_FILE.ipe-tmp.$$" "$LANG_FILE" \
        || die "cannot rewrite $LANG_FILE"
    say "removed the unmanaged block an older configure.sh wrote"
fi

IPE_LAST_BACKUP=""
if [ -f "$LANG_FILE" ] && ! ipe_has_block "$LANG_FILE" \
    && grep -Eq '^[[:space:]]*\[language-server\.ipe-lsp\]|^[[:space:]]*name[[:space:]]*=[[:space:]]*"ipe"' "$LANG_FILE"; then
    warn "$LANG_FILE already defines Ipê outside a managed block — leaving it as is"
    warn "delete that definition and re-run to let this script manage it"
else
    ipe_fetch "editors/helix/languages.toml" "$IPE_WORK/block"
    ipe_write_block "$LANG_FILE" "#" "$IPE_WORK/block"
fi

# --- verify (fail closed) -----------------------------------------------------------
if hx --health ipe > "$IPE_WORK/health" 2>&1; then HX_OK=1; else HX_OK=0; fi
ipe_strip_ansi < "$IPE_WORK/health" > "$IPE_WORK/health.txt"
if [ "$HX_OK" != 1 ] || ! grep -q '^Highlight queries: ✓' "$IPE_WORK/health.txt"; then
    cat "$IPE_WORK/health.txt" >&2
    if [ -n "$IPE_LAST_BACKUP" ]; then ipe_restore "$LANG_FILE" "$IPE_LAST_BACKUP"; fi
    die "'hx --health ipe' did not confirm the Ipê setup"
fi

say "setup complete (grammar, queries, ipe lsp) — verified with 'hx --health ipe'"
say "open the folder holding package.ipe; gd = go to definition, <space>a = code actions"
if ipe_version_ge "$HX_VERSION" "25.01"; then
    say "tip: for inline diagnostics add to $CONFIG_DIR/config.toml:"
    printf '  [editor]\n  end-of-line-diagnostics = "hint"\n  [editor.inline-diagnostics]\n  cursor-line = "warning"\n  other-lines = "error"\n'
fi
