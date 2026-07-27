# `ipe doc` — API documentation generation

Status: design proposal, no implementation yet.

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
the same split, and — because Completeness is a first-class value — ships all
three renderings in the first cut rather than deferring the reader-facing one:

1. **`docs.json`** — the machine-readable source of truth (one record per
   exposed module). Stable, versioned schema so a package index, LSP hover, and
   the curated GitHub index can all consume it without re-parsing source. Every
   other rendering is a pure view over this JSON.
2. **Markdown** — one file per module, readable in a repo and on GitHub.
3. **HTML** — a self-contained static site (an index page + one page per module,
   plus CSS), openable from the filesystem with no server. This is the primary
   reader-facing surface, not a later increment.

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
- **Resolved names** — the canonicaliser's resolved identity for every type and
  value mentioned in a signature or doc-comment (module + name), so a reference
  can be linked to its definition without heuristic text matching.

## Cross-module references and links

Every documented entry gets a **stable anchor** derived from its module + name
(e.g. `Ipe.String#toUpper`), identical across all three renderings so a
`docs.json` consumer, a Markdown reader, and the HTML site agree on one address
per entry.

A **cross-reference** is any type or value name appearing in a rendered
signature (or a doc-comment's `` `backticked` `` code span) that resolves — via
the canonicaliser's already-computed resolved names, never a text guess — to an
entry documented in this package. Each such reference becomes a link to that
entry's anchor:

- A type in a signature (`toInt : String -> Maybe Int`) links each named type
  (`String`, `Maybe`, `Int`) to its definition when that definition is in the
  package; built-ins with no in-package definition render as plain text.
- A reference to a definition in **another module** of the same package links
  across module pages (the "cross-module reference" case).
- In HTML the link is an `<a href>`; in Markdown a relative link + `#anchor`; in
  `docs.json` a structured `{ module, name }` reference the consumer resolves.

Navigation the HTML site provides: an **index page** listing every module;
per-module a table of contents of its exposed entries; and each entry's
**jump-to-source** link (file + line). A reference to a definition outside the
package (another package, or the stdlib when documenting a user package) renders
as plain text in the first cut — inter-*package* linking needs the package index
and is tracked separately.

## Command surface

Illustrative only — the design's target shape, not yet implemented (nothing to
run). `ipe doc` has three **mutually-exclusive modes**, so the raw flags are
parsed into a closed value at the CLI boundary — an invalid combination is not
representable downstream (make-invalid-states-unrepresentable, parse don't
validate):

    DocMode = Generate { out : Path, format : Format }   -- write the docs to disk
            | Serve    { port : Maybe Port }             -- build the HTML, preview it locally
            | Check                                       -- verify coverage, produce nothing

There is no `Serve { format }` or `Check { port }` to construct, so no code past
the parser can hold an invalid mix. Each mode is surfaced as its own subcommand
carrying ONLY its valid flags (Elm-style — `elm make` / `elm reactor`):

```
ipe doc [PATH] [--out DIR] [--format markdown|json|html|all]   -- generate (bare `ipe doc`)
ipe doc serve [PATH] [--port N]                                 -- local preview
ipe doc check [PATH]                                            -- coverage gate
```

- `PATH` — a package directory (default `.`) or a single `.ipe` module (every mode).
- **generate** (the bare `ipe doc`): writes `docs.json` plus the `--format`
  renderings to `--out` (default `doc/`). `--format` is `all` (default:
  `docs.json` + Markdown + HTML) or one of `markdown` / `json` / `html`.
- **serve**: builds the HTML site and serves it read-only on `http://127.0.0.1`,
  opening the browser. The port defaults to an **auto-selected free one** — bind
  `127.0.0.1:0`, then report and open the port the OS assigned, so it never fails
  on a busy fixed port; `--port N` pins one and errors if it is taken. Serving is
  always HTML and persists nothing, so `--format` and `--out` do not apply — to
  keep files, run generate.
- **check**: exits non-zero if any exposed binding lacks a doc-comment — a
  pass/fail CI gate that writes nothing and serves nothing, so it takes neither
  `--out`/`--format` nor `--port`.

Why modes and not one flag bag: `--port` is meaningless without a server,
`--format`/`--out` are meaningless when serving HTML or checking, and `--check`
produces nothing to serve. Splitting the modes into subcommands makes
`ipe doc check --port 8080` and `ipe doc serve --format json` *unwriteable*, not
merely rejected after the fact — the invalid state has no representation, so no
runtime check has to defend against it.

Honest-surface still holds — the command ships only what works — but the target
for the first cut now includes HTML, cross-module references, and links, not a
deferred increment. What remains explicitly out of the first cut: inter-*package*
linking (needs the package index), full-text search, and hosting.

## Boundaries

- The HTML is static and self-contained (relative links, bundled CSS), openable
  via `file://` or previewed with `--serve`. `--serve` binds loopback only
  (`127.0.0.1`) and serves the already-built static files read-only — no writes,
  no external interface. *Remote* publishing/hosting stays out of scope (a later
  `ipe package` concern); `--serve` is a local preview of that static output, not
  a publish path.
- Signatures come from the checker, so `ipe doc` runs the front end (parse →
  canon → typecheck) but never the emit tier — it needs types, not code.
- `docs.json` schema is versioned from the first release so downstream consumers
  can rely on it; the anchor scheme is part of that contract.

## First cut

Single package → `docs.json` + per-module Markdown + a self-contained HTML site,
from the exposing list, `-- |` comments, checker signatures, source locations,
and canonicaliser-resolved cross-references (intra- and inter-module links +
stable per-entry anchors), plus `--check`. Deferred and tracked separately:
inter-package linking, search, and hosting.
