# Editor integration

## Quick start

Run one command to configure your editor automatically:

```bash
# Helix (full auto — appends config, fetches grammar, installs queries)
curl -fsSL https://raw.githubusercontent.com/arthurmaciel/ipe-lang/main/editors/helix/configure.sh | sh

# Zed (full auto — merges settings.json; grammar install via GUI, see output)
curl -fsSL https://raw.githubusercontent.com/arthurmaciel/ipe-lang/main/editors/zed/configure.sh | sh

# Neovim (installs query files; prints Lua snippet to paste into init.lua)
curl -fsSL https://raw.githubusercontent.com/arthurmaciel/ipe-lang/main/editors/neovim/configure.sh | sh

# Emacs (installs treesit query files; prints Elisp snippet to paste into init.el)
curl -fsSL https://raw.githubusercontent.com/arthurmaciel/ipe-lang/main/editors/emacs/configure.sh | sh
```

Each script is idempotent (safe to re-run), never overwrites existing config, and
backs up any file it edits. The Neovim and Emacs scripts do not touch config files
directly — Lua and Elisp config is code, so they print the exact snippet to paste.
See `editors/<editor>/configure.sh` for the full source.

---

Two complementary pieces make an editor understand Ipê:

- **`ipe lsp`** — semantics (completion, go-to-definition, rename, formatting,
  diagnostics). Works with any LSP-compliant editor.
- **`tree-sitter-ipe`** — syntax highlighting through the grammar in
  `editors/tree-sitter-ipe/`. VS Code highlights through its own TextMate
  grammar; every other editor highlights through tree-sitter. Each editor
  section below wires both.

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

## Syntax highlighting (tree-sitter)

The grammar lives at `editors/tree-sitter-ipe/` and ships the generated parser
plus `queries/{highlights,injections,locals,tags}.scm`. Build it once with the
tree-sitter CLI (installed through the Rust toolchain — no JS toolchain needed):

```bash
cargo install tree-sitter-cli
# Editor hosts load at most ABI 14; keep it pinned when regenerating the parser.
cd editors/tree-sitter-ipe && tree-sitter generate --abi 14
```

The per-editor sections below point each host at this directory (or a checkout
of it) and install the query files, alongside the LSP.

## Helix

**Quick start:** `curl -fsSL https://raw.githubusercontent.com/arthurmaciel/ipe-lang/main/editors/helix/configure.sh | sh`

The script appends the language + grammar block to `languages.toml`, fetches and builds the grammar, and installs the query files. Idempotent; backs up `languages.toml` before editing. Manual steps follow if you prefer.

Add to `~/.config/helix/languages.toml`:

```toml
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

# Point Helix at the tree-sitter grammar over git — no local checkout needed.
[[grammar]]
name = "ipe"
source = { git = "https://github.com/arthurmaciel/ipe-lang", rev = "main", subpath = "editors/tree-sitter-ipe" }
# Or build from a local checkout instead:
# source = { path = "/path/to/ipe-lang/editors/tree-sitter-ipe" }
```

Fetch and build the grammar, then install the queries into Helix's runtime.
Fetch the query files straight from the repo — no clone needed:

```bash
hx --grammar fetch
hx --grammar build

# Helix looks up highlight queries under runtime/queries/<lang>/.
mkdir -p ~/.config/helix/runtime/queries/ipe
base=https://raw.githubusercontent.com/arthurmaciel/ipe-lang/main/editors/tree-sitter-ipe/queries
for q in highlights injections locals tags textobjects indents; do
  curl -fsSL "$base/$q.scm" -o ~/.config/helix/runtime/queries/ipe/"$q".scm
done
```

Verify with `hx --health ipe` — the *Highlight*, *Textobject*, and *Indent* rows
should all read `✓` once the query files are installed. Those rows report that
the query *file* was found, not that the compiled grammar loaded: highlighting
also needs the `[[grammar]]` `source` pointed at the current grammar directory
and the parser built at ABI 14 (`hx --grammar build`).

Code actions surface on `<space>a` (Helix's default) with the cursor on a
diagnostic — add a missing type annotation, add a missing import, repoint a
wrong import, and more. Open the project directory (the folder holding
`package.ipe`), not a loose single file, so go-to-definition and the other
cross-module features have a project to resolve against — outside a project
they return nothing.

Diagnostics are pushed as you type, but Helix draws none of them in the buffer
by default — they appear only in the gutter, the statusline, and the
`:diagnostics` picker. To get inline squiggles, enable inline diagnostics in
`~/.config/helix/config.toml`:

```toml
[editor]
end-of-line-diagnostics = "hint"

[editor.inline-diagnostics]
cursor-line = "warning"
other-lines = "error"
```

Go-to-definition and type-definition resolve names declared **in your project**.
A standard-library name (`Io.println`, `List.map`, …) has no navigable source —
the stdlib is compiled into `ipe`, not checked out on disk — so it reports "No
definition found"; hover still shows its type.

## Neovim (with `nvim-lspconfig`)

**Quick start:** `curl -fsSL https://raw.githubusercontent.com/arthurmaciel/ipe-lang/main/editors/neovim/configure.sh | sh`

The script installs the tree-sitter query files and prints the Lua snippet to paste into `init.lua` (it does not edit your config directly — Lua config is code). Manual steps follow.

```lua
local lspconfig = require("lspconfig")
local configs = require("lspconfig.configs")

if not configs.ipe then
  configs.ipe = {
    default_config = {
      cmd = { "ipe", "lsp" },
      filetypes = { "ipe" },
      root_dir = lspconfig.util.root_pattern("package.ipe", ".git"),
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

### Highlighting with nvim-treesitter

Register the grammar as a custom parser, then install it and drop in the
queries:

```lua
local parsers = require("nvim-treesitter.parsers").get_parser_configs()

parsers.ipe = {
  install_info = {
    -- A local checkout of this repo's grammar directory…
    url = "/path/to/ipe-lang/editors/tree-sitter-ipe",
    -- …or fetch from git and point at the subdirectory:
    -- url = "https://github.com/arthurmaciel/ipe-lang",
    -- location = "editors/tree-sitter-ipe",
    files = { "src/parser.c", "src/scanner.c" },
    branch = "main",
  },
  filetype = "ipe",
}
```

Then `:TSInstall ipe`. Fetch the query files where nvim-treesitter looks them
up (`queries/ipe/` on the runtimepath) — straight from the repo, no clone:

```bash
mkdir -p ~/.config/nvim/queries/ipe
base=https://raw.githubusercontent.com/arthurmaciel/ipe-lang/main/editors/tree-sitter-ipe/queries
for q in highlights injections locals tags textobjects indents; do
  curl -fsSL "$base/$q.scm" -o ~/.config/nvim/queries/ipe/"$q".scm
done
```

Enable highlighting in the nvim-treesitter setup (`highlight = { enable = true }`)
and open a `.ipe` file; `:InspectTree` shows the parse.

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

Install the extension from `editors/vscode/ipe-lang-0.1.0.vsix`:

```bash
code --install-extension editors/vscode/ipe-lang-0.1.0.vsix
```

Or build from source:

```bash
cd editors/vscode && npm install && npm run compile && npm run package
code --install-extension ipe-lang-0.1.0.vsix
```

The extension bundles LSP client + formatter. Set as default in `.vscode/settings.json`:

```json
{
  "[ipe]": {
    "editor.defaultFormatter": "arthurmaciel.ipe-lang",
    "editor.formatOnSave": true
  }
}
```

Alternatively, configure formatter-only manually:

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

**Quick start:** `curl -fsSL https://raw.githubusercontent.com/arthurmaciel/ipe-lang/main/editors/emacs/configure.sh | sh`

The script installs the treesit query files and prints the Elisp snippet to paste into `init.el` (it does not edit your config directly — Elisp config is code). Manual steps follow.

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

### Highlighting with treesit (Emacs 29+)

Emacs 29+ has a built-in tree-sitter (`treesit`). Register the grammar source,
install it, and derive the major mode from `prog-mode` via `treesit`:

```elisp
;; Where Emacs fetches and compiles grammars from.
(add-to-list
 'treesit-language-source-alist
 '(ipe "https://github.com/arthurmaciel/ipe-lang"
       :source-dir "editors/tree-sitter-ipe/src"))
;; Then: M-x treesit-install-language-grammar RET ipe RET
;; (or `treesit-install-language-grammar` for each grammar you need).

(define-derived-mode ipe-mode prog-mode "Ipê"
  :group 'languages
  (setq tab-width 4)
  (when (treesit-ready-p 'ipe)
    (treesit-parser-create 'ipe)
    (treesit-major-mode-setup)))

(add-to-list 'auto-mode-alist '("\\.ipe\\'" . ipe-mode))
```

`treesit` reads highlight rules from the grammar's `queries/highlights.scm`;
copy the query files where your configuration expects them, or load them via
`treesit-font-lock-rules` if you maintain the faces yourself.

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

**Quick start:** `curl -fsSL https://raw.githubusercontent.com/arthurmaciel/ipe-lang/main/editors/zed/configure.sh | sh`

The script merges the Ipê language + LSP block into `settings.json` (idempotent; backs up before editing). Grammar/highlighting install requires the GUI — the script prints the one-step instruction. Manual steps follow.

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
> does not work for your version, open a project containing a `package.ipe` and
> use the command palette (`Cmd+Shift+P` / `Ctrl+Shift+P`) → *Add Language
> Server* to register `ipe lsp` interactively.

### Highlighting via a Zed extension

Zed highlights through a tree-sitter grammar packaged as an *extension*. A
ready-to-install skeleton lives at `editors/zed-ipe/`; it points Zed at this
repo's grammar and reuses the same queries. Its `extension.toml` declares the
grammar and language, and `languages/ipe/config.toml` sets the file match,
comments, and brackets:

```toml
# editors/zed-ipe/extension.toml
id = "ipe"
name = "Ipê"
version = "0.1.0"
schema_version = 1

[grammars.ipe]
repository = "https://github.com/arthurmaciel/ipe-lang"
# The commit/tag to build; update on grammar changes.
rev = "main"
path = "editors/tree-sitter-ipe"

[language_servers.ipe-lsp]
name = "Ipê LSP"
languages = ["Ipê"]
```

Install it as a dev extension: **Extensions** → **Install Dev Extension** →
select `editors/zed-ipe/`. Zed builds the grammar and loads the highlight
queries from the extension's `languages/ipe/` directory (copied from
`editors/tree-sitter-ipe/queries/`). Keep the `settings.json` LSP block above
for `ipe lsp`.
