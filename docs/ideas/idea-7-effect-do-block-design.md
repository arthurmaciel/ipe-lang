# Idea 7 — The `do` effect block (design)

> **Status:** DESIGNED (2026-07-04). Implementation deferred **post-parity**
> (this is a syntactic divergence from Elm/Ipê, not oracle-verifiable — it must
> not compete with the examples-sweep-green push). Supersedes the three earlier
> candidate spellings (Gleam `use`, `Task.block` + `!`, Roc bare `!`) — filed
> in `docs/divergences-from-sky.md#planned-future-divergences` §6.5.

## 1. Summary / the decision

Add a **`do` block** — a Task-only, keyword-introduced statement context for
writing sequential effectful code in direct style instead of an `andThen`
pyramid. Three line forms, distinguished by the operator alone:

```elm
do
    row <- queryUser uid       -- <-      effect-bind: run the Task, bind its result
    n    = row.age + 1         -- =       pure-bind (no `let`, no `in`)
    logAccess uid              -- (bare)  run for effect, discard the result
    User row (mk n)            -- tail    the block's result expression
```

- **Task-only.** Not `Result`/`Maybe` (§3).
- **Keyword `do`**, not a function-shaped `Task.block`, not `perform` (§4).
- **No `let` inside the block** — pure binds are bare `x = e`, reusing Ipê's
  existing definition-binding shape (§4).
- **Desugars to `Task.andThen`** — no Monad typeclass, zero runtime cost (§6).
- **Short-circuits to the first `Err`** — `Task.andThen` semantics (§7).
- **Retires the `let _ = TaskExpr` auto-force** (§8) and pairs with a dev-only
  `Debug.*` as the *only* effect permitted in pure code (§8).

Everything below the block header is sugar: `do { … }` lowers to exactly the
`Task.andThen` chain a user writes today. No new IR, no runtime support.

## 2. The problem — and where it actually bites

Sequential effectful code drifts into nested-lambda pyramids:

```elm
queryUser uid
    |> Task.andThen (\row ->
        logAccess uid
            |> Task.andThen (\_ ->
                queryPrefs row.id
                    |> Task.andThen (\prefs ->
                        Task.succeed (User row prefs))))
```

**Crucial scoping note — this is NOT primarily a TEA/frontend problem.** In
idiomatic Elm/Ipê TEA, sequential effects are decomposed into **Msg
transitions**: `Task.attempt`/`Cmd.perform` turns a Task into a `Cmd` whose
result returns as a Msg, so a multi-step flow becomes separate `update`
handlers, not a nested chain — and that's deliberate (you *want* to update the
Model and paint intermediate UI between steps). `Task.andThen` is used in TEA
only for short, atomic sub-sequences you don't want to interrupt with a Msg.

The deep pyramid is a **server / CLI / script** phenomenon — Node-style
sequential I/O with no UI to update between steps (`Ipe.Http.Server`,
`Ipe.Cli`, background jobs; e.g. `examples/07-todo-cli`, `examples/18-job-queue`).
Elm never had that surface (browser-only); Ipê does. **So the `do` block is
sugar for Ipê's non-TEA effect surface** — a real but bounded footprint. That
footprint is the justification; weigh the feature against it.

## 3. Why Task-only (a principle, not a preference)

`Result` and `Maybe` are **values with visible constructors** — you can
`case` on them, and you have `andThen` plus applicative `map2..5` / `andMap`.
Two composition tools already.

`Task e a` has **neither**. It is a *suspended effect*: no constructors,
nothing to match; `case someTask of …` is meaningless. The only way to reach a
Task's result in direct style is to **run** it. So Task is the one type where
run-and-bind sugar adds a genuinely new capability rather than saving nesting.
That retires `Result.block`/`Maybe.block` as **redundant** (not merely "monad
machinery we avoid") and keeps us clean of any Monad typeclass. `do` over a
non-Task is a compile error (§9).

## 4. Syntax — keyword `do`, no `let`

**Why `do`, not `Task.block`:** a `Task.block` looks like a function/value but
is a syntactic form (can't be aliased or passed) — a form disguised as a
qualified name. `do` is an honest keyword and the established name for exactly
this construct (Haskell / PureScript / Idris).

**Why not `perform`:** `perform` already means something in Ipê —
`Cmd.perform task toMsg` and `Task.perform` are the TEA functions that *run* a
Task. A `perform` block keyword would collide head-on with them.

**Why no `let` inside the block:** Elm/Ipê require `let … in` *always* — there
is no bare `let x = e` statement. Rather than invent a bare `let` (a wart), the
block drops `let` entirely: pure binds are bare `x = e`, which is **already
Ipê's binding shape** at module level (`main = …`, `foo x = …`). So inside a
`do` block, `n = e` reads like a local definition — not a new syntax. `let … in`
stays the *expression*-level binder, unchanged, everywhere else. (Eliminating
`let … in` language-wide, Roc-style, is a separate and much larger identity
decision — explicitly out of scope here.)

The three line forms are distinguished by the operator between binder and
expression: `<-` (effect), `=` (pure), or neither (discard). No `!` — a single
punctuation mark carrying bind + discard + (by-absence) pure was rejected as
easy to misread.

## 5. Examples

**Happy path — the payoff:**

```elm
loadUser : UserId -> Task Error User
loadUser uid =
    do
        row   <- queryUser uid
        logAccess uid                -- bare line: run + discard
        prefs <- queryPrefs row.id
        User row prefs               -- pure tail → auto Task.succeed-wraps
```

vs today's four-deep `andThen` pyramid.

**Pure intermediate bind (no `let`):**

```elm
do
    row <- queryUser uid
    n    = row.age + 1               -- pure-bind, no `let`/`in`
    logAccess uid
    User row (mk n)
```

**What does NOT belong in a `do` block — effects in pure code.** A `do` block
produces a `Task`; it never runs anything itself. You cannot smuggle an effect
into a pure computation:

```elm
-- WRONG: this function returns Int (pure); `do println` here would make a
-- pure-typed function secretly effectful — the exact leak Task-boundary forbids.
compute =
    let
        a   = 1
        b   = 2
        sum = a + b
        _   = Debug.println sum      -- the ONLY effect allowed in pure code:
    in                               -- dev-only, impure-by-design, prod-hard-error
    sum
```

`Debug.*` (Elm's `Debug.log`, stripped in production) is the single sanctioned
way to observe inside pure `let … in`. Real effects live only where the type is
`Task` — that separation is what keeps "effectful vs pure" readable at a glance.

## 6. Code transformation (desugaring)

A `do` block is a sequence of statements followed by a **required tail
expression** `E`. Write `⟦ · ⟧` for the desugaring to a plain Task expression:

```
Tail (single trailing expression E):
    ⟦ E ⟧  =  E                    if  E : Task Error a
    ⟦ E ⟧  =  Task.succeed E       if  E : a   (pure — auto-wrap)

Effect-bind:
    ⟦ (p <- e) ; rest ⟧  =  e |> Task.andThen (\p -> ⟦ rest ⟧)

Pure-bind:
    ⟦ (p = e) ; rest ⟧   =  let p = e in ⟦ rest ⟧

Discard (bare effect line, e : Task Error _):
    ⟦ e ; rest ⟧         =  e |> Task.andThen (\_ -> ⟦ rest ⟧)
```

Worked, on the §5 example:

```elm
do                                          queryUser uid
    row <- queryUser uid                        |> Task.andThen (\row ->
    n    = row.age + 1            ≡                  let n = row.age + 1 in
    logAccess uid                                    logAccess uid
    User row (mk n)                                      |> Task.andThen (\_ ->
                                                            Task.succeed (User row (mk n))))
```

The block's type is `Task Error T`, where `T` is the tail's result type
(unwrapped if the tail is itself `Task Error T`). Pure-bind `let p = e in …`
reuses the existing `Let` AST node — so **canon / type-inference / lower / emit
are untouched**; the whole feature is a **parser + desugar pass** producing
today's `Task.andThen` + `Let` nodes. No new IR, no runtime, no Monad
abstraction.

## 7. Failure semantics — short-circuit to first `Err`

Because `Task.andThen f (Err e) = Err e` (the continuation never runs), the
block **short-circuits at the first failing effect step**, exactly like Rust `?`
/ Haskell `do` over `Either`:

- A `<-` bind or bare discard line that yields `Err e` → the block returns that
  `Err e`; **all subsequent lines (including the tail) are skipped**.
- A pure `=` line **cannot** contribute an `Err` — it produces a plain value,
  not a `Task`, so it has no error channel. (A genuinely partial pure op like
  `x / 0` is a *runtime panic* classified by the synchronous-panic gate, NOT an
  `Err` in the block's channel.)
- The tail on success → `Ok (tail)`.

All Tasks in the block share the one `Error` type (`Task Error a`), so failures
unify with no `mapError` ceremony.

**No rollback.** Short-circuit skips *future* steps; it does not undo effects
that already ran. If `logAccess` succeeded and a later step failed, the log
already happened — `Task` has no transaction semantics (reach for
`Db.withTransaction` when you need all-or-nothing).

This is the ergonomic win: write the happy path top-to-bottom; the first failing
effect exits with its error, no per-step `case … of Err …`. It is exactly what
the AGENTS.md two-level error pattern rides on — the block propagates `Err` to
the boundary (`Task.onError` / `Cmd.perform … ResultMsg`), where the errId +
structured log are attached.

## 8. What it retires + the `Debug.*` relationship

**Retires the `let _ = TaskExpr` auto-force.** Today Ipê auto-forces a discarded
Task binding (`let _ = println "x" in …` runs the effect via `rt.AnyTaskRun`).
That overloads Elm-valid-but-*inert* syntax (`let _ = …` is a pure no-op in Elm)
to mean "perform" — a hidden effect keyed off a discarded wildcard. The `do`
block replaces it with an explicit, visible discard (the bare line). After `do`:

- Effect sequencing = the `do` block.
- Effects *outside* a block are just `Task` values, consumed by
  `Cmd.perform` / `Task.run` / `Task.parallel` at the boundary.
- Observing inside genuinely pure code = **`Debug.*`** only (dev-only,
  production-hard-error). No general `Task.do : Task e a -> ()` — that would
  erase the `Task` marker, the very effect-visibility leak this design closes.

## 9. Compiler assistance (kind-teacher)

The operator *is* the effect marker, and the compiler polices the boundary — so
"effectful vs pure" is not just visible but checked:

| Situation | Compiler response |
|---|---|
| `p = e` where `e : Task Error a` | **Suggest** (not error): "`p : Task Error a` — did you mean `p <- e` to run it?" (binding a Task *value* is legal, e.g. building a list for `Task.parallel`, so it stays a hint) |
| bare discard line where `e` is **pure** (not a `Task`) | **Warn/error**: "this value is computed and discarded; bare lines run effects — drop it, or `_ = e` if intentional" (enforces: bare line ⇒ effect) |
| block ends with a binding (no tail expr) | **Error**: "a `do` block must end in an expression" |
| `do` over a non-Task (e.g. a `Result` step) | **Error**: "`do` is Task-only; use `case`/`andThen` for `Result`/`Maybe`" |

## 10. Grammar + lexer

```
expr      ::= … | doBlock
doBlock   ::= 'do' INDENT stmt+ tailExpr DEDENT      -- layout-delimited, like let-bindings
stmt      ::= pattern '<-' expr                       -- effect-bind
            | pattern '=' expr                        -- pure-bind
            | expr                                    -- discard (any non-final line)
tailExpr  ::= expr                                    -- required final expression
```

- `do` becomes a reserved keyword.
- `<-` is a single token (Elm freed it). Disambiguation with compare-to-negative
  `x < -3` is already handled by Ipê's rule that negative literals need
  parentheses (`x < (-3)`), so no new lexer rule.
- Parser distinguishes the three `stmt` forms by scanning for a top-level `<-`
  then `=` at brace/paren depth 0 (Ipê has no `=` operator in expressions; `=`
  only appears inside `{ … }` records) — unambiguous.

## 11. Deliberately NOT done

- **Not `!` (Roc):** one punctuation mark carrying bind + discard + pure-by-
  absence is easy to misread; and inline firing `(read a!) + (read b!)` was the
  only thing `!` bought — dropped with it.
- **Not `Task.block`:** a syntactic form disguised as a qualified function name.
- **Not `perform`:** collides with `Cmd.perform` / `Task.perform`.
- **Not `Result.block` / `Maybe.block`:** redundant (§3).
- **Not eliminating `let … in`** language-wide (Roc-style): a separate, larger
  identity decision with a big migration cost and loss of Elm familiarity; the
  `do` block coexists with `let … in` exactly as Haskell does.

## 12. Effort

Moderate, front-loaded on the parser:

- **Parser** (`sky_parse`): new `doBlock` production + layout handling +
  three-form statement disambiguation. The bulk.
- **Desugar**: a pass lowering `doBlock` → existing `Task.andThen` + `Let` nodes
  (canon or a pre-canon desugar). Because it targets existing nodes,
  **type-inference / lower / emit / LSP are essentially untouched**.
- **Formatter** (`sky_format`): render the block form.
- **Compiler suggestions** (§9): the effect-visibility checks + hints.
- **Docs / `templates/AGENTS.md`**: syntax reference + the auto-force retirement.

No new IR, no runtime, no oracle divergence at the value level (emitted Task
behaviour is identical to hand-written `andThen`).

## 13. Disposition

**DESIGNED, implementation deferred post-parity.** It is a syntactic divergence
(records in `docs/divergences-from-sky.md` when built) and not oracle-verifiable
at the source level, so it must not compete with the examples-sweep-green push.
Sequence it after parity, alongside the other macro-roadmap syntactic
departures (Ideas 5 or-patterns, 6 guards, 8 field-punning). When implemented,
update `templates/AGENTS.md` + `docs/` in the same commit (template-sync rule).

## 14. Open sub-questions (small; settle at implementation)

1. **Pattern binds in `<-` / `=`.** Allow `(a, b) <- e` and `{ x, y } = e`
   (destructuring binds)? Lean **yes** — they desugar through the same `\p ->`
   / `let p =` and Ipê already supports the patterns (#96). Refutable patterns
   in `<-` (e.g. `Just x <- e`) would need a failure story — lean **disallow**
   (require irrefutable), since Task has no `MonadFail`; use an explicit
   `case` on the bound value instead.
2. **Empty / single-line `do`.** `do` with only a tail (`do  e`) = just `e`
   coerced to `Task` — allow as a degenerate case, or require ≥1 statement?
   Lean allow (harmless; `do e` ≡ `⟦ e ⟧`).
3. **`Debug.println` vs keeping `println` pure-callable.** The auto-force
   retirement implies `println` in pure code should become a `Debug.*` call.
   Confirm the `Debug` surface (`Debug.println` / `Debug.log` / `Debug.toString`)
   and the production-hard-error gate when finalising §8.
