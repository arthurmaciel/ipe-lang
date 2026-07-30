# The Ipê language

The details of the Ipê language — the reference for readers who want to know how
the language itself works, chapter by chapter. For how to build an application,
see the [shape guides](../shapes/README.md); this book is about the language.

## Chapters

- [Strings](strings.md) — ordinary and triple-quoted strings, `{{expr}}`
  interpolation, and the escape grammar.
- [Errors: the `Ipe.Error` type](error-handling.md) — the typed,
  pattern-matchable `Error` at every `Task` boundary.
- [Capabilities](capabilities.md) — what a program is allowed to do, inferred
  from its code with nothing to declare.
- [Filesystem: `Ipe.Path` and `Ipe.File`](filesystem.md) — paths are a typed,
  traversal-checked value, not raw strings.
- [Views: Ui, Html, and Css](ui.md) — the `Ipe.Ui`, `Ipe.Html`, and `Ipe.Css`
  view vocabularies and how they intermix. Includes `Ipe.Markdown` — render
  markdown to `Element msg` (safe for untrusted input).
