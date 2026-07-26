# `ipe doc` — API documentation generation

Status: design proposal, pending review. No implementation yet.

## Purpose

Generate reference documentation for an Ipê package from its own source — the
public API a consumer sees, with each entry's type signature, doc-comment, and a
stable source location. This is the discoverability surface the stdlib modules
already anticipate ("so `ipe doc` shows a stable source location for every
entry"). It is distinct from `explain`, which teaches error codes and concepts;
`doc` describes *code*.

## Model: mirror Elm

Ipê follows Elm's design where one exists, and Elm's package docs are the
reference. Elm compiles a package to a `docs.json` — an array of module records,
each with its doc-comment and its exposed unions, aliases, and values (name +
type + comment). The rendered package page is a view over that JSON. Ipê adopts
the same split:

1. **`docs.json`** — the machine-readable source of truth (one record per
   exposed module). Stable schema so a future package index, LSP hover, and the
   curated GitHub index can all consume it without re-parsing source.
2. **A rendered view** — Markdown by default (readable in a repo and on GitHub);
   HTML is a later addition, not part of the first cut.

## Inputs (all already produced by the pipeline)

- **Public API** — the module's `exposing (...)` list. Only exposed items are
  documented; nothing internal leaks.
- **Doc-comments** — the `-- |` line-comment attached to a module header or a
  binding (the convention the stdlib already uses).
- **Type signatures** — taken from the type checker, not re-parsed, so the
  documented type is exactly the inferred/checked type (generics, records, and
  aliases render identically to compiler output).
- **Source location** — file + line of each exposed binding, for a
  jump-to-source link.

## Command surface

Proposed surface — illustrative only, not yet implemented (nothing to run):

```
ipe doc [PATH] [--out DIR] [--format markdown|json] [--check]
```

- `PATH` — a package directory (default `.`) or a single `.ipe` module.
- `--out` — output directory (default `doc/`); writes `docs.json` and, for
  Markdown, one file per module.
- `--format` — `markdown` (default) or `json` (emit only `docs.json`).
- `--check` — write nothing; exit non-zero if any exposed binding lacks a
  doc-comment. This is the CI-gateable honest-surface check: a package's public
  API is fully documented or the gate fails.

Following the honest-surface rule, the command ships only what works: the first
cut is `markdown` + `json` + `--check` over a single package. HTML output and
cross-package linking are separate, later increments and are not advertised
until implemented.

## Boundaries

- No network, no hosting — `ipe doc` only reads local source and writes local
  files. Publishing/serving is out of scope (a later `ipe package` concern).
- Signatures come from the checker, so `ipe doc` runs the front end (parse →
  canon → typecheck) but never the emit tier — it needs types, not code.
- `docs.json` schema is versioned from the first release so downstream consumers
  can rely on it.

## MVP cut

Single package → `docs.json` + per-module Markdown from the exposing list,
`-- |` comments, checker signatures, and source locations, plus `--check`.
Everything else (HTML, search, cross-links, hosting) is deferred and tracked
separately.
