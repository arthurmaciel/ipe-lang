#!/bin/sh
# editors/emacs/configure.sh — Ipê query files + config snippet for Emacs.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/arthurmaciel/ipe-lang/main/editors/emacs/configure.sh | sh
#
# What it does:
#   1. Preflight: ipe, emacs, curl present.
#   2. Fetches the four tree-sitter query files into
#      ~/.config/emacs/tree-sitter/queries/ipe/ (Emacs 29+ convention).
#      Idempotent — skips files already present.
#   3. Prints the Elisp config snippet you must paste into your init.el.
#
# Non-destructive: does NOT edit init.el or any Emacs config file.

set -eu

# --- helpers -----------------------------------------------------------------

die() { printf 'error: %s\n' "$*" >&2; exit 1; }

check_cmd() {
    command -v "$1" >/dev/null 2>&1 || die "'$1' not found — $2"
}

# --- preflight ---------------------------------------------------------------

check_cmd ipe    "install the Ipê toolchain first: https://github.com/arthurmaciel/ipe-lang"
check_cmd emacs  "install Emacs first: https://www.gnu.org/software/emacs"
check_cmd curl   "install curl"

# Require Emacs 29+ for built-in treesit.
EMACS_MAJOR="$(emacs --version 2>/dev/null | head -1 | sed 's/[^0-9]*\([0-9]*\).*/\1/')"
if [ -n "$EMACS_MAJOR" ] && [ "$EMACS_MAJOR" -lt 29 ] 2>/dev/null; then
    printf 'warning: Emacs %s detected; treesit is built in from Emacs 29.\n' "$EMACS_MAJOR"
    printf '         The query files are still installed; lsp-mode works on older Emacs.\n'
fi

# --- query files (Emacs 29+ treesit convention) ------------------------------

# Emacs 29 looks for query files in treesit-extra-load-path or in
# ~/.config/emacs/tree-sitter/queries/<lang>/ when configured.
# We place them here; the snippet below wires treesit-extra-load-path.
EMACS_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/emacs"
QUERY_DIR="$EMACS_DIR/tree-sitter/queries/ipe"
mkdir -p "$QUERY_DIR"

BASE="https://raw.githubusercontent.com/arthurmaciel/ipe-lang/main/editors/tree-sitter-ipe/queries"

for q in highlights injections locals tags textobjects indents; do
    DEST="$QUERY_DIR/$q.scm"
    if [ -f "$DEST" ]; then
        printf 'Emacs: query %s.scm already present — skipping.\n' "$q"
    else
        curl -fsSL "$BASE/$q.scm" -o "$DEST"
        printf 'Emacs: installed query %s.scm\n' "$q"
    fi
done

# --- config snippet ----------------------------------------------------------

QUERIES_PARENT="$EMACS_DIR/tree-sitter/queries"

printf '\n'
printf '================================================================\n'
printf 'Emacs: query files installed at %s\n' "$QUERY_DIR"
printf '\n'
printf 'Add the following to your init.el (requires lsp-mode from MELPA;\n'
printf 'Emacs 29+ for treesit highlighting):\n'
printf '================================================================\n'
printf '\n'

# Print snippet with the actual query path interpolated.
cat << 'ELISP_SNIPPET'
;; ── Ipê — treesit query path ─────────────────────────────────────────────────
;; Tell treesit where to find the query files installed by configure.sh.
ELISP_SNIPPET
# The path is the only interpolated value; inject it via printf so the rest of
# the snippet can stay in a QUOTED heredoc (an unquoted one would collapse the
# `\\.ipe\\'` backslashes in the auto-mode-alist regexp into broken elisp).
printf '(add-to-list '\''treesit-extra-load-path'\'' "%s")\n' "$QUERIES_PARENT"
cat << 'ELISP_SNIPPET'

;; ── Ipê — grammar source (Emacs 29+) ─────────────────────────────────────────
;; After adding this, run: M-x treesit-install-language-grammar RET ipe RET
(add-to-list
 'treesit-language-source-alist
 '(ipe "https://github.com/arthurmaciel/ipe-lang"
       :source-dir "editors/tree-sitter-ipe/src"))

;; ── Ipê — major mode ─────────────────────────────────────────────────────────
(define-derived-mode ipe-mode prog-mode "Ipê"
  :group 'languages
  (setq tab-width 4)
  (setq-local format-all-formatters '(("Ipê" . (ipe "fmt" "--stdin"))))
  (when (treesit-ready-p 'ipe)
    (treesit-parser-create 'ipe)
    (treesit-major-mode-setup)))

(add-to-list 'auto-mode-alist '("\\.ipe\\'" . ipe-mode))

;; ── Ipê — LSP (requires lsp-mode) ────────────────────────────────────────────
(use-package lsp-mode
  :ensure t
  :hook ((ipe-mode . lsp-deferred))
  :commands lsp
  :config
  (lsp-register-client
   (make-lsp-client :new-connection (lsp-stdio-connection '("ipe" "lsp"))
                    :major-modes '(ipe-mode)
                    :server-id 'ipe-lsp)))

;; ── Ipê — format on save ─────────────────────────────────────────────────────
(add-hook 'ipe-mode-hook
          (lambda ()
            (add-hook 'before-save-hook
                      (lambda ()
                        (when (executable-find "ipe")
                          (shell-command-on-region
                           (point-min) (point-max)
                           "ipe fmt --stdin" nil t)))
                      nil t)))
ELISP_SNIPPET

printf '================================================================\n'
printf '\n'
printf 'After pasting:\n'
printf '  M-x treesit-install-language-grammar RET ipe RET  (Emacs 29+)\n'
printf 'Open a directory containing package.ipe for cross-module analysis.\n'
