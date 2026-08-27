# Documentation infrastructure (Option B): doc-strings as SSOT

Status: tbd / ratified design, not yet implemented.
Implementation is split into the lettered components below; a component is the
unit a single lane picks up.

All fenced code blocks below are **illustrative** — they sketch proposed syntax
and interfaces that do not yet exist. They are not runnable and are not doc-test
inputs; the doc-test gate applies only to real examples authored
once this infrastructure lands.

## Goal

One source of truth for every piece of Ipê documentation — symbol reference,
teaching prose, diagnostics, glossary — authored once, verified by compilation,
and rendered to several surfaces (terminal, Markdown files, an HTML site) that
cannot drift from the language or from each other.

A reader with no prior Ipê vocabulary can learn the whole language from the
generated site: every code example is real (it compiles and runs), every term
in a snippet links to its definition, and nothing sells the language — the prose
is neutral, concrete, and practical.

## Principles (binding)

- **SSOT, derive everything.** No content is written twice. Each fact lives in
  exactly one place; every rendering is generated from it; CI regenerates and
  diffs so a stale rendering reddens the build.
- **Verified by construction.** Every example is extracted, compiled, and run by
  a doc-test gate. A code change that breaks an example reddens CI. This is the
  SSOT rule's "assert equality in a test" applied to documentation.
- **No selling.** Present the language plainly; never advocate for it or use
  marketing language. Easy-to-read, concrete, practical.
- **One boundary per concern.** Highlighting and term-linking share one
  compiler-exposed mechanism (annotated tokens); no hand-rolled approximations.

## Architecture

```
                 ┌─────────────── per-kind SSOTs ───────────────┐
  .ipe doc-strings    diagnostic explain/*.md    content/*.md    command registry
  (symbols, types,    (132 pages, already SSOT)  (constructs,    (help.rs COMMANDS)
   veneers, examples)                             idioms,
                                                  glossary)
                 └───────────────────┬──────────────────────────┘
                                     │  (index only links + orders; never copies content)
                            ┌────────▼─────────┐
                            │   content index   │  crate: ipe_docs (new)
                            └────────┬─────────┘
             ┌───────────────────────┼───────────────────────┐
             ▼                       ▼                       ▼
      docs/*.md (+ internals)   `ipe doc <key>`        `ipe doc serve`
      generated, CI-diffed      terminal, rich          HTML site, highlighted
                                                         + term-linked
                                     ▲
                            ┌────────┴─────────┐
                            │ annotated tokens  │  (highlighting + linking SSOT)
                            │  compiler API     │  Rust lib + `ipe doc --tokens` JSON
                            └──────────────────┘
```

The content index is a pure aggregator: it references each per-kind SSOT and
imposes ordering and cross-links. It never stores prose.

## Components

### A. Doc-strings in `.ipe` source (parser + AST) — the symbol SSOT

Today the parser does not attach comments to declarations (no `doc_comment`
handling in `src/compiler/parse/`). This component adds it.

- **Syntax.** A doc-string is a block comment placed immediately above a
  top-level declaration (`fn`, type, port, veneer). Choose the Elm form
  `{-| … -}` (already lexed as a block comment) to avoid a new token; the
  leading `|` marks it as a doc-string. A doc-string attaches to the next
  declaration with no blank line between them.
- **AST/HIR.** Add an optional `doc: Option<DocString>` field to the top-level
  declaration node (`src/compiler/syntax/`), populated by the parser. `DocString`
  carries the raw markdown body and the spans of any fenced ipe example blocks
  (so the doc-test gate and renderers can find them).
- **Structure of a doc-string body.** Free markdown, but with recognized
  sections rendered specially: a one-line summary (first paragraph), a fenced
  ipe example block (required for every exported symbol — real, practical,
  never a toy `1 + 1`), and optional `Idiom:` / `See also:` lines. The type
  signature is NOT written in prose — it is derived (see B).
- **Acceptance.** Parsing a module with doc-strings round-trips; a doc-string
  above a non-exported binding is a warning (documented but unreachable);
  every exported symbol lacking a doc-string is a lint (opt-in first, gate
  later). No behavior change to emitted code (doc-strings are erased before
  lowering).

### B. Kernel veneers + derived type schemes — the SSOT for kernel-backed exports

Every kernel-backed stdlib export gets a thin `.ipe` veneer that is the single
source for its **type signature + doc-string + verified example**, delegating to
the native kernel. Illustrative shape of a veneer (proposed syntax):

```
{-| Return `True` when the `Maybe` holds a value.

    isJust (Just 3)   --> True
    isJust Nothing    --> False
-}
isJust : Maybe a -> Bool
isJust = @kernel Maybe.isJust
```

- **Compiles away.** The veneer lowers to a direct kernel call — no indirection,
  no wrapper frame (honors efficiency). This is the same relationship Elm has
  between `String.length : String -> Int` and `Elm.Kernel.String.length`.
- **Derive the scheme.** The kernel type-scheme table
  (`constrain.rs::stdlib_scheme` + `kernels::StdlibKernel::scheme`) is DERIVED
  from — or asserted byte-equal to — the veneer's written signature. This
  eliminates the drift class where a kernel's `d(module, name, arity, …)` and
  its `scheme()` are hand-kept in sync — the same class of defect as a kernel
  registered with no backing runtime fn. A test asserts, for every kernel, that
  veneer signature == registered scheme == runtime fn arity.
- **Acceptance.** For each existing kernel-backed module (`Maybe` first, then
  `List`, `String`, `Result`), the veneer is authored, the scheme is
  derived/asserted-equal, emitted code is byte-identical to today's (golden
  diff clean), and the runtime-symbol-resolution guard still holds.

### C. Annotated-tokens compiler API — the SSOT for highlighting AND linking

One mechanism serves both syntax highlighting and term-to-definition linking,
computed from the real lexer + resolver so neither can drift. Illustrative
Rust surface (proposed):

```
fn annotate(source, module_ctx) -> Vec<AnnotatedToken>
struct AnnotatedToken { span, class: TokenClass, def: Option<DefKey> }
```

- `TokenClass` is the syntactic/semantic category (keyword, type, type-var,
  function, kernel, constructor, variable, module, operator, string, number,
  comment, punctuation) — a superset of `semantic_tokens.rs`'s legend, which is
  re-expressed on top of this. `DefKey` is the resolved definition key for a
  name (`module::symbol`, kernel id, construct id) — `None` for non-names.
- **Reuse, don't reinvent.** `src/lsp/features/src/semantic_tokens.rs` already
  computes classes; the resolver already computes definitions. This component
  factors both into the shared `annotate` and re-implements `semantic_tokens`
  as a thin projection (class only) — proving no drift.
- **Tool-agnostic surface.** Expose (1) the Rust lib for in-process consumers
  (LSP, docs generator) and (2) a stable `ipe doc --tokens <file>` JSON output
  for external tools. The only tool-specific piece is mapping a `DefKey` to that
  tool's anchor (docs URL vs editor location vs code-review unit id) — a trivial
  key→URL function, not a drift class.
- **Acceptance.** `semantic_tokens` output is unchanged (LSP golden stable);
  `annotate` classifies a known corpus correctly including the cases a
  hand-rolled highlighter gets wrong (shadowed names, names inside strings,
  operator vs punctuation); `--tokens` JSON is schema-stable.
- **Deferred follow-up (already filed):** retire `tools/code-review`'s
  `Lib/Highlight.ipe` and `Lib/Links.ipe` onto this mechanism.

### D. Content index — the aggregator

New crate `ipe_docs`. Builds an in-memory index whose entries reference (never
copy) the per-kind SSOTs:

- symbols/modules → the parsed doc-strings from (A)/(B)
- diagnostics → `src/compiler/diagnostics/explain/*.md` (already SSOT, 132 pages)
- constructs / idioms / glossary → `docs/constructs/*.md` (new content files, the
  only newly-authored prose — for language constructs like `case`, `do`, and a
  glossary with etymology per the kind-teacher convention)
- CLI commands → the `help.rs` `COMMANDS` registry (already SSOT)

The index exposes `resolve(key) -> Entry` where a key is a symbol
(`List.map`), module, diagnostic (`IPE-L0107`), construct (`case`), or
idiom/topic. This is the single lookup that both `ipe doc` and the site use.

### E. `ipe doc` CLI — one verb, retire `ipe explain`

- `ipe doc <key>` resolves any entity via the index. **Rich by default**:
  signature (derived) + usage example + idioms + glossary + explanation, kept
  under a readable length threshold. `--plain` = terse (signature + example, for
  machines/terse humans). `--json` = machine-structured. `--tokens <file>` =
  the annotated-token stream from (C).
- **Remove `ipe explain`.** Fold its behavior into `ipe doc`. Update every
  diagnostic hint from "run `ipe explain <code>`" to "run `ipe doc <code>`"
  (the pointer text is itself derived from one constant). The `explain` entry in
  `help.rs` COMMANDS and `explain.rs` are deleted; `run_explain` becomes
  `run_doc`. The 132 explain pages stay where they are (they are the diagnostic
  SSOT); only the command surface changes.
- Rationale: shrink the CLI verb surface (one teaching/reference verb), one SSOT
  for docs/teaching/explanations. Help text and `ipe doc <command>` both derive
  from the COMMANDS registry.
- **Acceptance.** `ipe doc List.map`, `ipe doc IPE-L0107`, `ipe doc case`,
  `ipe doc do` all render; `ipe explain` is gone (invoking it prints the
  usage that points at `doc`); every diagnostic footer says `ipe doc <code>`;
  no dangling `explain` references in code or shipped docs.

### F. Doc-test gate

- Extract every fenced ipe example from every doc-string (A/B) and content file
  (D). For each, synthesize a minimal module, run `ipe` on it (and, where the
  example shows a result with `-->`, run the emitted program and assert the
  printed result matches). A failure reddens CI.
- Runs as a CI job gated on `code`/docs changes; also a local
  `ipe doc --check-examples`.
- **Acceptance.** The gate fails when an example is edited to be wrong and
  passes on the real corpus; it is deterministic and sharded like the existing
  e2e gate.

### G. `docs/*.md` generation

- Generate `docs/reference/stdlib.md` (index: modules → exported symbols with one-line
  summaries, cross-linked) and `docs/reference/stdlib/<Module>.md` (detailed:
  each symbol's signature + doc-string + example) from the index (D).
- A CI job regenerates and `git diff --exit-code`s them, so a doc-string edit
  that isn't reflected in the committed `.md` reddens the build (no drift).
- **Acceptance.** `docs/reference/stdlib.md` + internals are generated, committed, and
  diff-clean in CI; editing a doc-string and regenerating updates them.

### H. `ipe doc serve` — the HTML site

- Serves a complete, structured, hyperlinked site a reader can learn the whole
  language from: getting-started → concepts (constructs) → per-module reference
  → idioms → glossary, all cross-linked.
- **Every code snippet is highlighted** (via C's annotated tokens → CSS classes)
  and **every term links to its definition** (via C's `DefKey` → docs URL). A
  reader needs no prior vocabulary: click any name to reach its page.
- Static generation preferred (emit an HTML tree + a tiny local server for
  `serve`), so the same artifact can be published. No client-side framework;
  plain semantic HTML + a small stylesheet. Neutral visual design, no marketing.
- **Acceptance.** `ipe doc serve` opens a browsable site; a snippet's `List.map`
  links to the `List.map` page; highlighting matches the LSP classification
  (same SSOT); the site builds from the index with no hand-written HTML content
  pages (only templates).

## Implementation order and dependencies

- **A (doc-strings)** and **C (annotated tokens)** have no dependency on each
  other and can be built concurrently — the two natural first lanes.
- **B (veneers)** depends on A.
- **D + E (index and `ipe doc` terminal)** depend on A and B, and consume C for
  any rendered snippets.
- **F (doc-test gate)** depends on A.
- **G (`docs/*.md`)** depends on D.
- **H (`ipe doc serve`)** depends on C, D, and G.

Each component ships behind its own gate and leaves the tree green; none changes
emitted code.

## Migration / retirement

- `ipe explain` → `ipe doc`; no back-compat alias (small CLI
  surface is the goal). Diagnostic footer text derives from one constant.
- `tools/code-review` `Highlight.ipe` + `Links.ipe` → the annotated-tokens
  mechanism (C). Filed deferred; lands once C ships. Do not author a new
  highlighter/linker or depend on the hand-rolled ones.

## Non-goals

- No new markup language; doc-strings are markdown.
- No client-side JS framework in the site.
- No marketing/advocacy copy anywhere.
- No change to emitted code from doc-strings or veneers (both compile away;
  golden diffs stay clean).
