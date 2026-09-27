#!/bin/sh
# editors/emacs/configure.sh — one-shot Ipê integration for Emacs and Doom Emacs.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/arthurmaciel/ipe-lang/main/editors/emacs/configure.sh | sh
#   (or from a checkout: sh editors/emacs/configure.sh)
#
# What it does:
#   1. Preflight: ipe, emacs (29+, for the built-in Eglot LSP client).
#   2. Installs ipe-mode.el (highlighting, comments, indentation, `ipe lsp`
#      via Eglot) into ${XDG_DATA_HOME:-~/.local/share}/ipe/emacs/.
#   3. Adds a managed two-line block that loads it to your init file — or to
#      $DOOMDIR/config.el when the Emacs directory is Doom Emacs. The file is
#      backed up first; the rest of it is untouched.
#   4. Verifies in a clean batch Emacs that the mode loads, highlights and
#      registers `ipe lsp` with Eglot; exits non-zero otherwise.
#
# Environment: IPE_EDITORS_REF (git ref a curl run fetches, default main),
# EMACSDIR (your Emacs directory, e.g. for `emacs --init-directory`),
# DOOMDIR (Doom's private config directory), XDG_DATA_HOME, XDG_CONFIG_HOME.

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
IPE_TAG="Emacs"

# --- preflight ---------------------------------------------------------------
need ipe "install the Ipê toolchain first: https://github.com/arthurmaciel/ipe-lang"
need emacs "install Emacs 29+ first: https://www.gnu.org/software/emacs"

EMACS_MAJOR="$(emacs -Q --batch --eval '(princ emacs-major-version)' 2>/dev/null)" || EMACS_MAJOR=""
case "$EMACS_MAJOR" in
    '' | *[!0-9]*) die "cannot read the Emacs version from 'emacs --batch'" ;;
esac
# Eglot, the LSP client ipe-mode uses, is built in from Emacs 29.
[ "$EMACS_MAJOR" -ge 29 ] || die "Emacs $EMACS_MAJOR is too old — Ipê needs Emacs 29 or newer"

# --- locate the init file (Emacs's own lookup order) ---------------------------
XDG_EMACS="${XDG_CONFIG_HOME:-$HOME/.config}/emacs"
if [ -n "${EMACSDIR:-}" ]; then
    EMACS_DIR="${EMACSDIR%/}"
elif [ -f "$HOME/.emacs" ] || [ -f "$HOME/.emacs.el" ]; then
    EMACS_DIR=""
elif [ -d "$HOME/.emacs.d" ]; then
    EMACS_DIR="$HOME/.emacs.d"
else
    EMACS_DIR="$XDG_EMACS"
fi

if [ -n "$EMACS_DIR" ] && { [ -f "$EMACS_DIR/.doom" ] || [ -f "$EMACS_DIR/lisp/doom.el" ]; }; then
    FLAVOR="Doom Emacs"
    if [ -n "${DOOMDIR:-}" ]; then
        DOOM_PRIVATE="${DOOMDIR%/}"
    elif [ -d "${XDG_CONFIG_HOME:-$HOME/.config}/doom" ]; then
        DOOM_PRIVATE="${XDG_CONFIG_HOME:-$HOME/.config}/doom"
    else
        DOOM_PRIVATE="$HOME/.doom.d"
    fi
    INIT_FILE="$DOOM_PRIVATE/config.el"
    [ -f "$INIT_FILE" ] || die "Doom Emacs found at $EMACS_DIR but no private config (config.el) in \$DOOMDIR, ~/.config/doom or ~/.doom.d — run 'doom install' first"
elif [ -z "$EMACS_DIR" ]; then
    FLAVOR="Emacs"
    if [ -f "$HOME/.emacs" ]; then INIT_FILE="$HOME/.emacs"; else INIT_FILE="$HOME/.emacs.el"; fi
else
    FLAVOR="Emacs"
    INIT_FILE="$EMACS_DIR/init.el"
fi

LISP_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/ipe/emacs"

# --- install ipe-mode.el (validated first) -----------------------------------------
ipe_workdir
mkdir -p "$IPE_WORK/lisp"
ipe_fetch "editors/emacs/ipe-mode.el" "$IPE_WORK/lisp/ipe-mode.el"
emacs -Q --batch --eval '(setq byte-compile-error-on-warn t)' \
    -f batch-byte-compile "$IPE_WORK/lisp/ipe-mode.el" > "$IPE_WORK/compile.log" 2>&1 || {
    cat "$IPE_WORK/compile.log" >&2
    die "ipe-mode.el does not byte-compile cleanly"
}
ipe_install_file "$IPE_WORK/lisp/ipe-mode.el" "$LISP_DIR/ipe-mode.el"

# --- wire it into the init file -------------------------------------------------------
cat > "$IPE_WORK/block" << EOF
(add-to-list 'load-path "$LISP_DIR")
(require 'ipe-mode)
EOF
ipe_write_block "$INIT_FILE" ";;" "$IPE_WORK/block"

# --- verify (fail closed) -------------------------------------------------------------
cat > "$IPE_WORK/check.el" << 'ELISP'
(condition-case err
    (progn
      (require 'eglot)
      (unless (eq (assoc-default "x.ipe" auto-mode-alist #'string-match) 'ipe-mode)
        (error "auto-mode-alist does not map .ipe to ipe-mode"))
      (with-temp-buffer
        (insert "module Main exposing (main)\n\nmain : Int\nmain =\n    1 -- note\n")
        (ipe-mode)
        (font-lock-ensure)
        (goto-char (point-min))
        (unless (eq (get-text-property (point) 'face) 'font-lock-keyword-face)
          (error "keyword not highlighted"))
        (search-forward "note")
        (unless (memq (get-text-property (1- (point)) 'face) '(font-lock-comment-face))
          (error "comment not highlighted")))
      (unless (assq 'ipe-mode eglot-server-programs)
        (error "ipe lsp not registered with Eglot"))
      (kill-emacs 0))
  (error (princ (format "%s\n" (error-message-string err)) #'external-debugging-output)
         (kill-emacs 1)))
ELISP
if ! emacs -Q --batch --eval "(setq ipe-auto-eglot nil)" --eval "(add-to-list 'load-path \"$LISP_DIR\")" \
    --eval "(require 'ipe-mode)" -l "$IPE_WORK/check.el" > "$IPE_WORK/check.log" 2>&1; then
    cat "$IPE_WORK/check.log" >&2
    if [ -n "$IPE_LAST_BACKUP" ]; then ipe_restore "$INIT_FILE" "$IPE_LAST_BACKUP"; fi
    die "batch Emacs check of ipe-mode failed"
fi

say "setup complete for $FLAVOR ($INIT_FILE) — verified in batch Emacs"
say "open a .ipe file inside the folder holding package.ipe; Eglot starts 'ipe lsp':"
say "completion = C-M-i (or your completion UI), M-. = go to definition, C-c C-a = code actions"
