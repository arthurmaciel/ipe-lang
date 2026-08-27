# `ipe doc` — stdlib API documentation, per-format output, and Pages deployment

## Purpose

`ipe doc` generates reference documentation for an Ipê package. This document
describes three design properties of that command:

1. **Per-format output subfolders** — each rendering lands in its own
   subdirectory of the output base, keeping formats cleanly separated.
2. **Stdlib documentation without a project** — the command succeeds in any
   directory and documents all stdlib modules when no project is present.
3. **Hierarchical module index** — the module listing is a namespace tree,
   not a flat alphabetical list.
4. **GitHub Pages CI workflow** — the HTML rendering is deployed automatically
   to `arthurmaciel.github.io/ipe-lang/api/`.

## Output layout

The output base defaults to `doc/` and is overridden with `--out DIR`. Under
that base, each format writes into its own subdirectory. The directory tree
below is illustrative — it shows the shape produced by
`ipe doc --write-format all` run outside a project (stdlib-only output):

<!-- illustrative directory tree, not a shell command -->
```
doc/
  json/
    docs.json            (machine-readable source of truth)
  markdown/
    index.md             (namespace-tree index)
    Ipe-List.md          (per-module page; one per stdlib module)
    …
  html/
    index.html           (namespace-tree index + filter box)
    style.css            (bundled stylesheet; no network fetch required)
    Ipe-List.html        (per-module page; one per stdlib module)
    …
```

`docs.json` is the source of truth; Markdown and HTML are pure views over the
same in-memory model. Cross-links within each format are relative to that
format's subfolder (e.g. `Ipe-List.html` links to `Ipe-List-someType.html`
within `html/`). The logical anchor scheme (`Module#Name`) is
format-neutral and shared across all three renderings.

When only a subset of renderings is requested (`--write-format json`), only
the corresponding subfolder(s) are written. The `json/` subfolder is always
written since `docs.json` is the source of truth.

## Stdlib documentation without a project

`ipe doc --write-format <fmt>` succeeds in any working directory. When the
target path contains a recognisable project (source files loadable by
`read_tree`), project modules are documented alongside the stdlib. When no
project is reachable, the command falls back to stdlib-only documentation via
`build_stdlib_docs()` — the project extraction pipeline (`extract_tree` /
`read_tree`) is not invoked, so no error is emitted.

`ipe doc check` remains project-only and is unchanged by this property.

## Hierarchical module index

The module listing in both the HTML index and the Markdown index is a
namespace tree keyed on dotted module names:

- `Ipe.Db` appears at its level; `Ipe.Db.Codec` and `Ipe.Db.Store` nest under
  it.
- A prefix that is not itself a module (has children but no module at its
  exact dotted path) renders as a non-link header.
- Within each level, nodes are sorted alphabetically.
- The existing "Project modules" / "Standard library" sectioning is preserved;
  each section is its own tree.

In text renderings (Markdown, `ipe doc list --plain`), each namespace depth
adds exactly two spaces of indentation. In HTML, children are wrapped in a
nested `<ul>`.

The `ipe doc list` human output follows the same tree structure. The `--json`
output remains a flat sorted list (a machine consumer does its own grouping).

## GitHub Pages CI workflow

The workflow at `.github/workflows/docs-pages.yml` deploys the HTML
rendering to GitHub Pages on every push to `main` that touches source files,
`Cargo.toml`, `Cargo.lock`, or the workflow file itself. It can also be
triggered manually via `workflow_dispatch`.

The deployment sequence:
1. Build `ipe` in release mode.
2. Run `ipe doc --write-format html` in a scratch directory (no project
   present, so stdlib-only documentation is generated).
3. Copy `doc/html/` into `_site/api/`.
4. Write a minimal `_site/index.html` that redirects `/ipe-lang/` to `api/`.
5. Upload `_site/` as a Pages artifact and deploy it.

The result URL is `arthurmaciel.github.io/ipe-lang/api/`.

**One-time manual step**: repo Settings → Pages → Source must be set to
"GitHub Actions" before the first deployment succeeds.

## Invariants

- `ipe doc --write-format <fmt>` never errors solely because no project is
  present. The worst case is a stdlib-only result.
- `ipe doc check` is always project-only and always errors when no project is
  present (unchanged).
- The `--write-format` closed set (`json | markdown | html | all`) is
  enforced at the CLI boundary; an unknown value is rejected with a typed
  error, never silently ignored.
- Every name `ipe doc list` advertises is queryable via `ipe doc <name>` —
  the list-vs-query registry is a single source of truth (`stdlib_module_names`
  reconciled against `build_stdlib_docs`).
