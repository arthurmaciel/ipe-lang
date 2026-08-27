# Property-based tests, co-located with the code under test

Status: design proposal, no implementation yet. The Ipê sketches show the
**intended surface** — this capability does not exist yet, so they are not
runnable; they illustrate types, not a shipped API.

## The problem

Ipê already has example-based unit tests (`Ipe.Test`): an author writes concrete
inputs and asserts concrete outputs. That catches the cases the author thought
of. A **property test** instead states a law that must hold for *all* inputs —
`List.reverse (List.reverse xs) == xs`, `Json.decode (Json.encode v) == Ok v` —
and a *generator* manufactures many inputs to try to falsify it. When a
counterexample is found, a *shrinker* reduces it to the smallest failing case so
the author reads `[0]`, not `[-4823, 991, …, 17]`.

Two things are wanted:

1. A **property surface** — declare a property + the generators that feed it, in
   Ipê source, composing small generators into big ones.
2. **Co-location** — a property lives next to the code it exercises (same module
   or a sibling), discovered and run without a hand-maintained registry, rather
   than only in a separate `tests/` tree.

## Current test baseline

What exists today, so the design extends rather than replaces it:

- **`Ipe.Test`** (`src/stdlib/Ipe/Test.ipe`) is a pure, in-process framework.
  `Test` is a tree — `Leaf String (() -> TestResult)` or `Suite String
  (List Test)`; `TestResult = Passed | Failed String`. Assertions (`equal`,
  `ok`, `err`, `expectErr`, `isTrue`, …) return a `TestResult`. `run : List Test
  -> List (String, TestResult)` walks the tree purely; `runMain : List Test ->
  Task Error ()` prints a summary and `System.exit`s 0/1. A universal renderer
  (`errorToString`, backed by `Basics.toString` / `{{expr}}` interpolation) turns
  any value into a failure message.
- **A test file is an ordinary program.** It exposes `main = Test.runMain tests`
  and `tests : List Test`, then is compiled and run like any `.ipe` program
  (`examples/…/36-composite-server/tests/CompositeServerTest.ipe`). There is **no
  `ipe test` subcommand** — the CLI dispatch (`run_cli`, `src/ipe-cli/src/lib.rs`)
  has `build`/`run`/`check`/`watch`/`fmt`/`doc`/… but no `test`. Tests are found
  by convention (a `tests/` dir) and run via `ipe run`.
- **`Ipe.Random`** (`src/stdlib/Ipe/Random.ipe`) already ships a **pure,
  deterministic seeded PRNG**: an opaque `Seed`, `seed : Int -> Seed`, and
  `seededInt : Seed -> Int -> Int -> (Int, Seed)` / `seededFloat : Seed ->
  (Float, Seed)` / `seededChoice : Seed -> List a -> (Maybe a, Seed)`. The kernel
  (`random_seeded_int`, splitmix64, `src/runtime/rust/src/random.rs`) is pure and
  reproducible across runs, and is itself already property-tested in Rust
  (`prop_ln_always_in_range`). It is documented non-cryptographic — fine here,
  since a fuzzer must be *predictable* (reproduce a failure from its seed), the
  opposite of a CSPRNG requirement.

The gap is precisely the middle layer: a `Fuzzer a` abstraction over the seeded
PRNG, a `fuzz` property builder that plugs into the existing `Test` tree, and a
shrinker. The runner and reporting already exist.

## Approaches considered

### A. Bind a Rust property crate (`proptest` / `quickcheck`) over FFI

Lower `fuzz` properties onto a vendored Rust property engine. The engine owns
generation, shrinking, and its own case-scheduling.

- **Against.** It contradicts the `Url.Parser` precedent (prefer a pure, total,
  portable Ipê implementation unless a crate is genuinely needed) on every count.
  The FFI boundary is a **security gate** (per AGENTS.md every language boundary
  needs a security-soundness review) bought for a subsystem that is pure
  arithmetic over a seed we *already own in pure Ipê*. It splits the source of
  randomness truth: `Ipe.Random`'s splitmix seed vs the crate's internal RNG,
  breaking single-source-of-truth for determinism. `proptest`'s value model and
  shrinker are Rust types; surfacing `Fuzzer a` for arbitrary Ipê ADTs across
  that boundary needs erasure/marshalling the backend forbids (no `dyn Any`).
  And it does not compile to the WASM target Ipê also serves.
- **The one thing it buys** — a mature shrinker and integrated failure-case
  persistence — is reproducible in pure Ipê at a fraction of the boundary cost
  (see the design). Rejected.

### B. Pure-Ipê fuzzers over the existing seeded PRNG (recommended)

Add a compiled-source `Ipe.Fuzz` module: `Fuzzer a` is a function from a `Seed`
to a value plus the next seed (and an integrated shrink tree), built on
`Ipe.Random`'s `seededInt`/`seededFloat`. `fuzz`/`fuzz2`/`fuzz3` are property
builders that produce ordinary `Ipe.Test.Test` leaves, so they drop straight into
the existing tree, runner, and reporting.

- **For.** Pure, total, portable — same code on native and WASM. Zero new FFI, no
  security gate, no new kernel (the seeded PRNG kernel already exists and is
  proven). One source of determinism (`Ipe.Random`'s seed). `Fuzzer a` composes
  in Ipê with `map`/`andThen`/`map2`, so any user ADT is generatable without
  erasure. It nests inside `Ipe.Test`, so co-location and the runner come almost
  for free.
- **Against.** We implement shrinking ourselves. Mitigated by the *integrated
  shrinker* model (below), which is a modest, well-understood amount of code and
  the design the Elm lineage validated.

### C. Random generation only, no shrinking

Generate N cases; on failure report the raw counterexample and the seed, no
minimisation.

- **Against.** Falsifiable but hostile: an un-shrunk counterexample is often
  unreadable, and PRINCIPLES ranks Readability as a real principle. This is the
  MVP of B, not a separate destination — ship it as **Phase 1** of B, then add
  the shrinker. Not a standalone recommendation.

## Recommendation

**Approach B — pure-Ipê fuzzers over the existing `Ipe.Random` seeded PRNG.**
This mirrors the `Url.Parser` decision exactly: a pure/total/portable Ipê
implementation is preferred over binding a crate unless the crate is genuinely
needed, and here it is not — the hard primitive (a deterministic, reproducible,
already-proven seeded PRNG) is *already in the tree as pure Ipê*, the composition
layer is ordinary Ipê functions, and the runner is the existing `Ipe.Test`. A
crate would add an FFI security boundary, a second source of randomness, an
erasure problem the backend forbids, and no WASM story, to buy a shrinker we can
write in pure Ipê. The Elm reference (`elm-explorations/test`, `Fuzzer a` +
`fuzz`) is itself pure Elm with an integrated shrinker — the lineage we "look to
first" already chose this shape.

## The design

### The `Fuzzer a` surface

The Elm-lineage vocabulary, adapted to Ipê. A `Fuzzer a` bundles a **seeded
generator** with an **integrated shrink tree**: producing a value also produces
the lazy tree of smaller values to try if that value fails. Integrated shrinking
(rather than a separate `shrink : a -> List a`) keeps the shrinker in sync with
the generator by construction — you cannot generate a value the shrinker cannot
shrink, which is *make-invalid-states-unrepresentable* applied to shrinking.

```
module Ipe.Fuzz exposing
    ( Fuzzer
    , int, intRange, float, bool, char, string, unit
    , constant, oneOf, frequency, maybe, result
    , list, listOfLength, tuple, tuple3
    , map, map2, map3, andThen, filter
    )

-- Opaque. A Fuzzer produces a RoseTree: the drawn value plus the lazy
-- tree of "smaller" candidates to try when the property fails.
type Fuzzer a
    = Fuzzer (Seed -> ( RoseTree a, Seed ))

-- A value and its shrink candidates (children are themselves shrinkable).
type RoseTree a
    = Rose a (List (RoseTree a))
```

Primitives draw from `Ipe.Random.seededInt` / `seededFloat` and pair the draw
with its shrink tree (integers shrink toward 0; lists shrink by dropping elements
and shrinking survivors; strings via their char list). Combinators are pure Ipê:

- `map : (a -> b) -> Fuzzer a -> Fuzzer b` — maps value and every shrink node.
- `map2`/`map3` — thread the seed left-to-right, combine values, and interleave
  the child shrink trees so a tuple shrinks each component.
- `andThen : (a -> Fuzzer b) -> Fuzzer a -> Fuzzer b` — monadic dependence (draw
  a length, then a list of that length).
- `oneOf` / `frequency` — pick a branch by a drawn index / weighted index (reuse
  the same weighting shape as `Ipe.Random.weighted`).
- `filter` — resample up to a bounded retry count, then fail closed with a
  diagnostic (never loop forever on an unsatisfiable predicate — a soundness
  requirement: a well-typed test program must terminate).

Because it is all pure Ipê, a user ADT gets a fuzzer by composition:

```
type Point = Point Int Int

pointFuzzer : Fuzzer Point
pointFuzzer =
    Fuzz.map2 Point (Fuzz.intRange 0 100) (Fuzz.intRange 0 100)
```

### The property builder — nesting into `Ipe.Test`

`fuzz` produces an ordinary `Test`, so the existing tree/runner/reporting apply
unchanged. A property is a function returning the existing `TestResult`, letting
authors reuse every `Ipe.Test` assertion:

```
module Ipe.Test.Fuzz exposing ( fuzz, fuzz2, fuzz3, fuzzWith, Options )

fuzz : Fuzzer a -> String -> (a -> TestResult) -> Test
fuzz2 : Fuzzer a -> Fuzzer b -> String -> (a -> b -> TestResult) -> Test

-- Example, co-located in the module under test's sibling test:
reverseInvolutive : Test
reverseInvolutive =
    Test.Fuzz.fuzz (Fuzz.list (Fuzz.intRange -100 100))
        "reverse is its own inverse"
        (\xs -> Test.equal xs (List.reverse (List.reverse xs)))
```

Under the hood `fuzz` builds a `Leaf name thunk` whose thunk: derives a run seed,
draws `runCount` values, evaluates the property on each; on the first `Failed`,
walks the value's rose tree greedily to the smallest still-failing child, and
reports the shrunk value **plus the originating seed** so the exact run
reproduces. All pure — it fits `run`'s existing `() -> TestResult` shape with no
runner change.

### Shrinking

Integrated (rose-tree) shrinking, as above. On failure the runner does a
depth-first greedy descent: among a node's children, take the first that still
fails the property, recurse, stop when no child fails. This is bounded by the
tree depth (finite: integers shrink toward 0, lists toward `[]`), so it always
terminates — the soundness bar. The reported counterexample is the last failing
node; the seed that produced the original draw is printed alongside so the author
can pin a regression.

### Determinism and seeding

- **Reproducible by construction.** All randomness flows through
  `Ipe.Random`'s pure `Seed`; identical seed ⇒ identical run, on native and WASM
  alike (single source of truth for determinism).
- **Default seed.** A property run derives its seed from a fixed base mixed with
  the property's name (stable string hash), so a given property is reproducible
  across runs *and* independent properties don't share a draw sequence.
- **Override.** `fuzzWith { seed, runs }` (an `Options` record) pins the seed and
  case count — the mechanism for reproducing a reported failure and for a
  regression that must always retry the known-bad seed.
- **No wall-clock entropy in tests.** The framework never calls the entropy-backed
  `Ipe.Random.int`/`float` (those return `Task` and are non-reproducible); only
  the seeded pure variants. A lint-style note in the module docs states this
  invariant.

### Discovery and co-location

Goal: a property lives next to the code it tests and is found without a
hand-edited registry. Ordered by how much new machinery each needs:

1. **Convention, no compiler change (Phase 1 shipping form).** A module under
   test has a sibling `⟨Module⟩Test.ipe` (the existing pattern) that `exposing
   (tests)` a `tests : List Test`, mixing `Test.test` and `Test.Fuzz.fuzz`
   leaves. Co-location = "sibling file", discovery = "the `tests/` convention".
   Works today with zero CLI work.
2. **`ipe test` subcommand (Phase 3).** A thin driver that, given a project or
   path, finds every module exposing `tests : List Test`, builds a synthetic
   entry that `Test.runMain (List.concat [...])` over them, runs it, and maps the
   0/1 exit to a pass/fail report. Discovery becomes "any module exposing
   `tests`", so a property can live in the same directory as its code, not only a
   `tests/` tree. This is a CLI/driver feature (a new `run_test` arm in
   `run_cli`), not a language change, and reuses the whole existing pipeline —
   `Test.runMain` already does the exit-code contract `ipe test` needs.
3. **In-source annotation (deferred, open question).** A `{-@ test -}`-style
   marker letting a property sit literally beside the function in the *same*
   module, harvested by the compiler. Strictly more than co-location-by-sibling
   needs; parked unless demand appears (see Open questions).

The recommendation ships **1** immediately (surface + fuzzers land, usable via
`ipe run` on a sibling test), then **2** to make discovery ergonomic and give
Ipê a real `ipe test`. **3** stays an open question.

### Runner integration

- Phase 1: none needed — a fuzz property is a `Test`; `Test.runMain` runs it.
- Phase 3: `ipe test [path]` → discover modules exposing `tests` → synthesise an
  entry calling `Test.runMain` over the union → compile + run via the existing
  build/run path → surface the summary and exit code. Report shows, per failing
  property, the shrunk counterexample and its seed. Honour a `--seed` flag
  (forces `fuzzWith`) and a `--runs N` default.

## Phased implementation plan (dependency-ordered)

1. **Phase 1 — `Fuzzer` core + primitives (pure Ipê, no shrinking).**
   Add `Ipe.Fuzz` with `Fuzzer`, `int`/`intRange`/`float`/`bool`/`unit`,
   `constant`, and `map`/`map2`/`andThen`, generating over `Ipe.Random`'s seeded
   PRNG. Add `Ipe.Test.Fuzz` with `fuzz`/`fuzz2` producing `Test` leaves that draw
   N cases and report the raw counterexample + seed (Approach C behaviour). No
   new kernel, no FFI, no CLI change. **Depends on:** existing `Ipe.Random`,
   `Ipe.Test`. Unblocks everything else.
2. **Phase 2 — integrated shrinking.** Introduce `RoseTree`; make primitives
   emit shrink trees (ints→0, lists→drop/shrink, strings→char list); make
   `map`/`map2`/`andThen`/`oneOf`/`frequency`/`filter` propagate them; add the
   greedy DFS minimiser in the `fuzz` thunk. Reports now show the *minimal*
   counterexample. **Depends on:** Phase 1.
3. **Phase 3 — richer combinators + `ipe test`.** Add `char`/`string`/`list`/
   `listOfLength`/`tuple`/`tuple3`/`maybe`/`result`/`oneOf`/`frequency`/`filter`
   and `fuzzWith`/`Options`. Add the `ipe test` CLI arm: discover modules
   exposing `tests : List Test`, synthesise a `Test.runMain` entry, run,
   report; `--seed`/`--runs` flags. **Depends on:** Phase 2 for shrinking in
   reports; Phase 1 for the surface.
4. **Phase 4 — docs, examples, dogfood.** A teaching page in the compiler's
   explain/glossary voice (property vs example testing, generators, shrinking,
   seeds). Convert one existing example's hand-rolled cases to properties. Add a
   property suite for `Ipe.Random` itself and for a stdlib round-trip
   (`Json.encode`/`decode`). **Depends on:** Phase 3.

Each phase is independently landable and leaves the tree green: Phase 1 is
useful on its own (random cases + seed), and every later phase strictly adds.

## Test strategy for the feature itself

The framework is pure Ipê, so it is testable with the tools it provides plus the
Rust harness:

- **Fuzzer laws, in `Ipe.Test`.** `map identity == identity`; `constant x` always
  draws `x`; `intRange lo hi` output is always in `[lo, hi]`; a fixed seed
  reproduces a fixed draw sequence (determinism regression). These are ordinary
  `Ipe.Test` leaves — no bootstrap paradox, because they assert *concrete* draws
  from *pinned* seeds.
- **Shrinker correctness.** For a property with a known minimal counterexample
  (e.g. "all ints are < 5" over `intRange 0 100`), assert the reported shrink is
  exactly the boundary value, from a pinned seed. Assert shrinking always
  terminates (bounded tree depth) via a pinned adversarial generator.
- **Golden CLI test (Phase 3).** A `golden_*.rs` in `src/ipe-cli/tests/`: a small
  project with one passing and one failing property; run `ipe test`; assert exit
  code, that the failing property's *shrunk* value and seed appear in the report,
  and that `--seed` reproduces. This also enforces the SEAL (accepts ⇒ builds ⇒
  runs).
- **Determinism across targets.** A pinned-seed draw sequence asserted equal on
  native and WASM (the portability claim that justifies pure-Ipê over a crate).
- **The `Ipe.Fuzz`/`Ipe.Test.Fuzz` modules go through the standard example
  sweep** so any codegen regression in the new surface is caught by CI, not only
  by unit tests.

## Open questions

1. **In-source property annotation (discovery option 3).** Is
   same-module-beside-the-function co-location worth a compiler-harvested marker,
   or is the sibling-`⟨Module⟩Test.ipe` convention (option 1/2) sufficient
   co-location? Deferred; revisit if authors ask for it. A marker adds a parser
   surface and a harvest pass — real cost — for an ergonomic gain the sibling
   file mostly already delivers.
2. **Should `ipe test` be its own subcommand or a `--test` flag on `run`?** A
   subcommand is clearer and matches the Elm/`elm-test` and `cargo test` mental
   model; a flag is cheaper. Leaning subcommand (option in Phase 3), pending the
   CLI owner's call — and it must never advertise the command before it works.
3. **Case-count / time budget default.** Elm defaults to 100 runs. Fixed default
   vs a per-property `fuzzWith` override vs a project-manifest (`ipe.toml`)
   default. Proposal: fixed 100, overridable via `fuzzWith`, manifest default
   deferred.
4. **Failure-case persistence.** `proptest` writes a regression file of
   known-bad seeds. Do we persist a `.ipe-fuzz-failures` seed list for `ipe test`
   to retry first, or is printing the seed (for a hand-pinned `fuzzWith`
   regression) enough? Proposal: print-the-seed first; persistence is a later,
   separable enhancement if it earns its keep.
5. **Distribution / coverage feedback.** No coverage-guided generation is
   proposed (that *would* argue for a crate). Confirm plain random + shrinking is
   the target; if coverage-guided fuzzing is ever wanted, re-open the crate
   question for that feature specifically, behind its own security review.
