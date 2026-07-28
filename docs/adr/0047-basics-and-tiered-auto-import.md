Status: Accepted
Date: 2026-07-28

# 0047. `Ipe.Basics` and the three-tier auto-import model

## Context

Every Ipê module needs a small set of names available without ceremony:
operators (`+`, `==`, `|>`), the base types (`Int`, `String`, `Bool`), and the
handful of core-language types that appear in almost every signature (`Maybe`,
`Result`, `List`). Requiring an `import` line to write `Maybe a` or `x + y` is
pure friction — these are the *vocabulary* of the language, not a library a file
opts into.

Two forces pull against each other:

- **Ceremony** — forcing an import for names that are effectively part of the
  grammar makes every file noisier and teaches nothing.
- **Magic** — making *everything* ambiently available (every stdlib module in
  scope with no import) hides what a file actually reaches for. A reader can no
  longer tell, from the imports, which capabilities a module touches.

The dividing line that resolves the tension is an axis, not a list: **types vs.
functions.** Core-language *type* names are vocabulary; they carry no capability
and appear in nearly every type signature, so they are implicit. Module
*functions* (`List.map`, `String.toUpper`, `Dict`, `Http`) are capabilities a
file chooses to use, so they are explicit and qualified — a reader sees every
capability in the import list.

The prior surface conflicted with this. An implicit module named `Ipe.Prelude`
was re-exported and, worse, examples opened it with `import Ipe.Prelude exposing
(..)` — flooding every value it carried into the unqualified namespace. That
open import is exactly the magic this decision rejects: it makes the set of
in-scope names unbounded and invisible at the call site.

Ipê's surface is designed to align with Elm's, whose `Basics` module fixes a
tight, well-chosen set of implicitly-available names. Adopting that set as the
boundary of what is ambient gives a principled, non-arbitrary line.

## Decision

Rename the implicit `Ipe.Prelude` to **`Ipe.Basics`** and adopt a three-tier
auto-import model.

**Tier A — `Ipe.Basics`.** Auto-imported unqualified into every module. Its
export set is scoped to exactly Elm's `Basics`: the arithmetic/comparison
operators (`+ - * / // ^ == /= < > <= >=`); `Bool` with `not`/`&&`/`||`/`xor`;
the base types `Int`/`Float`/`Char`/`String`; the function operators
`<| |> << >>`; `identity`/`always`; `++`; `Order` with `LT`/`EQ`/`GT`;
`Never`/`never`; and the numeric/math functions `min`/`max`/`abs`/`clamp`/
`negate`/`compare` and friends. Nothing library-flavoured lives here.

**Tier B — core type vocabulary.** In scope with no import: the type names
`List`, `Maybe`, `Result` and their constructors `Just`/`Nothing`, `Ok`/`Err`,
plus `True`/`False` and `LT`/`EQ`/`GT`. These appear in nearly every signature
and `case`; requiring an import for the *type* `Maybe` is ceremony. Local
definitions shadow these names normally — a user `map` binds locally without a
diagnostic.

**Tier C — everything else.** Every other module function requires an explicit
`import` and is used qualified: `List.map`, `String.toUpper`, `Dict`, `Set`,
`Json`, `Http`. A Tier-C name used without its import fails to resolve
(IPE-N0001 / IPE-N0004). This is stricter than Elm, which makes stdlib modules
ambiently available for qualified use; Ipê deliberately requires the import so
the import list is a complete inventory of a file's capabilities.

The line, stated once: **core-language types are vocabulary (implicit); library
types and all functions are imports (explicit).** A user's own type's
constructors always require its module to be imported — no type's constructors
are ever ambient except the Tier-B core set.

The value-flooding open import `import Ipe.Prelude exposing (..)` is **removed**.
Tiers A and B are ambient, so nothing is lost; examples and goldens drop the
line.

To keep Tier C painless, an **LSP "add import" code action** resolves an
unimported qualified name: an unresolved `List.map` offers a quick-fix inserting
`import Ipe.List as List`. This is the ergonomic counterpart to the strictness —
the reader gets a complete import list, the author gets it filled in
automatically.

### Alternatives rejected

- **Keep the open `exposing (..)` prelude.** Rejected: it makes the ambient name
  set unbounded and invisible, the "magic" this decision exists to remove.
- **Make all stdlib modules ambient (Elm's model) for Tier C.** Rejected: a
  reader could no longer see, from the imports, which capabilities a file
  reaches for. The explicit-import rule is the no-magic guarantee.
- **Require an import even for `Maybe`/`Result`/`List` (no Tier B).** Rejected as
  pure ceremony: these are grammar-level vocabulary, present in almost every
  signature.

## Consequences

- The unqualified ambient surface is now **finite and fixed** — exactly Tiers A
  and B. A reader knows every name that can appear unqualified without an
  import, and the import list enumerates everything else.
- The compiler owns one canonical implicit module, `Ipe.Basics`, scoped to the
  Tier-A set; the old `Ipe.Prelude` alias is retired.
- Tier C's strictness is only tolerable *with* the LSP add-import action; that
  action is part of this decision, though it may ship as a later stage than the
  resolver change.
- The invariant that must hold: nothing library-flavoured may migrate into Tier
  A or B. The moment a capability-bearing function becomes ambient, the no-magic
  guarantee is broken. New stdlib functions are always Tier C.
- Adding a new core-language type to Tier B is a deliberate, reviewed change to a
  small fixed list, never an incidental consequence of adding a stdlib module.
