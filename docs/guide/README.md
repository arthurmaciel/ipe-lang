# Learning Ipê

A path from never having seen Ipê to finding the exact function you need. Start
at the top and follow the links; each page assumes only the ones before it.

## The path

1. [Getting started](getting-started.md) — install, create a project, write and
   run a first program.
2. [Pure functions and immutability](pure-functions.md) — why an Ipê value never
   changes, and how effects are handled without breaking that.
3. [Types and inference](types.md) — how the compiler knows every value's type,
   and how `Maybe` and `Result` make absence and failure explicit.
4. [The Elm Architecture](the-elm-architecture.md) — how a program that reacts
   to input over time is structured.

## Reference and lookup

- [Glossary](glossary.md) — every term of art, defined once.
- **Module reference** (`../modules/`) — one page per `Ipe.*` module, each export
  with its signature, description, and a verified example. This is generated from
  the modules' source doc-strings (see the
  [documentation design](../internals/design/tbd/documentation-design.md)) and is
  browsable with `ipe doc serve`.
- [The Ipê language](../language/README.md) — the language reference, chapter by
  chapter (strings, errors, capabilities, the filesystem, views).
- [Application shapes](../shapes/README.md) — the four kinds of program and how
  to build each.

## Looking something up from the terminal

```
ipe doc Ipe.List            # a module's exports
ipe doc Ipe.List.filterMap  # one function: signature, description, example
ipe doc IPE-T0014           # a diagnostic code
```

Add `--plain` for terse output or `--json` for the machine-readable record.
`ipe doc serve` opens the full documentation as a local site.
