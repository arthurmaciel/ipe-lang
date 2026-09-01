# Divergences from Elm

Ipê draws on Elm for its surface syntax, the Elm Architecture, and much of its
standard-library vocabulary. Where Ipê intentionally differs from an Elm API it
mirrors, the difference is recorded here with its reason, so a reader who knows
Elm is never surprised silently.

## `Ipe.Parser` (vs `elm/parser`)

`Ipe.Parser` is a pure parser-combinator library modelled on `elm/parser`. It
keeps Elm's names and semantics for `succeed`, `map`, `andThen`, `oneOf`,
`backtrackable`, `symbol`, `keyword`, `int`, `float`, `spaces`, `chompIf`,
`chompWhile`, `getChompedString`, `end`, `loop` (`Step = Loop | Done`),
`Problem`, and `DeadEnd`. It differs as follows.

- **`Parser a` is a transparent type alias**, `State -> PStep a`, not an opaque
  wrapper. Elm hides the step function inside an opaque
  `Parser context problem value`. The Rust backend cannot store a function value
  inside a union payload (IPE-L0107), so a `type Parser a = Parser (…)` wrapper
  does not lower, whereas a bare function-typed value does. The alias is the
  sound shape; users still compose only through the combinators.

- **Record building uses `map2` … `map5`, not the `|=` / `|.` pipeline.** Elm
  writes `succeed ctor |= partA |= partB`, threading a curried constructor
  through a parser accumulator. That requires a `Parser` whose value is a
  function, which the backend cannot lower. `map2` … `map5` apply the builder
  directly to already-parsed results, so no parser holds a function. `keep` and
  `ignore` are two-parser sequencers for punctuation (run one, keep the other's
  value), not pipeline stages.

- **No `|=` / `|.` operators.** Ipê has no user-declarable infix operators yet
  (tracked in #1655). The named combinators above stand in; the operators can be
  added as thin aliases once infix support lands.

- **No `Parser.Advanced` tier.** Elm's context stack and custom-`problem` type
  parameter are omitted; `Problem` is a fixed union.

### Backend lowering caveat (tracked in #1657)

The whole `Ipe.Parser` API type-checks, but the Rust backend currently lowers
only the **non-composed** subset to runnable code: `run`, `succeed`, `int`,
`float`, `symbol`, `keyword`, `end`, `chompIf`, `chompWhile`,
`getChompedString`, `spaces`, `oneOf`, `backtrackable`, `map`, and a single
(non-nested) `andThen`. The combinators that build or sequence parsers through a
captured polymorphic function value — `map2` … `map5`, `keep`, `ignore`, `loop`,
and any nesting of `andThen` — type-check but hit a `Send + Sync` boxing /
returned-function-application gap in the backend and do not yet lower. They ship
so the reference API is complete and the gap can be closed in one place. Until
then, build parsers from the runnable subset. The shipped example under
`examples/shapes/script/parser-demo` uses only that subset and is exercised by
the first-party examples sweep.
