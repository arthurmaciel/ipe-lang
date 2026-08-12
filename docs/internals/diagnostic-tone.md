# Ipê Diagnostic Tone Guide

This document is the house-style SSOT for Ipê diagnostic messages — the voice,
vocabulary, and structural rules every pass that touches `render.rs`,
`diagnostic.rs`, or the `explain/*.md` pages must follow.

The render-golden harness in
`src/compiler/diagnostics/tests/render_goldens/` locks the current output
byte-for-byte. A pass that changes wording must re-bless those files with
`UPDATE_GOLDENS=1`; the diff is then the reviewable record of every word that
changed.

---

## Voice: second person, the compiler speaks

Write as if the compiler is a knowledgeable colleague, not an authority handing
down verdicts:

| Avoid | Prefer |
|-------|--------|
| "Unexpected token." | "I found `42` where I was expecting an identifier." |
| "Type mismatch error." | "I was expecting `Int`, but this expression has type `String`." |
| "Import not found." | "I can't find the module `Httpp`." |

The compiler says "I" when it describes what it found or expected. It says "you"
(or omits the subject) only when describing what the author can do next.

---

## No blame

The reader did not make a mistake — the program and the compiler disagree. Never
use words that imply carelessness or fault:

- No "illegal", "forbidden", "invalid" — prefer "not supported", "not allowed here".
- No "you forgot", "you must" — prefer "add …", "try …".
- No exclamation marks.

---

## The code is not the headline

`IPE-T0001` is a lookup key for `ipe explain`, not a summary. The error code
appears in the header but it never _is_ the message. The title after `:` must be
a plain-English description a reader can act on without knowing the code at all.

Good header: `error[IPE-T0001]: type mismatch`

Bad header: `error[IPE-T0001]: IPE-T0001`

---

## Always give a concrete next step

Every diagnostic must end with at least one actionable line — a `help:` hint, a
`note:` suggestion, or the `= note: run \`ipe explain <CODE>\`` footer (the
renderer appends the explain pointer automatically). The reader should never be
left with only a description of what went wrong and no path forward.

Good: "I can't find `lenght` in scope. `help: replace \`lenght\` with \`length\``"

Bad: "Name `lenght` not found."

---

## No internal jargon in user-facing text

The internal representation lives in the compiler, not in the user's mind. These
terms must never appear in a rendered diagnostic or an `explain/*.md` page:

| Internal term | Approved user-facing equivalent |
|--------------|--------------------------------|
| `Ty` / `IrType` | "type" |
| `salsa` | (never mention) |
| `canon` / "canonical" | "resolved" or just drop it |
| `HM inference` / "unification" | "type checking" |
| `VarId` / `Symbol` | (never mention) |
| `zonk` / "zonking" | (never mention) |
| raw Rust types (`Vec<_>`, `Box<_>`, `u32`) | (never mention) |
| stage names ("lowerer", "canonicaliser") | use "the compiler" |
| `ICE` (except in the IPE-I* title) | "internal compiler error" |

When an `explain/*.md` page needs to describe how the compiler works internally
to motivate why a restriction exists, use plain language at the user's level.

---

## Span labels are inline phrases, not sentences

The text that follows `^^^` or `---` is a phrase aligned with the offending
token, not a standalone sentence. Keep it short (ideally under 60 characters),
lowercase, no trailing period.

The golden files in `src/compiler/diagnostics/tests/render_goldens/` show the
exact current layout for all covered families — read those as the living examples.
As a rule of thumb:

- Good label: `expected Int, found String`
- Bad label: `This expression has type String but Int was expected here.`

---

## Help and note lines use `help:` / `note:` prefixes

- `help:` — something the author can actively do (add, remove, replace, rename).
- `note:` — context that aids understanding but is not an actionable edit.

These prefixes are written in lowercase and followed by a space. The renderer
adds the `= ` gutter prefix automatically; do not write it into the message text.

---

## Did-you-mean collapses when there are multiple candidates

When the compiler has two or more name suggestions, they collapse into a single
`help: did you mean one of:` block with each candidate indented beneath. A single
candidate gets the terse inline form `help: did you mean \`X\`?`.

This collapsing is automatic in `render.rs`; no manual logic is needed in
individual diagnostic constructors.

---

## Internal compiler errors (IPE-I*) always apologise

A `CompilerBug` is a gap in the compiler, not the author's fault. The renderer
always appends:

1. The `detail` string as a `note:` (so the author has something to paste into
   a bug report).
2. `note: this is a bug in Ipe, please report it`
3. A polite apology and the issue tracker URL.

When writing a new `CompilerBug`, the `detail` field should contain the internal
context a developer needs to localise the failure — not a user-facing message.

---

## Explain pages follow the same voice

The `explain/*.md` pages are long-form companions to the codes. They must:

- Open with a one-paragraph plain-language description of what the code means.
- Give a minimal failing example followed by a corrected version.
- Explain the _why_ at the user's level (type safety, effect discipline, …) —
  not the implementation detail.
- Use second person consistently ("If you see this …", "You can fix this by …").
- Never reference internal types, stage names, or the Rust backend directly.

---

## Timeless phrasing

Diagnostic text and explain pages are permanent reference material. Avoid:

- Temporal markers: "currently", "for now", "as of this version".
- Internal build-process labels (work-item numbers, batch names, iteration markers).
- Hedges about future support: "may be supported later" — if something is not
  supported, say so plainly and point to the workaround that exists today.

When a feature is not yet implemented, the lower-layer diagnostic already carries
a `[feature: <name>]` tag in the label. The tone guide's job is to make the
surrounding prose humane, not to promise anything.
