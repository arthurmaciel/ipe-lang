# Departures from upstream Sky — idea log (research / deferred)

> **Purpose:** a single running log of ideas that **intentionally diverge** from
> the upstream Sky project's design. These are *our own* directions, mostly
> **runtime-side**, captured for later investigation — **not** committed to and
> **not** scheduled. They must not distract from the compiler core (M1–M6).
>
> **Governing rules:** every divergence here, if/when adopted, becomes a
> documented entry (per `PRINCIPLES.md`: a divergence is *documented*, never
> silent) and flips the relevant `docs/parity/runtime-parity.md` row from
> "mirrors Go" → "intentional Rust design + rationale + own tests." Until then we
> still **mirror** upstream behaviour (so we can keep tracking it); these ideas are
> the graduation path from *follower* to *designer*.
>
> Status legend: 🔬 research · 🧊 deferred · 🚧 prototyping · ✅ adopted.

---

## Idea 1 — Hotloading / hotpatching a running app (live UI on code change) 🔬🧊

**Goal:** edit Sky source and have a *running* app's UI update immediately, ideally
**preserving `Model` state** (the "edit `view`, see it change, state intact" loop).

**Key fact (don't conflate):** **salsa ≠ hotloading.** Salsa is incremental
*compilation* — it only makes the **rebuild** fast (`sky watch`/LSP). Getting new
code into a live process is a separate **runtime** mechanism.

**Mechanisms, ranked by principle-fit:**
1. **Dev-mode IR interpreter** — tree-walk the typed IR; hold the `Model`, hot-swap
   the interpreted `view`/`update` on change. *Safe* (no `unsafe`), reuses our IR,
   compile natively for prod. **Best fit.**
2. **WASM backend + host module reload** — swap the compiled WASM module, keep
   state in the host. Safe (sandbox); aligns with the multi-backend direction.
3. **Native dylib reload** (`hot-lib-reloader`-style) — fast but `dlopen` +
   fn-pointer transmute = `unsafe`/fragile. **Discouraged** (violates the spirit of
   `forbid(unsafe)` + soundness).
4. **Baseline (already ~half-there):** fast `sky watch` rebuild → restart →
   session-store state-persist → SSE reconnect. Salsa accelerates the rebuild step.

**Enabler:** Idea 3 (decoupled TEA). TEA's Model/code split is what makes
view-only hot-swap tractable; `Model`-shape changes need a migration story.

**Open questions:** `Model`-shape migration on type changes; granularity (view-only
vs `update` vs `Model`); how the dev host preserves in-flight `Cmd`/`Sub`; security
of an interpreter eval path (must stay sandboxed, no arbitrary host access).

---

## Idea 2 — `Std.Ui` as a backend-agnostic UI IR; target chosen by a function call 🔬🧊

**Today:** `Std.Ui` rendering is coupled to the *app shape* — Live → HTML, TUI →
ANSI, Webview → HTML. The target is implied by where you run.

**Idea:** make `Std.Ui` a **pure UI description (its own IR)**, and select the
target with an explicit **function call**, decoupling *what UI* from *how/where
rendered*. UI becomes **data you can target anywhere**:
- `Ui.toHtml : Element -> String` (generate HTML/CSS as a value)
- `Ui.toAnsi : Element -> AnsiBuffer`
- …one lowering per UI backend, analogous to `sky_ir` → code backends.

**Unlocks:** *generate* (not render) HTML/CSS from inside a **TUI** app; *generate*
ANSI from inside a **Live**/**Webview** app. Concretely enables a CLI or Live app
that **emits whole sites** — a programmatic/graphical **site-builder**.

**Principle notes:** these are **pure functions** (no I/O) → excellent fit.
**Security is load-bearing:** HTML *generation* must keep `Std.Ui`'s
HTML-escaping / no-`data-sky-eval` / no-XSS guarantees even when producing-not-
serving; ANSI generation must sanitize control bytes (mirror the Tui
`sanitiseRune` discipline). The UI-IR is, in effect, a second IR with its own
backend trait — keep it as cleanly bounded as `sky_ir`/`sky_backend`.

---

## Idea 3 — Decouple TEA from Live/TUI/Webview (standalone TEA engine) 🔬🧊

**Analogy:** Iced (GUI) and Ratatui (TUI) are *standalone* frameworks. Make the
**TEA runtime** (`Model`/`Msg`/`init`/`update`/`view`/`Cmd`/`Sub` loop) a
**backend-agnostic engine**; Live / TUI / Webview become **transports/drivers**
plugged into it, not the engine itself.

**Unlocks:**
- One TEA app driven by *any* transport (write once, run as web / terminal /
  desktop — already a Sky aspiration, but as a *library boundary*, not three
  hard-wired shapes).
- A **dev hot-reload host** (Idea 1) that owns the `Model` and swaps `update`/`view`.
- Free composition with Idea 2 (generate HTML in a TUI, ANSI in Live).

**Open questions:** the engine↔transport interface (event in, patch/render out);
where `Cmd`/`Sub` effect execution lives; how `Sky.Live`'s SSE/diff layer becomes
just one transport implementation.

---

## Why these three belong together

Standalone TEA engine (3) + `Std.Ui`-as-IR (2) + a hot-swap host (1) =
**"edit code → UI updates live, render any UI to any target."** That combination
is the foundation for the **graphical-and-programmatic site-builder** vision
(build sites in a CLI or inside a Live app, updating the UI directly from code).

## Disposition

All three are **runtime-side and depart from upstream Sky's coupling**, so they are
**research/deferred** until the compiler core is solid (M1–M6) and the
mirror/parity machinery is in place. Revisit when: (a) the IR + multi-backend
boundary is proven, and (b) we are ready to *design* rather than *mirror* — at
which point each becomes a real divergence-ledger entry with its own tests.

## Idea 4 — Deep nested-record-update sugar `{ r | a.b.c = v }` 🔬🧊

**Goal:** a concise deep-update form, since the manual nested form repeats the
access path at every level and gets noisy at depth 3+:
```elm
-- manual (works today on concrete records; parity-preserving)
{ model | user = { model.user | profile = { model.user.profile | bio = newBio } } }
-- proposed sugar
{ model | user.profile.bio = newBio }
```

**Why it's a departure:** upstream Sky follows Elm, which has NO deep-update
syntax. Adding it is a deliberate Sky-Rust **divergence** (a *superset* of the
grammar), so it must be documented and gated — never allowed to silently
disagree with upstream behaviour.

**Shape of the work (small, self-contained):**
- Parse a dotted field-chain on an update LHS: `field (. field)* = expr`.
- **Desugar in canon** into the existing nested `update`+`access` form (the
  manual example above) — so types/lower/backend see ONLY the primitives we
  already support; zero new IR, zero new codegen. Pure front-end sugar.
- Multi-field at the same prefix may share the path (optional optimization).

**Constraints / gates:**
- **Static path only** — the LHS must be a literal field chain; no computed or
  conditional paths.
- Works wherever the underlying nested `update`+`access` works: concrete records
  **now**; generic records once `SKY-L0111` (generic-record-update gate) is
  lifted in M2d.
- Since it desugars to parity-sound primitives, runtime behaviour is identical to
  the manual form — the only divergence is the *accepted grammar*. Record it in
  the parity ledger as "intentional surface extension" if adopted.

**Disposition:** filed (user opted in 2026-06-28). Implement as a canon-level
desugaring in a **post-M6 "designer" phase, after the parity-faithful compiler is
complete** — NOT mid-port. Rationale (2026-06-28): it's a divergence, so it
*can't* be checked against the Go oracle (Go's parser rejects it) and would muddy
every parity sweep + incur re-verification through M2d/M4/M5/M6 if added early;
the parse/canon surface is still moving until then; and deferring costs ~nothing
(documented, self-contained, depth 1–2 fine manually). General rule: parity
features go incrementally; **divergences go last, on a verified-complete base.**

## Idea 5 — Or-patterns (alternative patterns in one case arm) 🔬🧊

**Goal:** one arm matching several patterns, sharing a body — DRY vs Elm's
repeat-the-body:
```elm
-- Elm/Sky today: duplicate the body
case v of
    A -> arm1
    B -> arm1
    _ -> arm3
-- proposed
case v of
    A | B -> arm1
    _     -> arm3
```

**Syntax decision: `|` (NOT `||`).** Decided 2026-06-28. `||` is *boolean OR*
everywhere in the language; reusing it for pattern alternation overloads one token
with two structurally different meanings (value op vs pattern combinator). `|`
already means "one of these" via ADT sums (`type T = A | B`), matches
Rust/OCaml/F#/Python-3.10 match or-patterns, is unambiguous in pattern position
(no clash with record-update `{ r | f = v }` which is expression-position, nor with
type decls), and maps **1:1 to Rust's native or-pattern** `A | B => ...`. No `or`
keyword (needless reserved word).

**Why it's a departure:** Elm/Sky have no or-patterns. Superset of the grammar →
documented divergence, not oracle-verifiable.

**Shape of the work (small; mostly native):**
- Parse `pat (| pat)* -> body` in case arms (pattern position only; refutable
  context — case, not irrefutable let-destructure).
- IR: `Pat::Or(Vec<Pat>)`; backend emits Rust `p1 | p2 => body` (native).
- Exhaustiveness: extend the M3b-2 Maranget check to expand `p|q` into two rows
  (the algorithm already supports this — minimal work).

**Correctness gate (load-bearing):** every alternative MUST bind the **same set of
variables at the same types** (`Just x | Nothing ->` is illegal if `x` is used).
Reject mismatched-binding or-patterns with a fail-fast diagnostic in canon/types
(Rust rejects it too, but we must catch it first to stay sky-build⇒cargo-clean and
fail-fast). Runtime behaviour is identical to the duplicated-body form.

**Disposition:** filed (user opted in 2026-06-28). Post-M6 "designer" phase, on the
verified-complete parity base (same rule as Idea 4). Low effort (native Rust + the
Maranget algo already handles it), moderate value (real DRY win, very common
idiom) — a strong early candidate once divergences open.

## Idea 6 — Pattern guards (Haskell-style guards) 🔬🧊

**Goal:** condition an arm on a boolean predicate over the bound vars, with
fall-through to later arms:
```elm
case v of
    n if n < 1            -> arm1
    n if n >= 10          -> arm1
    s if String.isEmpty s -> arm2
    _                     -> arm3
```

**Syntax decision: `if` (NOT Haskell's `|`).** Decided 2026-06-28. Haskell uses
`|` for guards only because it has no or-patterns; we picked `|` for or-patterns
(Idea 5), so a guard marker of `|` would COLLIDE. Use Rust's spelling: `pattern if
cond -> body`. `if` is already a keyword but is unambiguous in case-arm-LHS
position. Guards COMPOSE with or-patterns: `A | B if cond -> ...` (guard applies to
the whole or-pattern). Maps **1:1 to Rust match guards** `n if n < 1 => ...`.

**Why it's a departure:** Elm/Sky have no pattern guards (you nest `if` in the arm
body). Superset → documented divergence, not oracle-verifiable.

**Shape of the work (native Rust target):**
- Parse `pattern (if <boolExpr>)? -> body` in case arms.
- IR: add an optional guard expr to `Arm { pat, guard: Option<Expr>, body }`.
- types: guard expr must be `Bool`, scoped over the pattern's bound vars.
- backend: emit Rust `pat if <guard> => body`.

**Correctness gate (load-bearing — soundness floor):** a **guarded arm does NOT
contribute to exhaustiveness** (the guard may be false). The M3b-2 Maranget check
must treat guarded rows as non-covering, so a case whose coverage relies on guarded
arms requires an unguarded/wildcard fallback else **SKY-T0010** — caught BEFORE
emit (Rust would otherwise reject the guard-only match as non-exhaustive E0004 =
exit-0-then-cargo-fail). Guards also affect redundancy: a guard can make an
otherwise-shadowed later arm reachable. Guard expr must be pure `Bool` (Sky is
pure → fine).

**Disposition:** filed (user opted in 2026-06-28). Post-M6 "designer" phase
(divergence rule). Pairs naturally with Idea 5 (or-patterns) — implement together;
both are native Rust, the only real work is the exhaustiveness/redundancy
adjustments. High value (very common idiom), moderate effort.

## Idea 7 — Effect/monadic sequencing sugar (UNDECIDED — 3 candidate syntaxes) 🔬🧊

**Problem:** effectful Sky code (Task/Result/Maybe chains) drifts into nested-lambda
pyramids:
```elm
readConfig |> Task.andThen (\cfg ->
  connect cfg |> Task.andThen (\conn ->
    query conn |> Task.andThen (\rows -> process rows)))
```
All three candidates below desugar to exactly this (nested callbacks / `andThen`) —
**zero runtime difference**; the choice is purely surface syntax + how much
machinery each introduces. **No decision yet** (coordinator leans against `use` as
too polluting — revisit post-M6).

**Candidate A — Gleam `use` (per-line keyword):**
```elm
use cfg  <- Task.andThen readConfig
use conn <- Task.andThen (connect cfg)
use rows <- Task.andThen (query conn)
process rows                              -- result = the rest of the function
```
`use x <- f` ⇒ `f (\x -> <rest>)`. PRO: general (any callback-last fn, not just
monads), no monad/typeclass abstraction, composes with normal code, trivial uniform
desugar, no compiler-known names. CON: `use` keyword + the chainer repeated on every
line — reads as visual noise to some (coordinator's concern).

**Candidate B — F#-style named block `Task.chain`:**
```elm
Task.chain
    cfg  <- readConfig
    conn <- connect cfg
    rows <- query conn
    process rows            -- trailing result expr required
```
PRO: cleanest vertical read; factors the chainer to one header. CON: reintroduces
"monad-shaped" machinery — either a per-type builder (`Task.chain`/`Result.chain`/…,
i.e. a typeclass-ish abstraction Elm deliberately refused) OR compiler magic on the
`.chain` naming convention; needs a trailing result expr; is a walled sub-block;
type-coupled.

**Candidate C — Roc-style postfix `!`:**
```elm
cfg  = readConfig!
conn = (connect cfg)!
rows = (query conn)!
process rows
```
`e!` runs the effect and binds its result. PRO: most concise; no keyword-per-line,
no block, no builder names. CON: needs an effect-tracking model in the type system
(Roc tracks effects/purity) to know what `!` may apply to; terse `!` is easy to miss;
the most machinery under the hood.

**Disposition:** filed (2026-06-28), UNDECIDED. Post-M6. Evaluate against: cleanliness
(coordinator: `use` too noisy), generality, and how much new abstraction each forces
(Elm-spirit favours LESS). Prior art: Haskell `do`, OCaml `let*`, F# computation
expressions, Gleam `use`, Roc `!`.

## Idea 8 — Record field-punning on construction 🔬🧊

**Goal:** when a local variable matches a field name, drop the redundant `= name`:
```elm
-- today
{ name = name, age = age, email = email }
-- punned
{ name, age, email }
```
Dual of the record-PATTERN punning we already support (`{ x, y }` destructure). Common
in Rust / JS / Gleam / OCaml.

**Why it's a departure:** Elm/Sky require `field = value` on construction. Superset of
the grammar.

**Shape of the work:** pure **canon-level desugar** — `{ name, age }` in expression
position ⇒ `{ name = name, age = age }`, resolving each bare field to the in-scope
local of the same name (error if no such local). Zero new IR / codegen / runtime.

**Only downside: a small loss of explicitness** — the reader no longer sees `= name`,
so "this field is filled from a same-named local" is implicit. Mitigated by: the field
names are still right there; it only fires when a matching local exists; it's a
well-loved idiom elsewhere. Low risk.

**Disposition:** filed (2026-06-28). Post-M6, low effort, low risk — a clean polish.

### Idea 7 update (2026-06-28) — preferred spelling for Candidate A: `let x <- e` (not `use`)

Coordinator finds `use` alien/polluting. Prefer reusing `let` with the `<-` arrow
to mark an effectful (callback) bind, distinct from `=` (pure bind):
```elm
let cfg  <- Task.andThen readConfig
let conn <- Task.andThen (connect cfg)
let rows <- Task.andThen (query conn)
process rows
```
Same desugaring as `use` (`let x <- f` ⇒ `f (\x -> <rest>)`); identical runtime.
Well-precedented: Haskell `do` `x <- action`, OCaml `let*`, F# `let!` — all mark
the effectful bind. `<-` reads as "draw x from e" and is free in Sky (Elm dropped
its old `<-`).

**Wrinkle (the cost of overloading `let`):** Sky's `let x = e in body` needs `in`
and may multi-bind; `let x <- f` has NO `in` (rest-of-block is the continuation).
So `let x <- e` must be a STANDALONE statement form, and `=` and `<-` cannot be
mixed inside one `let … in` multi-binding block (Haskell avoids this only because
its `<-` lives inside `do`). Parser disambiguates on the `=` vs `<-` token after the
binder. Net vs `use`: drops a new keyword, adds a small "which `let`?"
disambiguation — a familiarity win if `use` reads alien.

Open: `let x <- e` (favoured) vs `use` vs the `chain` block (7-B) vs Roc `!` (7-C).
Still UNDECIDED, post-M6.

### Idea 7 update (2026-06-28) — the `<-` fork + effect-discard ergonomics

Trigger: coordinator dislikes the effect-DISCARD idiom `let _ = println "hi" in ()`
(auto-forced Task) and asked whether `let _ <- println "hi"` replaces it. It exposes
that `<-` has TWO possible meanings — a real decision, not just spelling:

- **Gleam-style** `let x <- f` ⇒ `f (\x -> rest)`, f = a CALLBACK-ACCEPTING fn.
  General (resource brackets, guards, any callback-last fn); but each line names the
  chainer, and **bare `let _ <- println "hi"` does NOT type-check** (a Task value
  isn't a callback-acceptor) — you'd write `let _ <- Task.andThen (println "hi")`,
  which is LONGER than today's `let _ = …`. So Gleam-style does NOT fix the discard
  wart.
- **Haskell-style** `let x <- action` where action is an EFFECT VALUE (`Task e a`);
  bind is implicit ⇒ `Task.andThen action (\x -> rest)`. Then `let _ <- println "hi"`
  WORKS and is clean. Cost: a per-type bind notion for the effect types
  (Task/Result/Maybe/List) — a monad-ish abstraction Elm avoided. Mitigation: keep it
  **built-in for the small fixed set of effect types**, NOT a user-facing Monad
  typeclass, to stay Elm-clean.

**Cleanest discard form (independent of the above):** a **bare effect-statement line**
inside an effect block (Gleam itself uses this for discard):
```elm
    println "hi"            -- run, discard result, continue
    let x <- readLine       -- run, bind result
    println ("got " ++ x)
    Task.succeed ()
```
A bare line = run/discard; `let x <- e` = bind; `let _ <- e` becomes unnecessary.
This directly removes the `let _ = … in ()` wart.

**Revised disposition:** the effect-sequencing form AND the discard ergonomics are
ONE co-design. Leaning: a Haskell-style effect block (bare statement = run/discard,
`let x <- effect` = bind) with bind built-in for the fixed effect types only (no
general Monad class). Still UNDECIDED, post-M6. Open spellings: block header
(`do`-like / `Task.chain` 7-B) vs free-floating `let x <- e`; bare-statement
sequencing; Roc `!` (7-C). Decide together.

### Idea 7 update (2026-06-28) — effect/pure VISIBILITY is a first-class criterion (reorders candidates)

Coordinator: distinguishing effectful vs pure code is very important. This becomes a
ranking criterion — and it changes the order.

- **Gleam is the wrong model here:** Gleam is impure + effect-UNTRACKED; nothing in
  its types or syntax marks `io.println` as an effect (only the module name hints).
  So Gleam-`use` (and a bare-statement line) give **no inline effect marker** — they
  rank LOW on this axis.
- **Sky already marks effects at the TYPE level:** every effect returns `Task e a`
  (effect-boundary rule). The `Task` type IS the pure/effect marker. PRESERVE this —
  any sugar must not erase it.
- **Roc `!` marks effects INLINE, per call:** `read! config`, `println! "hi"`. A
  visible marker on every effectful call (Roc's compiler enforces it via purity
  inference). Fits Sky: effects are already reified as `Task`, so `!` = "run this
  Task here and bind its result," legal only in an effect context, visible per call;
  handles bind (`x = read! cfg`) AND discard (`println! "hi"` line). Essentially
  Haskell-style bind with a clear postfix marker instead of a quiet `<-`.

**Revised lean:** on the combined axes (effect VISIBILITY + clean discard + no
general Monad class), **Roc-style `!` now leads** — it keeps effects visible at the
call site (Gleam-`use`/bare-line do not), kills the `let _ = … in ()` wart
(`println! "hi"`), and rides Sky's existing `Task` reification (no new typeclass;
the compiler just gates `!` to Task-typed exprs in an effect context). `let x <- e`
and `Task.chain` blocks remain options but LOSE inline effect-marking unless paired
with a marker. Still UNDECIDED, post-M6 — but the criterion ordering is now: keep
the `Task` type marker; prefer a VISIBLE per-call form (`!`) over invisible ones.

### Idea 7 update (2026-07-03) — `Task.block` scoped-`!` refinement (coordinator steer; UNDECIDED, DEFERRED)

Coordinator dislikes FREE-FLOATING `!` (scattered; "where is `!` legal?" is fuzzy).
Refinement: **FENCE `!` inside a `Task.block`** — `!` is legal ONLY inside a block, which
evaluates to `Task Error a`.
- `x = e!` effect-bind; pure `x = e` (no `!`); a **bare `e!` line = run/discard** (kills
  the `let _ = … in ()` wart); `!` may fire **INLINE** mid-expression
  (`(read! a) + (read! b)`, fired LEFT-TO-RIGHT). Final expr = the tail (a pure final
  auto-`Task.succeed`-wraps).
- **DOUBLE visibility**: the block boundary marks the effect REGION + `!` marks each
  effect LINE (pure lines have no `!`) — strictly more visible than bare `!`.
- Desugars to `Task.andThen`; **Task-specific, no Monad typeclass**. **REPLACES the
  `let _ = TaskExpr` auto-force** (retire it — `Task.block` becomes the one sequencing
  form; effects outside a block are just `Task` values for `Cmd.perform`/`Task.run`).
- Debug tracing in PURE code stays a dev-only `Debug.*` (prod-hard-error) — orthogonal.
  Still **NO** general `Task.do : Task e a -> ()` (that erases the `Task` marker — the
  effect-visibility leak the whole idea exists to avoid).

Coordinator's lean: `Task.block` > bare `!`. THREE OPEN sub-decisions (coordinator is
thinking): (1) **Task-only vs per-type blocks** — lean Task-only (Result/Maybe keep
`andThen`+`map2-5`; a `Result.block`/`Maybe.block` family reintroduces the per-type-builder
machinery Elm avoids); (2) **bind spelling** `=`+`!` (lean `!` — only spelling that also
works INLINE) vs `<-` (statement-level only); (3) **name/shape** `Task.block` (a
block-introducer, indentation-delimited like `let`; mild magic on a qualified name) vs a
real keyword (`effect`/`perform`). DEFERRED 2026-07-03 — coordinator wants more thinking
time. Post-M6 regardless (divergence, not oracle-verifiable).

## Idea — Elm-style time-travel debugger for live apps (dev-only) 🔬🧊

A built-in debugger for TEA/live apps (Sky.Live + Webview + Tui), in the spirit of
Elm's debugger: record the Msg history, step back/forward through states, inspect
the Model at each step, import/export sessions. **Dev mode only** (off in
production — no overhead, no surface).

Future improvements to design when the time comes:
- **Simulate Model-value changes** — edit a Model field in the debugger and replay
  forward to see the effect (if feasible given the typed Model + serde round-trip).
- Msg injection / replay, time-scrubbing, diff between adjacent states.

Disposition: filed (2026-06-29). Post-M6 (needs the live runtime first). Ties to
the hot-reload idea (Ideas 1-3) — same dev-loop family.

## Idea — Language-level ADT⇄string ergonomics (parse-don't-validate, facilitated) 🔬🧊

`parse, don't validate` is a rule WE enforce on the compiler — but the language
itself should make it cheap for the *user* to follow. The recurring case: a sum
type used as an option set —

```elm
type Theme = Light | Dark | Purple
```

— almost always needs a `Theme -> String` (rendering UI, persisting, query
params) AND a `String -> Maybe Theme` (parsing a request/stored value back into
the typed value). Today the user hand-writes both, and they drift. Two
directions to brainstorm (keep ipê's syntax clean — this is the open question):

1. **LSP-assisted, no new syntax.** An LSP code-action that generates the
   `toString` / `fromString` (and a round-trip property test) from the variant
   list — regenerated when variants change. Zero magic in the language; the
   functions are ordinary, visible Sky. Scales to many variants without
   boilerplate. (Depends on good LSP — ties to [[incremental-compilation-salsa]].)

2. **Syntax extension — variant name + stringified name as a single SSOT.**
   Declare the wire/display name alongside each variant once, and the compiler
   derives `Theme.toString` / `Theme.fromString` automatically, e.g. a sketch:

   ```elm
   type Theme = Light "light" | Dark "dark" | Purple "purple"
   -- auto: Theme.toString Light == "light"; Theme.fromString "dark" == Just Dark
   ```

   Single source of truth, no drift. But it's "magic" derivation — the core
   question is whether that magic is *good* (ergonomic, predictable) or *bad*
   (hidden behaviour, surprises). Compare: Rust `#[derive(Display/FromStr)]` +
   `strum`, Haskell `deriving (Show/Read)` vs custom, Elm's explicit-everything
   stance (which would reject the magic). The variant-with-payload case
   (`type Shape = Circle Float | Rect Float Float`) complicates a blanket
   derive — decide whether the SSOT name applies only to the *tag*.

Disposition: filed (2026-06-30). Brainstorm post-core. The "is this magic good?"
judgement is the crux — lean toward the LSP route if the magic proves
surprising. Connects to the parse-don't-validate principle and the
namespace/identity rethink ([[post-completion-rename-and-namespace]]).

## Idea — CI patch queue over the upstream examples corpus (departure-proof regression sweep) ✅

**Problem.** The examples sweep (35 upstream examples + Go-oracle equivalence)
is our best regression corpus, but every surface departure breaks it by
definition: dropping `Task.run`/`Task.perform` (#128), multiline-string margin
stripping (#133), the eventual rename, and any future syntax change make the
pristine upstream sources uncompilable — exactly when we most need the sweep
as a safety net.

**Proposal (user, 2026-07-05).** Keep the examples byte-identical to upstream
and carry a **patch queue** in-repo that CI applies before build/run/perf:

```
tests/example-patches/
  20-cli-counter/0001-drop-trailing-task-run.patch
  07-todo-cli/0001-margin-strip-multiline.patch
  …
scripts/examples-sweep.sh --patched   # rsync example → tmp, apply patches, build
```

* **Unpatched = upstream.** The tree keeps a clean provenance story; syncing a
  new upstream release is `git subtree`-style copy + patch rebase, never a
  hand-merge of forked sources.
* **Patch-apply failure is a feature** — it fires precisely when upstream
  changed the lines we diverge on, forcing a conscious rebase instead of
  silent drift. The sweep stays green-everywhere as a hard gate.
* **Go-only examples become reachable**: an example that upstream ships only
  for a Go-specific FFI surface gets an adaptation patch (or a full
  replacement file in the same queue) so Ipê can build/run/perf it — widening
  the corpus instead of skipping rows.
* **Oracle policy per patch class**: output-neutral departures (`Task.run`
  drop) keep byte-equivalence against the Go oracle running the UNpatched
  source; output-changing departures (margin stripping alters string
  literals) record an `oracle_divergence + reason` per the sanctioned-
  divergence policy — the patch file itself is the natural place to carry the
  annotation (header comment).
* **Codemod synergy**: mechanical departures should ship with a `skyc fix`
  auto-migration; CI can GENERATE those patches by running the migrator over
  pristine sources and diffing. The queue then doubles as an end-to-end test
  of the user-facing migration tool — if the migrator can't produce a working
  patch for our own corpus, it isn't ready for users' code.

**Disposition: accepted (2026-07-05), execute at Tier-3 start** — first
consumer is #128/#133; wire `--patched` mode into the sweep + CI (#37) when
the first departure lands. Until then the queue is empty and the sweep runs
pristine.
