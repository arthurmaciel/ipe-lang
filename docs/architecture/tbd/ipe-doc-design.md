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
run):

```
ipe doc [PATH] [--out DIR] [--format markdown|json|html|all] [--serve] [--port N] [--check]
```

- `PATH` — a package directory (default `.`) or a single `.ipe` module.
- `--out` — output directory (default `doc/`); writes `docs.json`, and per the
  format the Markdown files and/or the HTML site.
- `--format` — `all` (default: `docs.json` + Markdown + HTML), or one of
  `markdown` / `json` / `html`.
- `--serve` — build the HTML site and serve it locally at
  `http://127.0.0.1:<port>`, opening it in the browser. The port defaults to an
  **auto-selected free one**: bind `127.0.0.1:0`, let the OS assign an open port,
  then report and open exactly that one — so `ipe doc --serve` never fails on a
  busy fixed port. `--port N` pins a specific port instead (and errors if it is
  taken, rather than silently picking another). Loopback-only and read-only — a
  local preview convenience, never an external listener.
- `--check` — write nothing; exit non-zero if any exposed binding lacks a
  doc-comment. The CI-gateable honest-surface check: a package's public API is
  fully documented or the gate fails.

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
