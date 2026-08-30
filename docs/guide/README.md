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

## Standard-library guides

Each teaches the *mental model* of one module through a worked, runnable example,
then links to the per-symbol `ipe doc` reference. Read the one you need; they
cross-link where topics meet.

- **Collections** — [Sets](set.md) (unique, unordered, membership),
  [Dictionaries](dict.md) (a value per key).
- **Absence and failure** — [Maybe](maybe.md) (a value that might be missing),
  [Result](result.md) (a failure that carries a reason).
- **Text and binary** — [Regular expressions](regex.md), [Text
  encodings](encoding.md) (base64/URL/hex), [Bytes](bytes.md) (raw octets).
- **Files and configuration** — [Files](file.md) (typed paths, effects as tasks),
  [Configuration](config.md) (typed TOML/YAML/JSON decoders), [CSV](csv.md).
- **Numbers** — [Math](math.md) (roots, trig, rounding, NaN).

## Reference and lookup

- [Glossary](glossary.md) — every term of art, defined once.
- **Module reference** (`../modules/`) — one page per `Ipe.*` module, each export
  with its signature, description, and a verified example; browsable with `ipe doc serve`.

## Looking something up from the terminal

```
ipe doc Ipe.List            # a module's exports
ipe doc Ipe.List.filterMap  # one function: signature, description, example
ipe doc IPE-T0014           # a diagnostic code
```

Add `--plain` for terse output or `--json` for the machine-readable record.
`ipe doc serve` opens the full documentation as a local site.
