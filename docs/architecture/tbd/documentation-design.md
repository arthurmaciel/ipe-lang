# Reader-facing documentation: design

The design for how Ipê is documented for its readers: the learning path, the
`ipe doc` command and `ipe doc serve` site, the authoring format for source
doc-strings and prose pages, and the style rules that keep it all neutral and
easy to read.

This document defines the reader-facing model and the content format the
generator consumes. The generator itself, the doctest gate, and `ipe doc serve`
are built separately; this document is what they are built *to*.

## Principles this design serves

- **Single source of truth.** The usage example for every exported function,
  type, and value is authored exactly once — in that symbol's source
  doc-string. The Markdown files, the `ipe doc` terminal output, and the served
  HTML site are all generated from it. Nothing is hand-synced.
- **Every example is real.** An example that appears in the documentation
  compiles and runs. Unverified code is not documentation; it is a claim. The
  doctest gate turns every example block into a test.
- **A newcomer needs no prior vocabulary.** In the served site, every
  identifier in every snippet links to its own definition, so a reader who does
  not yet know what `andThen` or `SqlValue` is can reach the answer in one
  click.
- **Neutral presentation.** The documentation describes what the language does
  and how to use it. It does not sell, rank the language against others, or
  editorialise. The reader decides whether they like it.

## 1. Structure and learning path

The documentation is one connected path from "I have never seen Ipê" to "I can
find the exact function I need". It has five layers, each linking down to the
next and up to the previous.

1. **Getting started** (`docs/guide/getting-started.md`) — install, create a
   project, run it, read the output. One page, one working program, no
   forward references the reader cannot resolve. Ends by pointing at the core
   concepts.
2. **Core concepts** (`docs/guide/*.md`) — the handful of ideas the rest of the
   language assumes: pure functions and immutability, types and type
   inference, `Maybe`/`Result` for absence and failure, and The Elm
   Architecture (TEA) for stateful programs. Each concept page is prose with
   short, verified snippets, and every term links into the glossary or the
   module reference.
3. **Per-module reference** — generated from source doc-strings, one page per
   `Ipe.*` module. Each page lists the module's exported types and functions
   with their checker-inferred signatures, description, verified example, and
   cross-links. This is the bulk of the documentation and is never
   hand-written: it is the rendered form of the `.ipe` doc-strings.
4. **Idioms** (`docs/guide/idioms/*.md`, or an `Idioms` section on a module
   page) — short, task-shaped recipes that cross more than one module ("read a
   config file and decode it", "query a table and render the rows"). An idiom
   is prose plus one verified program; it links to every module it touches.
5. **Glossary** (`docs/guide/glossary.md`) — one entry per term of art
   (`Task`, `kernel`, `pure function`, `The Elm Architecture`, `capability`),
   each a short definition with an etymology note where it clarifies, linking
   to the concept page or module that develops it.

### How the layers cross-link

- A **concept page** links each function it names to that function's reference
  entry, and each term of art to its glossary entry.
- A **reference entry** links each type in its signature to that type's
  definition (already implemented — see §3), and its "See also" line to related
  modules.
- A **glossary entry** links to the one concept page or module that develops
  the term, so the glossary is an index, not a second explanation.
- **Getting started** links forward only to concepts; it never sends a
  newcomer into the reference before they have the vocabulary to read it.

The reader can enter at any layer (a search result, a diagnostic link, an
external link) and always has a link back up to the context that explains it.

## 2. The `ipe doc` command

One verb, three levels of verbosity. There is no separate `explain` command;
diagnostics link `ipe doc <code>`.

```
ipe doc <key>            # rich: signature + description + example + see-also, capped
ipe doc <key> --plain    # terse: signature (and one-line summary), no prose
ipe doc <key> --json     # machine: the structured record
```

`<key>` resolves a module (`ipe doc Ipe.List`), a qualified symbol
(`ipe doc Ipe.List.filterMap`), or a diagnostic code (`ipe doc IPE-T0014`).

**Rich is the default** and shows, for a symbol: its signature, its
description, its first example block, and its "See also" line. For a module:
the module description, a grouped list of its exports (one-line summaries), and
the module-level example. Rich output is **capped at a readable length**: if the
full entry would exceed the threshold (see §4), the rich view prints the
signature, description, and first example, then a line pointing at
`ipe doc serve` or the module page for the rest. The cap keeps a terminal
invocation scannable; the site is where exhaustive detail lives.

**`--plain`** is the terse form for scripting and quick lookup: signatures and
one-line summaries, flush-left, no example blocks.

**`--json`** is the stable machine record (the same schema the generator emits),
for editors and downstream tools.

> Open item (parser/generator boundary): today `ipe doc <MODULE>` and
> `ipe doc <MODULE> --json` render only signatures — they do not attach the
> per-symbol doc-comments (verified: the `comment` field is empty on that
> path). The full generator path (`ipe doc . --write-format json`) *does* carry
> them for compiled-source modules. Rich `ipe doc <key>` as specified here
> requires the single-symbol path to read the same doc-comments the generator
> reads. This is a generator change, noted in §6.

## 3. The `ipe doc serve` site

`ipe doc serve` builds and previews a self-contained HTML site (loopback only)
that a reader can learn the whole language from. It is a pure view over the
generated `docs.json`; it invents no content.

### Layout

- **Left:** persistent navigation — the five layers as top-level sections
  (Getting started, Concepts, Modules, Idioms, Glossary), the Modules section
  expanding to the `Ipe.*` tree. The current page's position is highlighted.
- **Centre:** the page. A module page is: module description, then a grouped
  export list (Types, then Values, grouped by family as the module doc-string
  declares), each export rendered as signature + description + example +
  see-also. A concept or idiom page is rendered Markdown.
- **Right (on reference pages):** an on-page table of contents — the export
  names — for jumping within a long module.

### Term linking (the core mechanism)

Every identifier in every rendered code snippet is a link to its definition.
This is what lets a newcomer read a snippet with no prior vocabulary.

Resolution is by **name against the generated model**, not a text guess:

- **Type names** in a signature already resolve via the canonicaliser's
  computed type identity to a stable anchor (`Module#Name`); this is
  implemented today and is identical across JSON, Markdown, and HTML. A
  built-in with no in-package definition (`Int`, `String`) renders as plain
  text — never a dangling link.
- **Function and value identifiers** in an example block resolve by
  (module-qualifier, name). An example that writes `List.filterMap` or
  `Store.fromColumns` links the identifier to that export's anchor. An
  unqualified call to an auto-imported builtin (`modBy`, `compare`) links to
  its builtin's entry; an unqualified local (a `let`-bound name, a lambda
  parameter) is not a link.
- **Constructors** (`Just`, `Nothing`, `Ok`, `SqlString`) link to the type that
  declares them.
- **Keywords and constructs** (`case … of`, `do`, `type alias`) link to the
  concept or language page that defines them.

Concretely, for the module-level `Ipe.List` example:

```
    import Ipe.List as List

    List.filter (\n -> modBy 2 n == 0) [ 1, 2, 3, 4 ]
        |> List.map (\n -> n * 2)
    -- == [ 4, 8 ]
```

- `Ipe.List` / `List` → the `Ipe.List` module page.
- `List.filter` → `Ipe.List#filter`. `List.map` → `Ipe.List#map`.
- `modBy` → the builtin arithmetic entry.
- `|>` → the pipe-operator entry on the language page.
- `n` (lambda parameter) and the literals are not links.

The linker works from the tokens the generator already has per example (the
example is source text; the module import aliases in scope are known from the
snippet's `import` lines or the enclosing module). Where an identifier cannot be
resolved to a documented anchor, it renders as plain text, never a broken link —
the same fail-closed rule the type linker already follows.

### Search

A client-side index over: module names, export names, glossary terms, and
concept/idiom page titles. A query ranks exact export-name matches first, then
module and glossary matches, then full-text over descriptions. Selecting a
result lands on the export's anchor. The index is generated from `docs.json`, so
it never drifts from the reference.

### "Beautiful and complete enough to learn the language", concretely

- **Complete:** every exported symbol of every `Ipe.*` module has a real,
  verified example — including kernel-backed symbols, which carry their
  doc-string on a thin `.ipe` veneer (§6). No entry reads "no example".
- **Navigable:** at most two clicks from any page to any other — the left nav
  reaches every module; the term links reach every definition a snippet uses.
- **Legible:** monospace code with syntax colouring, comfortable measure for
  prose, visible focus states, and a legible type scale. Layout stays usable at
  narrow widths and in both light and dark. Colours come from the existing
  `Ipe.Palette` single source of truth, not ad-hoc values.
- **Self-contained:** the served bundle needs no network — fonts, styles, and
  the search index ship with it, so it works offline and from a checkout.

## 4. Authoring format

### Symbol doc-strings (the SSOT)

A symbol's documentation is its `-- |` doc-comment in the `.ipe` source,
immediately above the binding. The lexer discards comments before the AST
exists, so the generator recovers doc-comments by a source scan that attaches a
block to the binding it precedes; a plain `--` line continues an open block. The
format below is a **convention inside that one Markdown block** — it needs no
new parser: the block is Markdown, and the sections are recognised by simple
leading markers.

The shape of an export's doc-string:

```
-- | `name arg1 arg2` — one-sentence summary of what it returns, in terms of
-- its arguments. Then one or two sentences of what it is for and when to reach
-- for it rather than a neighbour. Reference other symbols in `backticks`.
--
-- Example:
--
--     name someArg otherArg == expectedResult
--     name edgeCaseArg == expectedEdgeResult
--
-- See also: `neighbourOne`, `neighbourTwo`.
```

Rules:

- **First line** starts with the symbol applied to named arguments in
  backticks, an em-dash, then the summary. This is what `--plain` and the
  export list show.
- **`Example:`** introduces one or more example blocks — each an indented
  (four-space) code block, the Markdown code-block form the scanner and every
  renderer already handle. Each line is a complete, runnable expression,
  written as an **equality** (`expr == result`) so the doctest gate can check
  it directly. An example that needs a definition in scope (a helper, a type)
  includes it in the block. An example that must run a `Task` (a database read,
  a file write) is shown as a short program; the doctest gate compiles and runs
  it rather than evaluating an equality.
- **`See also:`** (optional) is a comma-separated list of related symbols or
  modules in backticks, rendered as the see-also line and links.
- **Backticked identifiers** anywhere in the block become term links in HTML and
  are left as plain backticks in the terminal.
- **Implementation notes stay out of the reader-facing block.** Rationale for a
  private helper, a tail-call note, an inference subtlety — anything a *reader*
  of the API does not need — goes in a plain `--` comment on the private helper
  or after the binding, never in the exported symbol's `-- |` block. (The
  exemplar modules in this change move existing implementation prose out of the
  reader-facing blocks accordingly.)

The module-level doc-string (the `-- |` above `module …`) follows the same shape
at module scale: a paragraph on what the module is for, a paragraph grouping its
exports into families, one example that shows the module in typical use, and a
`See also:` line to sibling modules.

### Kernel-backed symbols

A symbol whose implementation is a native kernel (much of `Ipe.List`,
`Ipe.Maybe`, `Ipe.String`) still gets a real doc-string: the `.ipe` source
carries a thin veneer — the export's signature and its `-- |` doc-string in the
exemplar format — even when the executable body is the kernel. The reference
entry then joins the kernel's checker-inferred signature to the source
doc-string by name. (The generator change in §6 is what makes this join happen
for the kernel-qualifier modules; the format authored here is already correct.)

### Prose pages (concepts, idioms, getting started, glossary)

These are Markdown content files under `docs/guide/`, not source doc-strings —
they are cross-module narrative, which no single symbol owns. They follow the
same style rules (§5) and the same example rule: every snippet in a prose page
is a verified program, drawn from or checked by the same doctest harness. A
concept page links each symbol it names to the reference and each term to the
glossary.

## 5. Style guide

- **Concrete and practical.** Say what the function returns and when to use it.
  Prefer a short verified example over a paragraph of description.
- **Neutral. No selling, no advocacy, no marketing.** Do not compare the
  language favourably to others, do not use "powerful", "elegant", "simply",
  "just", "blazing", or similar. State behaviour; let the reader judge.
- **Address the reader plainly.** "Returns `Nothing` when the list is empty",
  not "You'll be delighted to find it returns `Nothing`".
- **Define a term the first time it appears** on a page, or link it to the
  glossary. Assume no prior Ipê vocabulary.
- **No archaeology.** No dates, version numbers, issue references, phase names,
  or "was X, now Y" narration in reader-facing text (the ADR log is the one
  place history belongs).
- **One voice.** Present tense, active voice, second person for instructions
  ("create a file"), third person for behaviour ("`map` applies `f`").

### Length threshold for rich `ipe doc`

Rich `ipe doc <symbol>` prints signature + description + first example +
see-also. The **cap is 40 lines of rendered output** for a single symbol
(roughly one terminal screen). A module's rich view caps at **60 lines** — the
description, the grouped export list, and the module example — beyond which it
prints the summary and a pointer to the module page. These are rendered-line
caps, not source caps: a symbol may carry several example blocks in source (all
kept in the site and all doctested); the terminal shows the first and links the
rest. The thresholds are a single constant in the renderer, adjustable in one
place.

## 6. Generator work this format implies (for the implementer)

The content and format in this document are authored to the current scanner,
with two changes needed on the tool side:

1. **Attach doc-comments on the single-symbol / single-module path.** Rich
   `ipe doc <key>` needs the same doc-comment scan the full generator runs;
   today that path returns empty `comment` fields.
2. **Join kernel-qualifier module doc-comments.** `build_stdlib_docs` scans
   source doc-comments only for `COMPILED_STD_MODULES`. Kernel-qualifier
   modules (`Ipe.List`, `Ipe.Maybe`) get their signatures from the kernel type
   table with an empty comment. The fix is to also scan the embedded source of
   the kernel-backed modules and join those doc-comments to the kernel-derived
   signatures by name — so the doc-strings authored in `Ipe/List.ipe` and
   `Ipe/Maybe.ipe` reach the reference. (Verified: `Ipe.Db.Store`, a
   compiled-source module, already flows its per-symbol `Example:` blocks into
   `docs.json`; `Ipe.List`/`Ipe.Maybe` do not yet.)

Neither is a format change — the doc-strings in this change are already in the
final shape; these are the joins that surface them.

## 7. Rollout

This change establishes the model and the exemplars. The remaining `Ipe.*`
modules get the same treatment by copying the exemplar shape:

- **Exemplified here:** `Ipe.Maybe` and `Ipe.List` (pure-data, kernel-backed)
  and `Ipe.Db.Store` (real-world, compiled-source), plus the getting-started,
  core-concept, and glossary-seed pages.
- **Per remaining module:** author the module doc-string and each export's
  doc-string in the §4 format, write one verified example per export (a pure
  equality where possible, a short program where a `Task` is involved), run
  every example through the doctest harness, and add a `See also:` line.
- **Sequence:** the pure-data and formatting modules first (`String`, `Dict`,
  `Set`, `Result`, `Tuple`, `Char`, `Math`), then the effectful and
  domain modules (`Task`, `File`, `Http`, `Db`, `Codec`, the `Ui`/`Html`/`Css`
  view modules), then the specialised ones. Roughly 37 modules remain.
- **Effort:** a pure-data module is about half a day (author + verify every
  export). A large domain module with `Task`-driven examples is one to two
  days (each example is a small program to write and run). The concept and
  idiom pages are a few days total. The two generator joins in §6 are a small,
  separate task. The doctest gate — which turns every `Example:` block into a
  test so the corpus stays honest — is a prerequisite for treating the rollout
  as done.
```
