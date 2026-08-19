Status: Accepted

# 60. `do` hardening: reject stepless blocks; remove `doParallel`

## Context

ADR-0050 introduced `do` as Task-sequencing sugar and `doParallel` as a
concurrent fan-out keyword. Two weaknesses emerged in practice.

**Stepless `do`.** ADR-0050 permits `=` pure-let bindings inside a `do` block,
and auto-wraps the trailing expression in `Task.succeed` when it is pure. A
block containing only `=` bindings — no `<-` Task bind, no bare-run Task
step — compiles today but writes pure code inside an effect block. The author
chose `do` where `let … in` is the right tool; the `Task.succeed` wrapping is
accidental. Nothing prevents a reader from misreading the block as effectful.

**`doParallel` redundancy.** `doParallel a b c` desugars to
`Task.parallel [a, b, c]`. `Task.parallel : List (Task Error a) -> Task Error (List a)`
is a plain function available anywhere, including as a `<-` bind target inside
a `do`:

```
results <- Task.parallel [a, b, c]
```

That spelling is already unambiguous, discoverable from `Task`'s documentation,
and consistent with every other `Task` combinator. `doParallel` adds a parallel
keyword form that earns no expressive power over the explicit function call; it
is extra surface to learn for zero gain.

## Decision

**Reject stepless `do`.** A `do` block whose every statement is a `=` pure-let
binding — no `<-` Task bind and no bare-run line anywhere, the final one
included — is a compile error: `IPE-P0065`. A bare-run line is a Task step
regardless of position, so a `do` ending in one passes this gate; whether that
final expression is genuinely effectful is a type-level question left to the
lowering gates (`IPE-L0141`), not a parse-time one. The check is purely
structural because the parser cannot see types. The message directs the author
to `let … in` for pure bindings and to `Task.succeed` when a `Task` result is
genuinely needed. Detection happens in the parser's `desugar_do` fold, before
any other stage sees the AST.

The invariant that follows: `do` and `let … in` are **disjoint by
construction**. `let … in` is pure binding; `do` is Task sequencing with at
least one real step. An author cannot pick the wrong one and have it silently
accepted.

**Remove `doParallel`.** The `doParallel` keyword is removed from the lexer.
The token, its parse path, and its desugar are deleted. Every existing
`doParallel` use is migrated to `results <- Task.parallel [...]` inside a `do`
block. The `Task.parallel` function itself is unchanged.

Alternatives rejected:

- **Keep `doParallel` as a convenience shorthand.** It adds surface area for
  zero expressive gain; `Task.parallel [...]` is already the canonical spelling,
  and `<-` makes the bind explicit. Explicit-over-magic is the language's
  consistent principle.
- **Warn rather than error on a stepless `do`.** A warning is ignorable;
  errors enforce the invariant. The boundary between pure code and Task
  sequencing is load-bearing: letting it erode via an accepted-but-warned
  misuse recreates the problem the rule is solving. Fail-closed is the correct
  choice.
- **Auto-rewrite a stepless `do` to `let … in`.** Silent rewrites violate the
  principle that the compiler teaches rather than silently fixes. Showing the
  error with a directed message is more valuable to a reader's long-term
  understanding.

## Consequences

- A `do` block with no Task step is a compile error. Authors who wrote such
  blocks must rewrite them as `let … in`; the error message names both the
  fix and the alternative.
- `doParallel` is no longer a keyword. The identifier `doParallel` is now
  available as a plain binding name (it was previously a reserved word). Any
  existing `doParallel` use is a straightforward mechanical migration to
  `results <- Task.parallel [...]`.
- The invariant that must keep holding: a `do` block is only ever Task
  sequencing, never pure binding dressed as effect code. `let … in` remains
  the sole supported form for pure sequential binding.
- This decision supersedes the `doParallel` arm of ADR-0050; the `do`
  sequential sequencing rules from ADR-0050 are otherwise unchanged and remain
  in force.

## Conventions

ADRs describe Ipê on its own terms. This decision is stated as a standalone
Ipê decision, without reference to any prior or external implementation.
