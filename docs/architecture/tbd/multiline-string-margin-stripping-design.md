# Multiline-string margin stripping — anchor at the first content column (#133)

> Backlog item #133 (Post-completion): "Multiline-string margin
> stripping (anchor = first string character's column). Departure —
> output-changing; records an oracle divergence per patch class."
> Spec+plan written 2026-07-10. Design-only; no code has changed.
>
> **One-line decision:** the lexer computes the anchor column A (the
> column of the first non-newline content character of a `"""…"""`
> string) and carries it on the token/AST node; the canonicaliser's
> existing interpolation-desugar walk strips up to A−1 leading
> whitespace characters from every physical line after the first,
> leaving source offsets (and therefore `{{expr}}` sub-spans) intact.
> An immediate newline after the opening `"""` is dropped. The rule is
> anchor-based (deterministic from the opening line's layout), not
> min-common-margin.

## Problem statement

Triple-quoted strings today carry their raw body verbatim — every
newline and every column of leading indentation lands in the value. So
the idiomatic-looking

```elm
migrate conn =
    Db.execRaw conn """CREATE TABLE todos (
        id INTEGER PRIMARY KEY,
        title TEXT NOT NULL
    )"""
```

produces a value whose continuation lines start with eight spaces of
*source* indentation the author never meant as *data*. Authors must
choose between ugly source (dedent the string body to column 1) and
polluted values. Kotlin (`trimMargin`/`trimIndent`), Java (JEP 378 text
blocks), Scala, and YAML all solved this; Elm has no multiline-string
indentation handling (it also barely has multiline strings), and the
reference Ipê preserves the body verbatim — so this is a deliberate,
output-changing Ipê departure.

### Current behaviour (both compilers, confirmed 2026-07-10)

- **Ipê lexer:** `src/compiler/parse/src/lexer.rs:478-509`
  (`lex_triple_string`) returns `Tok::TripleStr(String)` with the RAW
  body — no escape resolution, no margin handling. The token
  (`lexer.rs:116-128`) carries 1-based `line`/`col` of the opening `"`;
  `Span` (`src/compiler/diagnostics/src/span.rs:3-8`) is byte-offsets
  only, so column info exists **only at lex time**.
- **Ipê parser/AST:** `src/compiler/parse/src/parser.rs:1148` wraps the
  raw body into `Expr_::MultilineStr(String)`
  (`src/compiler/syntax/src/ast.rs:150-156`); the canonicaliser later
  desugars `{{expr}}` interpolation and `\{{` / `\\` escapes into a
  `++` chain, mirroring the reference's `desugarMultiline`.
- **Reference:** `upstream:src/Sky/Parse/String.hs:23-37` returns
  `MultiLine (T.unpack content)` — verbatim; `findTripleClose`
  (lines 176–188) does no processing. Zero margin stripping.
- **Corpus:** 19 upstream examples use `"""`, essentially all as
  SQL/DDL (`07-todo-cli/src/Main.ipe:84-88`, `08-notes-app` etc.) where
  the leaked indentation is semantically inert — which is why the
  departure is safe to take, and why it is *output-changing* (the
  bytes printed/stored change) without being *behaviour-breaking*.

## Decision — normative semantics

Let the **anchor** A be defined at lex time:

1. If the first character after the opening `"""` is not a newline,
   A = that character's column (= opening-quote `col` + 3, since the
   lexer counts one column per character).
2. If the first character after `"""` is a newline (the common
   "`"""` then body on the next line" style), that **leading newline is
   dropped from the value** (Java-text-block precedent), and A = the
   column of the first character of the *following* line's content —
   i.e. leading whitespace of that first content line is itself subject
   to the strip rule below with the anchor taken as the column of its
   first non-whitespace character. A body consisting only of that
   newline (`"""\n"""`) is the empty string.

Then, for **every physical line after the line containing the first
content character** (including the final line that ends at the closing
`"""`):

- remove leading whitespace characters (space or tab, one column each,
  matching the lexer's column accounting) until A−1 characters have
  been removed, a non-whitespace character is reached, or the line
  ends — whichever comes first.

Properties (these are the tests):

- **Deterministic from the opening line.** The value never depends on
  which continuation line happens to be least-indented, so editing one
  line never silently changes the margin of the others (this is the
  reason min-common-margin was rejected — see Alternatives).
- **Relative indentation beyond the anchor is preserved.** Lines
  indented deeper than A keep the excess — nested structure survives.
- **Under-indented lines are preserved from their first non-blank
  character** (lenient: strip what's there, never error). A verbatim
  escape hatch therefore exists for free: content that must keep
  absolute leading whitespace can be indented beyond the anchor or the
  string can open with its content on the `"""` line at column 1.
- **Blank lines** become (or stay) empty — stripping stops at
  end-of-line.
- **Escapes and interpolation are untouched**: stripping operates on
  the raw body's physical lines; `\{{`, `\\` and `{{expr}}` handling
  stay in the canonicaliser exactly as today, applied to the
  post-strip literal segments.

### Alternatives considered and rejected

1. **Min-common-margin (Kotlin `trimIndent` / Java text blocks).**
   Rejected: the value depends on the least-indented non-blank line, so
   an edit to line 7 can change the meaning of lines 2–6; the
   anchor rule is locally readable at the opening delimiter. (The
   backlog row also already fixed the anchor rule; this records why it
   is also the better rule.)
2. **Explicit marker (`|`-margin à la Kotlin `trimMargin`).** Rejected:
   new syntax + line noise on every line; anchor achieves the common
   case with zero ceremony.
3. **Library function (`String.trimIndent`) instead of language
   semantics.** Rejected: runs at runtime on every evaluation, can't be
   constant-folded through interpolation seams, and the corpus shows
   the *default* is what's wrong — opt-in fixes nobody's SQL.
4. **Erroring on under-indented lines (fail-closed).** Rejected:
   there is no soundness/security stake in a string's leading spaces;
   lenient-strip is deterministic and keeps every existing program
   compiling. (Completeness beats strictness where no higher principle
   is in play.)

## Implementation plan (for a cold swarm lane)

The key constraint discovered in investigation: **column info dies at
the lexer** (Span is byte-only), and **span arithmetic for `{{expr}}`
sub-expressions is computed against the raw body** during
canonicalisation. Therefore: compute the anchor in the lexer, strip in
the canonicaliser.

1. **Lexer** (`src/compiler/parse/src/lexer.rs:478-509`): compute A per
   the rules above while scanning (the scanner already tracks
   line/col; record the column of the first non-newline content char).
   Extend the token to `Tok::TripleStr { raw: String, anchor: u32 }`.
   Do NOT strip here — the raw body must survive so downstream span
   arithmetic stays valid.
2. **Parser/AST** (`src/compiler/parse/src/parser.rs:1148`,
   `src/compiler/syntax/src/ast.rs:150-156`): carry the anchor —
   `Expr_::MultilineStr { raw: String, anchor: u32 }`. Update the
   formatter/pretty walkers the exhaustive-match friction points at.
3. **Canonicaliser** (the `desugarMultiline`-mirror): during the
   existing char walk that splits literal segments from `{{expr}}`
   segments, maintain "columns stripped on this line" state: after
   emitting a `\n` into a literal segment, skip up to A−1 following
   whitespace chars **in the raw body** (advancing the source offset —
   this is what keeps sub-spans correct) without emitting them. Drop
   the immediate leading newline per rule 2. Interpolation segments
   never participate (a `{{` cannot be leading whitespace); an interp
   spanning a newline resumes literal-stripping state after it closes.
4. **Formatter:** verify `ipe fmt` treats the body as opaque (it must
   not re-indent string bodies — if it ever aligns them, the value
   would change; add the idempotence + no-reindent test).
5. **Divergence bookkeeping (same commit):** promote the §6.9 planned
   entry in `docs/divergences-from-sky.md` to a live `divergence:`
   entry with this spec linked. Affected goldens get
   `oracle_divergence = true` + `divergence_reason` in `oracle.meta`
   per the established format (`tests/golden/*/oracle.meta`,
   `sanctioned.divergence` files). Upstream examples are NOT
   source-patched for #133 (the source stays valid; only values
   change) — the sweep's equivalence check consumes the recorded
   divergence per patch class instead. Where an example's *observable
   output* contains a multiline string (e.g. printed SQL/HTML), the
   sweep normalizer consults the divergence record rather than
   byte-comparing.
6. **Docs:** language syntax section (multiline strings) in the
   README-draft/templates updated in the same commit per the
   template-sync rule.

Ordering: Post-completion as filed; independent of #116/#128; touches
lexer+canonicaliser only (no type-system or backend interaction — the
desugared result is still a `++` chain of plain string literals).

## Test plan

Parser/canonicaliser unit tests (`src/compiler/parse` /
canonicaliser test module) — one per normative property:

- `m133_same_line_anchor` — `"""a\n   b"""` with opening at a known
  column: continuation stripped to A−1; value `"a\nb"` when b's
  indent < A.
- `m133_next_line_anchor` — `"""\n    hello\n      world\n    """`:
  leading newline dropped; anchor = col of `h`; `world` keeps 2 excess
  spaces.
- `m133_under_indented_preserved` — continuation line at column < A
  loses only its actual leading whitespace, content intact.
- `m133_blank_lines` — interior blank/whitespace-only lines → empty.
- `m133_tabs` — tabs count one column each; mixed tab/space stripping
  stops correctly.
- `m133_interp_spans` — `{{expr}}` after stripped margin: force a type
  error inside the interp and assert the diagnostic span still points
  at the right source columns (the span-arithmetic invariant).
- `m133_escapes_unchanged` — `\{{` and `\\` behave exactly as before
  around stripped margins.
- `m133_empty_and_singleline` — `""""""`, `"""x"""` unchanged.
- Formatter: `ipe fmt` twice byte-identical on a fixture containing an
  indented multiline string; body not re-indented.

Golden/E2E (`IPE_E2E=1`):

- `m133_sql_shape` — the `07-todo-cli` CREATE-TABLE shape printed to
  stdout; `oracle.meta` carries `oracle_divergence = true` with the
  patch-class reason; expected output is the *stripped* value.
- One pre-existing multiline golden re-recorded with the divergence
  flag, demonstrating the bookkeeping flow end-to-end.

Reference cross-check: run `m133_sql_shape` source through upstream Sky
once and archive its verbatim output inside the divergence entry (the
before/after pair is the documentation of the departure).
