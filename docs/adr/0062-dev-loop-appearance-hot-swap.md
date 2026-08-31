Status: Accepted
Date: 2026-08-31

# 0062. Dev-loop appearance hot-swap, and the data/logic boundary

## Context

A program is iterated by small, frequent edits whose effect you cannot infer from
the source — spacing, colour, text, layout. If each such edit costs a full
recompile-and-restart, focus is lost and the tool feels hostile. The target is that
an edit is reflected in the running program fast enough that there is no time to
switch attention away.

The compiled model is the constraint. An Ipê web application is a message loop
(`update : Msg -> Model -> Model`, `view : Model -> Html`) compiled to a native
binary; the running server renders `view` and pushes a diff to the browser over a
live socket. A naive edit rebuilds the binary and restarts it — dropping the socket,
resetting to `init`, and paying the compiler.

The forces, in strict precedence: **the dev preview must be exactly what ships**
(no second execution semantics — dev must equal prod); then latency. Any mechanism
that renders in dev by a different path than the compiled program can diverge, and
a divergence means the tool lies about what the code does.

## Decision

Split every edit into two kinds and serve each with the soundest mechanism.

**Appearance edits — hot-swap as data, compiling nothing.** An appearance literal
(a style value, an attribute value, static text) is inert data the compiled `view`
already consumes without branching on it. The compiler hoists these literals into a
per-view **literal table** that the emitted `view` reads by index, with the source
values baked in as the table's defaults. This is not a dev-only shadow: the shipped
program runs the identical code reading the identical table, so there is exactly one
render semantics. On an appearance edit the running program is handed a new table,
applies it, and re-renders `view` at its **current** Model through the existing
diff-and-push path — no compile, no restart, no reset. The dev loop is thus a
self-similar control loop one level up: a file change is a message, a conservative
classifier is its reducer, the literal table is its state, and the diff rides the
same channel the application already uses.

The classifier is **emit-diff**: it re-runs only the front-end and compares the new
emitted output against the previous; the edit is appearance-only iff the sole
difference is literal-table values. Because the emitted output is the source of
truth, a logic change can never be misclassified as appearance. It is **conservative
by construction** — anything not provably appearance-only recompiles.

The set of hoistable literals is a single declarative registry keyed by kernel. It
is **self-enforcing**: the match is exhaustive (a new kernel does not compile until
classified), and registry-driven tests iterate it so every entry is proven to render
byte-identically to its direct emit and to refuse hoisting a value that depends on
the Model. Extending appearance hot-swap to a new library is adding registry entries,
never touching the classifier.

**Logic edits — recompile, made fast the right way.** A change to structure, control
flow, a Model-dependent value, or a handler is a new program, not a new datum; it
recompiles. The recompile is kept cheap by **incremental compilation** (reusing the
unchanged monomorphized code across an edit) and by a **blue-green swap** in the dev
watcher: a persistent front proxy holds the port, the rebuilt binary starts behind
it and is cut over on a readiness signal, and the running Model is handed to the new
process — so a logic rebuild neither drops the browser connection nor loses your
place. This is a dev-only supervisor; a shipped program instead exposes drain and
readiness hooks and lets its deployment platform orchestrate.

### Rejected alternatives

- **Interpret the view (or ship a serialized view the client executes).** Fastest,
  but a second execution semantics that can diverge from the compiled output — the
  dev preview would lie. Rejected outright: it trades correctness for latency, which
  the precedence forbids. The literal table is the sound remnant of this idea,
  restricted to inert data the one compiled renderer already consumes.
- **Generalize the value table into a program table.** Collapses into the
  interpreter. The table stays data.
- **Split the application into a stable core crate and a thin view crate**, hoping a
  view edit recompiles only the view. Implemented and measured: it does not help.
  The entrypoint is intrinsic to the final binary, which re-instantiates the runtime
  generic surface reachable from it regardless of how the source is partitioned; a
  layout change cannot move that cost.
- **Break the inlining of the view into the runtime entrypoint** (via a
  never-inline annotation or function-pointer indirection) to make the runtime's
  generated code independent of the view body. Measured: no effect. The mechanism
  that reuses unchanged generated code across an edit is incremental compilation,
  which was simply disabled; enabling it is the actual lever.
- **A dedicated code generator that lowers directly to the browser target**, to make
  even a cold logic compile instant. The dominant cost is instantiating generic code
  for the program's own types, which is independent of the code generator; a new
  generator must still either pay it or erase types (reintroducing a dev-vs-prod
  representation gap), and interactive *logic* editing additionally needs
  function-granular incrementality the toolchain does not yet have. An unproven bet;
  deferred.

## Consequences

- Appearance edits reach the running program without any compilation; logic body
  edits pay only an incremental recompile; a type change re-instantiates the
  dependent surface and is the genuinely expensive case. This is the natural
  gradient — the cheapest edits are the most common.
- **The load-bearing invariant:** the literal table is read by the *one* compiled
  `view` in both the shipped program and the dev preview (baked defaults when never
  patched). A hot-swap therefore produces exactly what a full recompile of the same
  source would. This must never be weakened — no dev-only render path may be
  introduced, or dev ceases to equal prod and the whole mechanism becomes unsound.
  An automated conformance test pins it: rendering with the baked-default table is
  byte-identical to the direct emit.
- The classifier must stay conservative: a false "appearance" that hot-swaps a logic
  change is a correctness defect, whereas a false "logic" is merely a slower rebuild.
  The bias is always toward recompile.
- The appearance-literal registry must stay self-enforcing: an entry cannot exist
  without a proof, by construction, that it renders identically and refuses
  Model-dependent values.
- The whole hot-swap and blue-green apparatus is **dev-only** and may be aggressive
  precisely because it never ships. The shipped program is the ordinary compiled
  binary.

## Conventions

ADRs describe Ipê on its own terms. Do not reference any prior or external
implementation, parity with another system, or project ancestry — state each
decision as a standalone Ipê decision.
