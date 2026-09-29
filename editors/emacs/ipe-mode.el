;;; ipe-mode.el --- Major mode for the Ipê language -*- lexical-binding: t; -*-

;; URL: https://github.com/ipe-lang/compiler
;; Package-Requires: ((emacs "29.1"))
;; SPDX-License-Identifier: Apache-2.0

;;; Commentary:

;; Syntax highlighting, comments and indentation for `.ipe' files, plus the
;; `ipe lsp' language server through the built-in Eglot client: completion
;; (`completion-at-point'), go to definition (`xref-find-definitions', M-.),
;; code actions (`eglot-code-actions', C-c C-a), diagnostics and formatting.
;;
;; Installed and wired into your init file (or Doom's config.el) by
;; editors/emacs/configure.sh.

;;; Code:

(require 'cl-lib)
(require 'project)

(declare-function eglot-ensure "eglot" ())
(declare-function eglot-managed-p "eglot" ())
(declare-function eglot-format-buffer "eglot" ())
(declare-function eglot-code-actions "eglot" (beg &optional end action-kind interactive))
(defvar eglot-server-programs)

(defgroup ipe nil
  "Major mode for the Ipê language."
  :group 'languages
  :prefix "ipe-")

(defcustom ipe-lsp-command '("ipe" "lsp")
  "Command line that starts the Ipê language server."
  :type '(repeat string))

(defcustom ipe-auto-eglot t
  "When non-nil, `ipe-mode' starts Eglot (the `ipe lsp' client) automatically."
  :type 'boolean)

(defcustom ipe-format-on-save t
  "When non-nil, format the buffer through `ipe lsp' before saving."
  :type 'boolean)

(defconst ipe-keywords
  '("module" "import" "exposing" "as" "type" "alias" "foreign"
    "case" "of" "if" "then" "else" "let" "in" "do")
  "Reserved words of Ipê (the tree-sitter grammar's keyword set).")

(defvar ipe-mode-syntax-table
  (let ((st (make-syntax-table)))
    ;; `--' line comments and nestable `{- ... -}' block comments.
    (modify-syntax-entry ?\{ "(}1nb" st)
    (modify-syntax-entry ?\} "){4nb" st)
    (modify-syntax-entry ?- ". 123" st)
    (modify-syntax-entry ?\n ">" st)
    (modify-syntax-entry ?\" "\"" st)
    (modify-syntax-entry ?\\ "\\" st)
    (modify-syntax-entry ?_ "_" st)
    (modify-syntax-entry ?' "_" st)
    (dolist (c '(?+ ?* ?/ ?< ?> ?= ?| ?& ?! ?: ?. ?^ ?% ?$ ??))
      (modify-syntax-entry c "." st))
    st)
  "Syntax table for `ipe-mode'.")

(defconst ipe-font-lock-keywords
  `((,(regexp-opt ipe-keywords 'symbols) . font-lock-keyword-face)
    (,(regexp-opt '("True" "False") 'symbols) . font-lock-constant-face)
    ;; A top-level type annotation or definition names a function.
    ("^\\([a-z_][A-Za-z0-9_']*\\)\\_>[ \t]*:[^:]" 1 font-lock-function-name-face)
    ("^\\([a-z_][A-Za-z0-9_']*\\)\\_>[^=\n]*=" 1 font-lock-function-name-face)
    ;; Module paths, types and constructors all start upper-case.
    ("\\_<[A-Z][A-Za-z0-9_']*\\_>" . font-lock-type-face)
    ("'\\(?:\\\\.\\|[^'\\\\\n]\\)'" . font-lock-string-face)
    ("\\_<[0-9]+\\(?:\\.[0-9]+\\)?\\_>" . font-lock-constant-face))
  "Highlighting rules for `ipe-mode'.")

(defvar ipe-mode-map
  (let ((map (make-sparse-keymap)))
    (define-key map (kbd "C-c C-a") #'eglot-code-actions)
    (define-key map (kbd "C-c C-f") #'eglot-format-buffer)
    map)
  "Keymap for `ipe-mode'.")

(defun ipe-project-find (dir)
  "Return the Ipê package holding DIR (the folder with `package.ipe')."
  (let ((root (locate-dominating-file dir "package.ipe")))
    (and root (cons 'ipe-package root))))

(cl-defmethod project-root ((project (head ipe-package)))
  "The directory holding PROJECT's `package.ipe'."
  (cdr project))

(defun ipe--format-before-save ()
  "Format through `ipe lsp' when Eglot manages this buffer."
  (when (and ipe-format-on-save
             (fboundp 'eglot-managed-p)
             (eglot-managed-p))
    (eglot-format-buffer)))

;;;###autoload
(define-derived-mode ipe-mode prog-mode "Ipê"
  "Major mode for editing Ipê source files."
  :syntax-table ipe-mode-syntax-table
  (setq-local comment-start "-- ")
  (setq-local comment-start-skip "\\(?:--+\\|{-+\\)[ \t]*")
  (setq-local comment-end "")
  (setq-local font-lock-defaults '(ipe-font-lock-keywords))
  (setq-local indent-tabs-mode nil)
  (setq-local tab-width 4)
  (setq-local indent-line-function #'indent-relative)
  (add-hook 'before-save-hook #'ipe--format-before-save nil t)
  (when ipe-auto-eglot
    (eglot-ensure)))

;;;###autoload
(add-to-list 'auto-mode-alist '("\\.ipe\\'" . ipe-mode))

(add-hook 'project-find-functions #'ipe-project-find)

(defun ipe--lsp-contact (&optional _interactive)
  "The Eglot contact for `ipe-mode': `ipe-lsp-command', read at start-up."
  ipe-lsp-command)

(with-eval-after-load 'eglot
  (add-to-list 'eglot-server-programs '(ipe-mode . ipe--lsp-contact)))

(provide 'ipe-mode)

;;; ipe-mode.el ends here
