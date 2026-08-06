# `examples/sky/ipe-edits/`

Per-example semantic-delta edits, applied by `tools/scripts/lib/mirror.sh` (via
`tools/scripts/lib/apply-ipe-edits.py`) AFTER the shared `rename-map.tsv` token
rewrite, to produce the `examples/sky/ipe/<name>/` port. One optional
`<name>.edits` file per example.

An edits file lives here ONLY when the shared token rewrite cannot produce a
buildable-and-runnable Ipê port on its own — a Go-FFI reimplementation, a
stricter Ipê type signature, an env-var rename. Each edit is a `find`/`replace`
anchored on the **exact source text, never a line number**, so it survives the
line shifts upstream makes between releases. An edit whose `find` text no longer
occurs fails loud (a RED sweep row), never silently ignored.

## File format

```
# Any leading '#' lines are the rationale for the whole file.

[[edit]]
file: src/Main.ipe
find:
"""
<exact text to find — verbatim, may span multiple lines>
"""
replace:
"""
<replacement text — verbatim>
"""
```

- **`find` must occur exactly once** in `file` (zero or many ⇒ error).
- **`all: true`** before `find:` relaxes that to "every occurrence" (≥ 1
  required) — a file-scoped identifier rename, e.g. an env-var name.
- **Omit the whole `find:` block** ⇒ `replace` overwrites the file entirely, for
  a total port (e.g. a Go-FFI example reimplemented on `Ipe.Http.Server`).
- A fence is a lone triple-double-quote line; content between fences is verbatim.

The applier is `tools/scripts/lib/apply-ipe-edits.py <name>.edits <example-dir>`.
