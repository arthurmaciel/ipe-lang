# tree-sitter-ipe

A [tree-sitter](https://tree-sitter.github.io/) grammar for
[Ipê](https://github.com/arthurmaciel/ipe-lang), giving Helix, Zed, Neovim,
Emacs, and any other tree-sitter host syntax highlighting for `.ipe` sources.
The compiler's own LSP (`ipe lsp`) stays the source of semantics; this grammar
is complementary — highlighting only.

## Source of truth

The grammar mirrors the compiler's hand-written parser (the SSOT), not a
separate specification:

- tokens, keywords, operators — `src/compiler/parse/src/lexer.rs`
- declaration / expression / pattern / type forms — `src/compiler/syntax/src/ast.rs`
- recursive-descent structure — `src/compiler/parse/src/parser.rs`
- indentation-significant layout — `src/compiler/parse/src/layout.rs`

Because highlighting never gates the compiler's accept/reject or emit, the
grammar accepts a superset of the layout rule where a same-line construct is
ambiguous. The drift gate keeps it honest: `scripts/parity-check.sh` parses
every `examples/**/*.ipe` and `src/stdlib/Ipe/**/*.ipe` and fails on any
`ERROR`/`MISSING` node.

## Layout

Ipê is indentation-significant. Rather than express column rules in the LR/GLR
grammar (which cannot bound a `repeat` block without column information), an
external scanner (`src/scanner.c`) tracks a stack of block-start columns —
exactly `layout.rs`'s rule — and emits three zero-width layout tokens the
grammar uses to delimit `do` / `let` / `case` blocks and the top-level
declaration list:

- `_block_open` — the next token opens an indented block (its column is pushed)
- `_block_line` — the next token starts a sibling at the current block column
- `_block_close` — the next token is dedented past the current block (pop)

## Build & test

```bash
# from this directory
tree-sitter generate --abi 14 # regenerate src/parser.c from grammar.js
tree-sitter test              # run test/corpus/*.txt unit tests
bash scripts/parity-check.sh  # parse the whole reference corpus, fail on ERROR
```

The generated parser (`src/parser.c`, `src/tree_sitter/`) is committed so
consumers build without regenerating. Always regenerate with `--abi 14`: editor
hosts (Helix, Neovim, Zed, Emacs treesit) compile this committed `parser.c`, and
their bundled tree-sitter loads at most ABI 14. A newer default ABI compiles but
fails to load at parse time — the buffer stays unhighlighted with no error.

## Queries

`queries/` carries the standard tree-sitter query files, with capture names
from the shared highlight vocabulary (so Helix, Zed, nvim-treesitter, and Emacs
treesit all light up consistently):

- `highlights.scm` — keywords, types/constructors, functions, module names,
  strings, numbers, characters, comments, operators.
- `locals.scm` — local scopes and definitions for scope-aware highlighting and
  rename.
- `tags.scm` — ctags-style symbol index for code navigation.
- `injections.scm` — language injections (currently none).

## Editor setup

See `docs/topics/editor-integration.md` in the repository root for per-editor
wiring (Helix, Zed, Neovim/nvim-treesitter, Emacs treesit) alongside the
existing LSP and formatter configuration.
