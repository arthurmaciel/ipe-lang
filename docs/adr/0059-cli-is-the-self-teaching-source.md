Status: Accepted
Date: 2026-08-12

# 0059. The `ipe` CLI is the self-teaching source for the language — for humans and machines alike

## Context

Two audiences increasingly learn a language the same way: by *asking the tool*.
A newcomer wants to know "how does `case` work?" or "why is this an error?"; an
AI coding agent wants to grasp the whole language and write idiomatic code the
first time, often without any external prose it can trust. Today the `ipe` CLI
teaches only part of this:

- `ipe explain <CODE>` — 128 pages covering the *semantics of diagnostics* (why
  an error happened), each ELI-first then deep, with a glossary.
- `ipe doc <Module>` — the stdlib API surface (types + signatures), also `--json`.

Missing is exactly what makes someone write *idiomatic* code first-try: how to
*write* each construct (syntax), and the *conventions* that aren't derivable
from signatures (state via TEA, effects via `do`/`|> Task.andThen`, `main` as a
`Task Error ()`, no functions in records). That knowledge lives only in prose a
human may not read and an agent may not have. And the discovery ergonomics are
weak: bare `ipe explain` dumps all 128 codes as a flat wall, and a reader must
already know the code — there is no way to ask by concept.

## Decision

**The compiler is the language's teacher. Everything needed to learn Ipê and
write idiomatic Ipê is queryable from the `ipe` CLI — the same content served
human-first by default and machine-readable on request.** `ipe explain` is that
teacher, and it covers three kinds of thing:

1. **Diagnostic codes** (existing) — `ipe explain IPE-T0001`.
2. **Constructs / syntax** — `ipe explain case` / `let` / `do` / `type` /
   `or-pattern` / `record-update` / `|>` / `module` — the form, its meaning, an
   **idiomatic worked example**, and see-also links.
3. **Topics / idioms** — `ipe explain effects` / `state` / `errors` / `shapes` /
   `main` — the conventions. This is the queryable idiom source.

Ergonomics, chosen deliberately:

- **Resolution is by concept, not just by code.** `ipe explain <query>` resolves
  an exact code/construct/topic to its page; otherwise it does a **fuzzy /
  keyword search** over codes, titles, and construct/topic names (extending the
  did-you-mean already in `explain_lookup`) and offers the best matches. A reader
  describes the problem in words — `ipe explain "type mismatch"`, `explain typo`,
  `explain pattern` — and reaches the page without knowing the code.
- **Progressive disclosure — simple by default, comprehensive on demand.** Bare
  `ipe explain` prints a short friendly overview (what you can ask; how to
  browse), not the 128-code wall. `ipe explain list [codes|constructs|topics]`
  is a **subcommand** (matching `ipe doc list`) that browses a category, grouped
  and readable. Each page is ELI-first then deep.
- **Human first, machine on request — one content, two renderings.** Default
  output (a TTY) is friendly prose with colour; `--json` emits the same content
  structured, for agents, editors, and tooling. Neither audience is a
  second-class citizen; the content is the single source, rendered twice.
- **The taught code is guaranteed correct.** Every example in every explain/doc
  page is compiled in CI (a doctest-style gate). This is load-bearing: it is what
  makes "idiomatic first-try" true — a reader (human or agent) can copy any
  example and it compiles and is idiomatic. A page that would misteach fails CI.
- **Diagnostics teach, and are machine-readable.** Compile diagnostics carry
  idiom-nudging hints that link `ipe explain <topic>` (so writing non-idiomatic
  code is itself a lesson), and `build`/`run`/`check` gain a stable JSON
  diagnostics mode so an agent parses feedback instead of scraping the human
  layout. The loop — ask, write, get taught by the error, fix — closes inside the
  CLI.

Alternatives considered and rejected: a separate `ipe learn`/`ipe tutor` command
(rejected — one teaching entry, `explain`, is simpler and `explain <construct>`
reads naturally); keeping `--list` as a flag (rejected — a subcommand browses and
scales to categories, and matches `ipe doc list`); a machine-only JSON surface
(rejected — the same knowledge serves humans, and duplicating it would drift).

## Consequences

- A human or an agent can learn Ipê and write idiomatic code from the binary
  alone. `AGENTS.md` shrinks toward a pointer ("explore via `ipe explain`") or is
  *generated* from the same source, ending the drift between docs and compiler.
- The **examples-compile gate is non-negotiable**: without it, a stale page
  becomes a misteaching hazard, worse than no page. It, and the jargon/tone gate,
  are what keep the teacher trustworthy.
- This is a **large, staged content effort** (like the 128 error pages, now for
  constructs and topics) — sequenced, not one change. The CLI surface (fuzzy
  resolve, `list` subcommand, `--json`) is small; the content is the bulk.
- It composes with and absorbs the machine-readable-diagnostics work and extends
  the friendly-diagnostics work — the same teacher voice, now for every reader.

## Conventions

ADRs describe Ipê on its own terms. This decision is stated as a standalone Ipê
principle — the compiler as the language's teacher — not as parity with any other
tool.
