# zed-ipe

A [Zed](https://zed.dev/) extension for Ipê: syntax highlighting via the
`tree-sitter-ipe` grammar plus the `ipe` LSP.

Install as a dev extension: **Extensions** → **Install Dev Extension** → select
this directory. Zed builds the grammar declared in `extension.toml` and loads
the language config and highlight queries from `languages/ipe/`.

The `.scm` query files under `languages/ipe/` are copies of the source-of-truth
queries in `../tree-sitter-ipe/queries/` (Zed requires them co-located with the
language). When the grammar's queries change, re-copy them:

```bash
cp ../tree-sitter-ipe/queries/{highlights,injections,locals}.scm languages/ipe/
```

For LSP setup and the full per-editor guide, see
`docs/topics/editor-integration.md` in the repository root.
