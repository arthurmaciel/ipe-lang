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

- **The prelude** — [Basics](basics.md) (the auto-imported helpers: `clamp`,
  `compare`, `toString`, `min`/`max`).
- **Core data** — [Lists](list.md) (ordered sequences, folds, pipelines),
  [Strings](string.md) (text and the parse boundary), [Characters](char.md)
  (code points, classification), [Tuples](tuple.md) (anonymous pairs).
- **Collections** — [Sets](set.md) (unique, unordered, membership),
  [Dictionaries](dict.md) (a value per key).
- **Absence, failure, and effects** — [Maybe](maybe.md) (a value that might be
  missing), [Result](result.md) (a failure that carries a reason), [Error](error.md)
  (the structured, classified failure type), [Tasks](task.md) (effects as values,
  sequenced and recovered).
- **Text and binary** — [Regular expressions](regex.md), [Text
  encodings](encoding.md) (base64/URL/hex), [Bytes](bytes.md) (raw octets),
  [Compression](compression.md) (gzip/zstd over bytes).
- **Serialization** — [Codec](codec.md) (one bidirectional codec for JSON and
  storage; the round-trip law by construction).
- **Files and configuration** — [Files](file.md) (typed paths, effects as tasks),
  [Configuration](config.md) (typed TOML/YAML/JSON decoders), [CSV](csv.md).
- **Numbers and magnitudes** — [Math](math.md) (roots, trig, rounding, NaN),
  [Durations](duration.md) (unit-explicit time spans), [Byte sizes](bytesize.md)
  (unit-explicit byte quantities).
- **Exact quantities and money** — [Decimal](decimal.md) (arbitrary-precision
  decimal arithmetic), [Money](money.md) (currency-typed amounts, fair splits).
- **Randomness and identifiers** — [Randomness](random.md) (entropy vs seeded),
  [UUIDs](uuid.md) (random and time-ordered ids).
- **Addressing and routing** — [URLs](url.md) (typed, validated),
  [URL routing](url-parser.md) (typed route patterns), [Network
  primitives](net.md) (range-validated ports).
- **The process and the terminal** — [System](system.md) (arguments, environment,
  working directory, exit), [Standard I/O](io.md) (stdout/stderr/stdin, the
  password read).
- **Content and markup** — [HTML](html.md) (typed element trees, XSS-safe by
  construction), [Markdown](markdown.md) (a typed block tree, no raw HTML).

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
