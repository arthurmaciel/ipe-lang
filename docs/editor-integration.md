# Editor integration (LSP)

`ipe lsp` speaks JSON-RPC over stdio and works with any LSP-compliant editor.
Features: type-directed completion, go-to-definition, find-references, rename,
formatting, range formatting, code actions, semantic tokens, signature help,
and inlay hints.

Completion is type-directed: where the surrounding context expects a specific
type (a function argument, a typed binding's body, an `if`/`case` branch, a
list element), candidates whose type matches are offered first and the expected
type's own constructors are surfaced — an `Int` slot never offers a `String`.
Away from such a context it falls back to every in-scope name. Every suggestion
comes from the same type-checker `ipe build` runs, so a completion the editor
offers is one the compiler accepts.

## Helix

Add to `~/.config/helix/languages.toml`:

```toml
[[language]]
name = "ipe"
scope = "source.ipe"
file-types = ["ipe"]
roots = ["ipe.toml"]
language-servers = ["ipe-lsp"]
auto-format = true
formatter = { command = "ipe", args = ["fmt", "--stdin"] }

[language-server.ipe-lsp]
command = "ipe"
args = ["lsp"]
```

## Neovim (with `nvim-lspconfig`)

```lua
local lspconfig = require("lspconfig")
local configs = require("lspconfig.configs")

if not configs.ipe then
  configs.ipe = {
    default_config = {
      cmd = { "ipe", "lsp" },
      filetypes = { "ipe" },
      root_dir = lspconfig.util.root_pattern("ipe.toml", ".git"),
      settings = {},
    },
  }
end

lspconfig.ipe.setup({})
```

Add the filetype detection if needed:

```lua
vim.filetype.add({ extension = { ipe = "ipe" } })
```

To enable format-on-save, install [`conform.nvim`](https://github.com/stevearc/conform.nvim) and add:

```lua
require("conform").setup({
  formatters_by_ft = {
    ipe = { "ipe_fmt" },
  },
})

require("conform").formatters.ipe_fmt = {
  command = "ipe",
  args = { "fmt", "--stdin" },
  stdin = true,
}
```

Or without a plugin, set `formatprg` in a filetype config:

```lua
vim.api.nvim_create_autocmd("FileType", {
  pattern = "ipe",
  callback = function()
    vim.bo.formatprg = "ipe fmt --stdin"
  end,
})
```

## VS Code

Install the [Ipê extension](https://marketplace.visualstudio.com/items?itemName=arthurmaciel.ipe-lang)
(bundles the LSP client), or configure it manually in `.vscode/settings.json`:

```json
{
  "ipe.languageServer.command": "ipe",
  "ipe.languageServer.args": ["lsp"]
}
```

If you prefer a generic LSP client (e.g. `vscode-languageclient`), register:

```json
{
  "[ipe]": {},
  "languageServerExample.trace.server": "verbose"
}
```

and point `command` to `ipe lsp` for `.ipe` files.

To enable format-on-save via the generic LSP client, add to
`.vscode/settings.json`:

```json
{
  "[ipe]": {
    "editor.defaultFormatter": "arthurmaciel.ipe-lang",
    "editor.formatOnSave": true
  }
}
```

The bundled extension handles the `ipe fmt --stdin` plumbing automatically.

## Emacs

### lsp-mode

Add the following to your `init.el` (requires [`lsp-mode`](https://github.com/emacs-lsp/lsp-mode)):

```elisp
(use-package lsp-mode
  :ensure t
  :hook ((ipe-mode . lsp-deferred))
  :commands lsp
  :config
  (lsp-register-client
   (make-lsp-client :new-connection (lsp-stdio-connection '("ipe" "lsp"))
                    :major-modes '(ipe-mode)
                    :server-id 'ipe-lsp)))
```

Add a basic major mode for `.ipe` files (or install `ipe-mode` from MELPA if
available):

```elisp
(define-derived-mode ipe-mode prog-mode "Ipê"
  :group 'languages
  (setq tab-width 4)
  (setq format-prg "ipe fmt --stdin")
  (font-lock-fontify-buffer))

(add-to-list 'auto-mode-alist '("\\.ipe\\'" . ipe-mode))
```

`M-x indent-buffer` (`gq` in visual state) will now use `ipe fmt --stdin`.
For format-on-save, add:

```elisp
(add-hook 'ipe-mode-hook (lambda () (add-hook 'before-save-hook #'indent-buffer nil t)))
```

### Doom Emacs

Enable the `lsp` module in `init.el`:

```elisp
;; init.el
(:completion company)   ; or corso / vertico — your choice
(:checkers syntax)
(:tools lsp)
```

Then add the Ipê client in `config.el`:

```elisp
;; config.el
(use-package! ipe-mode
  :mode "\\.ipe\\'"
  :config
  (after! lsp-mode
    (lsp-register-client
     (make-lsp-client :new-connection (lsp-stdio-connection '("ipe" "lsp"))
                      :major-modes '(ipe-mode)
                      :server-id 'ipe-lsp
                      :activation-fn (lsp-activate-on 'major-mode)))))

(add-hook! 'ipe-mode-hook #'lsp-deferred)
```

To enable format-on-save, install [`apheleia`](https://github.com/radian-software/apheleia) and register the formatter:

```elisp
(setf (alist-get 'ipe-mode apheleia-mode-alist) '(ipe-fmt))

(setq-hook! 'ipe-mode-hook apheleia-formatter '(ipe-fmt))

(define-formatter ipe-fmt
  :command ("ipe" "fmt" "--stdin")
  :stdin t
  :stdout t)
```

## Zed

Add a custom language server entry to `~/.config/zed/settings.json` (or
open the settings panel and edit the JSON directly):

```json
{
  "languages": {
    "Ipê": {
      "path_separators": "/",
      "matcher": {
        "filename": "\\.ipe$"
      },
      "autoclose_before": "}] \")\n\t",
      "brackets": [
        { "start": "{", "end": "}", "close": true, "newline": true },
        { "start": "[", "end": "]", "close": true, "newline": true },
        { "start": "(", "end": ")", "close": true, "newline": false }
      ],
      "line_comments": ["-- "],
      "block_comment": ["{- ", " -}"]
    }
  },
  "language_servers": ["ipe-lsp"],
  "language_server_settings": {
    "ipe-lsp": {
      "binary": {
        "path": "ipe",
        "arguments": ["lsp"]
      }
    }
  },
  "auto_formatter": true,
  "format_on_save": "on"
}
```

To use the external formatter directly (instead of the LSP's `formatOnSave`),
add to the language entry:

```json
"formatter": {
  "external": {
    "command": "ipe",
    "arguments": ["fmt", "--stdin"]
  }
}
```

> **Note:** Zed's custom language server support is evolving. If the above
> does not work for your version, open a project containing an `ipe.toml` and
> use the command palette (`Cmd+Shift+P` / `Ctrl+Shift+P`) → *Add Language
> Server* to register `ipe lsp` interactively.
